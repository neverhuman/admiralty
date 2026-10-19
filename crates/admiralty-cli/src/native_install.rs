//! Integrity-pinned native packages installed into one new private directory.
//!
//! This accepts only the canonical Rust native archive format. It never runs
//! package bytes, changes a current link/profile/service, or authenticates a
//! publisher. An owning UID can subsequently modify its own files; mode0700
//! is a boundary against other users, not hostile same-UID code or the OS.
use crate::native_archive::{expected_tree, portable};
use crate::native_package::{
    PackageEntry, PackageManifest, validate_staged_guard_closure, validate_staged_manifest,
};
use flate2::bufread::GzDecoder;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags};
use serde::{
    Deserialize, Serialize,
    de::{self, SeqAccess, Visitor},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

const MANIFEST: &str = "MANIFEST.json";
const MAX_MANIFEST: usize = 1024 * 1024;
const MAX_FILES: usize = 1024;
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE: u64 = MAX_TOTAL + 16 * 1024 * 1024;
const MAX_NODES: usize = 8192;
const CHUNK: usize = 64 * 1024;
const GZIP_HEADER: [u8; 10] = [31, 139, 8, 0, 0, 0, 0, 0, 2, 255];
type Result<T> = std::result::Result<T, InstallError>;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InstallReconciliation {
    pub destination: PathBuf,
    pub temporary_directory: PathBuf,
    pub staged_device: Option<String>,
    pub staged_inode: Option<String>,
    pub archive_sha256: String,
    pub manifest_raw_sha256: String,
    pub publication_may_have_occurred: bool,
}
#[derive(Debug)]
pub enum InstallError {
    Invalid(&'static str),
    Io(io::Error),
    /// Preserve both names and the original archive; never automatically retry.
    CommitUncertain(Box<InstallReconciliation>),
    /// Cleanup could not prove exclusive ownership; no unrelated name is removed.
    CleanupUncertain(Box<InstallReconciliation>),
}
impl InstallError {
    pub fn reconciliation(&self) -> Option<&InstallReconciliation> {
        match self {
            Self::CommitUncertain(v) | Self::CleanupUncertain(v) => Some(v),
            _ => None,
        }
    }
}
impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(stage) => write!(f, "native installation refused: {stage}"),
            Self::Io(error) => write!(f,"native installation I/O failed ({:?})",error.kind()),
            Self::CommitUncertain(_) => f.write_str("native installation commit requires reconciliation; preserve destination, temporary name and archive"),
            Self::CleanupUncertain(_) => f.write_str("native installation cleanup requires reconciliation; preserve temporary directory and archive"),
        }
    }
}
impl std::error::Error for InstallError {}
impl From<io::Error> for InstallError {
    fn from(v: io::Error) -> Self {
        Self::Io(v)
    }
}
impl From<rustix::io::Errno> for InstallError {
    fn from(v: rustix::io::Errno) -> Self {
        Self::Io(v.into())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InstallReceipt {
    pub destination: PathBuf,
    pub version: String,
    pub target: String,
    pub source_commit_label: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub manifest_raw_sha256: String,
    pub unsigned_manifest_sha256: String,
    pub installed_file_count: u32,
    pub installed_payload_bytes: u64,
    pub installed_device: String,
    pub installed_inode: String,
    pub installation_scope: &'static str,
    pub installed_bytes: bool,
    pub installer_complete: bool,
    pub daemon_running: bool,
    pub autostart: bool,
    pub profile_changed: bool,
    pub path_changed: bool,
    pub publisher_authenticated: bool,
    pub source_commit_verified: bool,
    pub signed: bool,
    pub published: bool,
    pub execution_granted: bool,
    pub acceptance_granted: bool,
    pub native_execution_qualified: bool,
    pub activated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    uid: u32,
    links: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
fn identity(file: &File) -> io::Result<Identity> {
    let m = file.metadata()?;
    Ok(Identity {
        dev: m.dev(),
        ino: m.ino(),
        size: m.len(),
        mode: m.mode(),
        uid: m.uid(),
        links: m.nlink(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    })
}
fn same_inode(a: &Identity, b: &Identity) -> bool {
    a.dev == b.dev && a.ino == b.ino
}
fn private_directory(file: &File) -> Result<()> {
    let m = file.metadata()?;
    if !m.is_dir() || m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o7777 != 0o700 {
        return Err(InstallError::Invalid(
            "directory must be owned and mode0700",
        ));
    }
    Ok(())
}
fn directory_at(parent: &File, name: &str) -> Result<File> {
    Ok(File::from(rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}
fn absolute_directory(path: &Path) -> Result<File> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(InstallError::Invalid("absolute directory bound"));
    }
    let mut fd = File::from(rustix::fs::openat(
        rustix::fs::CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or(InstallError::Invalid("directory encoding"))?;
                fd = directory_at(&fd, name)?;
            }
            _ => return Err(InstallError::Invalid("directory traversal")),
        }
    }
    Ok(fd)
}
fn named_regular(parent: &File, name: &str) -> Result<File> {
    let st = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile || st.st_nlink != 1 {
        return Err(InstallError::Invalid(
            "input must be a regular single-link file",
        ));
    }
    let fd = File::from(rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let id = identity(&fd)?;
    if i128::from(st.st_dev) != i128::from(id.dev)
        || i128::from(st.st_ino) != i128::from(id.ino)
        || id.uid != rustix::process::geteuid().as_raw()
        || id.mode & 0o022 != 0
    {
        return Err(InstallError::Invalid(
            "input identity, ownership or permissions",
        ));
    }
    Ok(fd)
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_file(file: &mut File, size: u64, cap: u64) -> Result<String> {
    if size > cap {
        return Err(InstallError::Invalid("file byte bound"));
    }
    let before = identity(file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut sha = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0; CHUNK];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or(InstallError::Invalid("file byte overflow"))?;
        if total > size {
            return Err(InstallError::Invalid("file grew"));
        }
        sha.update(&buffer[..n]);
    }
    if total != size || identity(file)? != before {
        return Err(InstallError::Invalid("file changed"));
    }
    Ok(format!("{:x}", sha.finalize()))
}

// Canonical raw bytes close the existing serialize/deserialize manifest shape.
// A list visitor enforces the producer's count before growing its owned vector.
fn entries<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<PackageEntry>, D::Error> {
    struct Bounded;
    impl<'de> Visitor<'de> for Bounded {
        type Value = Vec<PackageEntry>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("at most1024 native entries")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while out.len() < MAX_FILES {
                match seq.next_element::<PackageEntry>()? {
                    Some(value) => out.push(value),
                    None => return Ok(out),
                }
            }
            if seq.next_element::<de::IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("entry count bound"));
            }
            Ok(out)
        }
    }
    d.deserialize_seq(Bounded)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestWire {
    schema_version: u32,
    version: String,
    target: String,
    source_commit: String,
    #[serde(deserialize_with = "entries")]
    entries: Vec<PackageEntry>,
    manifest_sha256: String,
}
fn parse_manifest(raw: &[u8], pin: &str) -> Result<PackageManifest> {
    if raw.is_empty() || raw.len() > MAX_MANIFEST || hash(raw) != pin {
        return Err(InstallError::Invalid("raw manifest integrity or bound"));
    }
    let w: ManifestWire = serde_json::from_slice(raw)
        .map_err(|_| InstallError::Invalid("closed bounded manifest JSON"))?;
    let m = PackageManifest {
        schema_version: w.schema_version,
        version: w.version,
        target: w.target,
        source_commit: w.source_commit,
        entries: w.entries,
        manifest_sha256: w.manifest_sha256,
    };
    validate_staged_manifest(&m).map_err(|_| InstallError::Invalid("native manifest policy"))?;
    let mut canonical =
        serde_json::to_vec(&m).map_err(|_| InstallError::Invalid("manifest serialization"))?;
    canonical.push(b'\n');
    if canonical != raw {
        return Err(InstallError::Invalid(
            "manifest must match canonical producer bytes",
        ));
    }
    Ok(m)
}

struct Expanded<R> {
    inner: R,
    bytes: u64,
}
impl<R: Read> Read for Expanded<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let max = out
            .len()
            .min(MAX_ARCHIVE.saturating_sub(self.bytes).saturating_add(1) as usize);
        let n = self.inner.read(&mut out[..max])?;
        self.bytes += n as u64;
        if self.bytes > MAX_ARCHIVE {
            return Err(io::Error::other("expanded native archive bound"));
        }
        Ok(n)
    }
}
fn header(name: &[u8], size: u64, mode: u32, kind: u8) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    let n = name.len().min(100);
    h.as_gnu_mut().expect("new GNU header").name[..n].copy_from_slice(&name[..n]);
    h.set_entry_type(tar::EntryType::new(kind));
    h.set_size(size);
    h.set_mode(mode);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
    h.set_cksum();
    h
}
fn read_header(reader: &mut impl Read) -> Result<[u8; 512]> {
    let mut raw = [0; 512];
    reader.read_exact(&mut raw)?;
    Ok(raw)
}
fn zeros(reader: &mut impl Read, size: usize) -> Result<()> {
    let mut block = [0; 512];
    let mut left = size;
    while left > 0 {
        let n = left.min(block.len());
        reader.read_exact(&mut block[..n])?;
        if block[..n].iter().any(|b| *b != 0) {
            return Err(InstallError::Invalid("nonzero native tar padding"));
        }
        left -= n;
    }
    Ok(())
}
fn padding(reader: &mut impl Read, size: u64) -> Result<()> {
    zeros(reader, ((512 - size % 512) % 512) as usize)
}
fn expect_header(reader: &mut impl Read, name: &str, size: u64, mode: u32) -> Result<()> {
    if name.len() > 100 {
        let long = header(b"././@LongLink", name.len() as u64 + 1, 0o644, b'L');
        if read_header(reader)? != *long.as_bytes() {
            return Err(InstallError::Invalid(
                "bounded canonical GNU longname header",
            ));
        }
        let mut raw = vec![0; name.len() + 1];
        reader.read_exact(&mut raw)?;
        if raw[..name.len()] != *name.as_bytes() || raw[name.len()] != 0 {
            return Err(InstallError::Invalid("GNU longname differs from manifest"));
        }
        padding(reader, raw.len() as u64)?;
    }
    if read_header(reader)? != *header(name.as_bytes(), size, mode, b'0').as_bytes() {
        return Err(InstallError::Invalid(
            "canonical native tar entry/order/type/metadata",
        ));
    }
    Ok(())
}

struct Node {
    file: File,
    id: Identity,
    directory: bool,
    expected_hash: Option<String>,
    size: u64,
    mode: u32,
}
struct Stage {
    parent: File,
    parent_path: PathBuf,
    name: String,
    root: File,
    root_id: Identity,
    nodes: BTreeMap<String, Node>,
    destination: PathBuf,
    pins: (String, String),
    published: bool,
}
impl Stage {
    fn create(
        parent: &File,
        parent_path: &Path,
        destination: &Path,
        pins: (&str, &str),
    ) -> Result<Self> {
        let parent_copy = parent.try_clone()?;
        let name = format!(".admiralty-native-install-{}", uuid::Uuid::new_v4());
        rustix::fs::mkdirat(parent, name.as_str(), Mode::from_bits_truncate(0o700))?;
        let observed = rustix::fs::statat(parent, name.as_str(), AtFlags::SYMLINK_NOFOLLOW).ok();
        let uncertain = || {
            InstallError::CleanupUncertain(Box::new(InstallReconciliation {
                destination: destination.to_owned(),
                temporary_directory: parent_path.join(&name),
                staged_device: observed.as_ref().map(|s| s.st_dev.to_string()),
                staged_inode: observed.as_ref().map(|s| s.st_ino.to_string()),
                archive_sha256: pins.0.into(),
                manifest_raw_sha256: pins.1.into(),
                publication_may_have_occurred: false,
            }))
        };
        let root = directory_at(parent, &name).map_err(|_| uncertain())?;
        private_directory(&root).map_err(|_| uncertain())?;
        let root_id = identity(&root).map_err(|_| uncertain())?;
        if !observed.as_ref().is_some_and(|s| {
            i128::from(s.st_dev) == i128::from(root_id.dev)
                && i128::from(s.st_ino) == i128::from(root_id.ino)
        }) {
            return Err(uncertain());
        }
        Ok(Self {
            parent: parent_copy,
            parent_path: parent_path.to_owned(),
            name,
            root,
            root_id,
            nodes: BTreeMap::new(),
            destination: destination.to_owned(),
            pins: (pins.0.into(), pins.1.into()),
            published: false,
        })
    }
    fn info(&self) -> InstallReconciliation {
        InstallReconciliation {
            destination: self.destination.clone(),
            temporary_directory: self.parent_path.join(&self.name),
            staged_device: Some(self.root_id.dev.to_string()),
            staged_inode: Some(self.root_id.ino.to_string()),
            archive_sha256: self.pins.0.clone(),
            manifest_raw_sha256: self.pins.1.clone(),
            publication_may_have_occurred: self.published,
        }
    }
    fn parent_for(&mut self, path: &str) -> Result<(File, String)> {
        let parts: Vec<_> = path.split('/').collect();
        let mut current = self.root.try_clone()?;
        let mut prefix = String::new();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/')
            }
            prefix.push_str(part);
            if !self.nodes.contains_key(&prefix) {
                if self.nodes.len() >= MAX_NODES {
                    return Err(InstallError::Invalid("installed node bound"));
                }
                rustix::fs::mkdirat(&current, *part, Mode::from_bits_truncate(0o700))?;
                let file = directory_at(&current, part)?;
                private_directory(&file)?;
                let id = identity(&file)?;
                self.nodes.insert(
                    prefix.clone(),
                    Node {
                        file,
                        id,
                        directory: true,
                        expected_hash: None,
                        size: 0,
                        mode: 0o700,
                    },
                );
            }
            let node = self
                .nodes
                .get(&prefix)
                .ok_or(InstallError::Invalid("missing staged parent"))?;
            let named = directory_at(&current, part)?;
            if !node.directory || !same_inode(&identity(&named)?, &node.id) {
                return Err(InstallError::Invalid("staged parent identity"));
            }
            current = named;
        }
        Ok((current, parts[parts.len() - 1].to_owned()))
    }
    fn write_file(
        &mut self,
        path: &str,
        size: u64,
        mode: u32,
        digest: &str,
        input: &mut impl Read,
    ) -> Result<()> {
        if self.nodes.len() >= MAX_NODES || self.nodes.contains_key(path) {
            return Err(InstallError::Invalid("installed node collision or bound"));
        }
        let (parent, name) = self.parent_for(path)?;
        let file = File::from(rustix::fs::openat(
            &parent,
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        let id = identity(&file)?;
        self.nodes.insert(
            path.to_owned(),
            Node {
                file,
                id,
                directory: false,
                expected_hash: Some(digest.into()),
                size,
                mode,
            },
        );
        let node = self.nodes.get_mut(path).expect("inserted owned file");
        let mut left = size;
        let mut buffer = [0; CHUNK];
        let mut sha = Sha256::new();
        while left > 0 {
            let n = (left as usize).min(buffer.len());
            input.read_exact(&mut buffer[..n])?;
            node.file.write_all(&buffer[..n])?;
            sha.update(&buffer[..n]);
            left -= n as u64;
        }
        if format!("{:x}", sha.finalize()) != digest {
            return Err(InstallError::Invalid(
                "installed content differs from manifest",
            ));
        }
        let mode = match mode {
            0o644 => Mode::from_bits_truncate(0o644),
            0o755 => Mode::from_bits_truncate(0o755),
            _ => return Err(InstallError::Invalid("native file mode")),
        };
        node.file.flush()?;
        rustix::fs::fchmod(&node.file, mode)?;
        node.file.sync_all()?;
        node.id = identity(&node.file)?;
        Ok(())
    }
    fn named(&self, path: &str, directory: bool) -> Result<File> {
        let mut fd = self.root.try_clone()?;
        let mut parts = path.split('/').peekable();
        while let Some(name) = parts.next() {
            fd = if parts.peek().is_some() || directory {
                directory_at(&fd, name)?
            } else {
                named_regular(&fd, name)?
            };
        }
        Ok(fd)
    }
    fn inventory(&self) -> Result<()> {
        fn walk(stage: &Stage, fd: &File, prefix: &str, seen: &mut usize) -> Result<()> {
            let mut entries = rustix::fs::Dir::read_from(fd)?;
            for item in &mut entries {
                let item = item?;
                let name = item
                    .file_name()
                    .to_str()
                    .map_err(|_| InstallError::Invalid("unknown installed entry"))?;
                if matches!(name, "." | "..") {
                    continue;
                }
                if name.len() > 255 || *seen >= stage.nodes.len() {
                    return Err(InstallError::Invalid("installed inventory bound"));
                }
                let path = if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}/{name}")
                };
                let node = stage
                    .nodes
                    .get(&path)
                    .ok_or(InstallError::Invalid("unknown installed entry"))?;
                let named = stage.named(&path, node.directory)?;
                if !same_inode(&identity(&named)?, &node.id) {
                    return Err(InstallError::Invalid("installed entry replaced"));
                }
                *seen += 1;
                if node.directory {
                    walk(stage, &named, &path, seen)?;
                }
            }
            Ok(())
        }
        let mut seen = 0;
        walk(self, &self.root, "", &mut seen)?;
        if seen != self.nodes.len() {
            return Err(InstallError::Invalid("installed entry missing"));
        }
        Ok(())
    }
    fn verify(&mut self, tree: &BTreeMap<String, bool>) -> Result<()> {
        if self.nodes.len() != tree.len()
            || self
                .nodes
                .iter()
                .any(|(p, n)| tree.get(p) != Some(&n.directory))
        {
            return Err(InstallError::Invalid(
                "installed tree differs from manifest",
            ));
        }
        private_directory(&self.root)?;
        self.inventory()?;
        for node in self.nodes.values_mut() {
            if node.directory {
                private_directory(&node.file)?;
                node.id = identity(&node.file)?;
            } else {
                if identity(&node.file)? != node.id
                    || node.id.mode & 0o7777 != node.mode
                    || node.id.links != 1
                    || node.id.uid != rustix::process::geteuid().as_raw()
                    || hash_file(&mut node.file, node.size, MAX_ARCHIVE)?
                        != *node.expected_hash.as_ref().expect("file digest")
                {
                    return Err(InstallError::Invalid(
                        "installed bytes, mode or identity changed",
                    ));
                }
            }
        }
        self.root_id = identity(&self.root)?;
        Ok(())
    }
    fn sync_directories(&self) -> Result<()> {
        for node in self.nodes.values().rev() {
            if node.directory {
                node.file.sync_all()?
            }
        }
        self.root.sync_all()?;
        Ok(())
    }
    fn cleanup(&self) -> Result<()> {
        self.inventory()?;
        let named = directory_at(&self.parent, &self.name)?;
        if !same_inode(&identity(&named)?, &self.root_id) {
            return Err(InstallError::Invalid("temporary root replaced"));
        }
        for (path, node) in self.nodes.iter().rev() {
            let (parent, name) = match path.rsplit_once('/') {
                Some((p, n)) => (
                    &self
                        .nodes
                        .get(p)
                        .ok_or(InstallError::Invalid("cleanup parent missing"))?
                        .file,
                    n,
                ),
                None => (&self.root, path.as_str()),
            };
            let named = self.named(path, node.directory)?;
            if !same_inode(&identity(&named)?, &node.id) {
                return Err(InstallError::Invalid("cleanup identity changed"));
            }
            rustix::fs::unlinkat(
                parent,
                name,
                if node.directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                },
            )?;
        }
        rustix::fs::unlinkat(&self.parent, self.name.as_str(), AtFlags::REMOVEDIR)?;
        self.parent.sync_all()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Point {
    ArchivePinned,
    StageCreated(PathBuf),
    FileWritten(usize),
    BeforeCommit,
    AfterCommit,
    BeforeParentSync,
    BeforeAck,
}

pub fn install_native_package(
    archive: &Path,
    expected_archive_sha256: &str,
    expected_manifest_raw_sha256: &str,
    destination: &Path,
) -> Result<InstallReceipt> {
    install_with_hook(
        archive,
        expected_archive_sha256,
        expected_manifest_raw_sha256,
        destination,
        |_| Ok(()),
    )
}
fn install_with_hook(
    archive: &Path,
    archive_pin: &str,
    manifest_pin: &str,
    destination: &Path,
    mut hook: impl FnMut(Point) -> io::Result<()>,
) -> Result<InstallReceipt> {
    if !valid_hash(archive_pin) || !valid_hash(manifest_pin) {
        return Err(InstallError::Invalid("SHA256 pins must be64lowercasehex"));
    }
    let parent_path = destination
        .parent()
        .ok_or(InstallError::Invalid("destination parent"))?;
    let name = destination
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(InstallError::Invalid("destination name"))?;
    if !portable(name) || name.contains('/') {
        return Err(InstallError::Invalid("destination name bound"));
    }
    let parent = absolute_directory(parent_path)?;
    private_directory(&parent)?;
    match rustix::fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => return Err(InstallError::Invalid("destination already exists")),
        Err(rustix::io::Errno::NOENT) => {}
        Err(e) => return Err(e.into()),
    }
    let archive_parent_path = archive
        .parent()
        .ok_or(InstallError::Invalid("archive parent"))?;
    let archive_parent = absolute_directory(archive_parent_path)?;
    let archive_name = archive
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(InstallError::Invalid("archive name"))?;
    let mut source = named_regular(&archive_parent, archive_name)?;
    let source_id = identity(&source)?;
    if source_id.size < 18
        || source_id.size > MAX_ARCHIVE
        || hash_file(&mut source, source_id.size, MAX_ARCHIVE)? != archive_pin
    {
        return Err(InstallError::Invalid("archive integrity or byte bound"));
    }
    source.seek(SeekFrom::Start(0))?;
    let mut gz_header = [0; 10];
    source.read_exact(&mut gz_header)?;
    if gz_header != GZIP_HEADER {
        return Err(InstallError::Invalid("canonical single native gzip header"));
    }
    source.seek(SeekFrom::Start(0))?;
    hook(Point::ArchivePinned)?;
    let gzip = GzDecoder::new(BufReader::with_capacity(CHUNK, source.try_clone()?));
    let mut decoded = Expanded {
        inner: gzip,
        bytes: 0,
    };
    let raw_header = read_header(&mut decoded)?;
    let h = tar::Header::from_byte_slice(&raw_header);
    let size = h
        .entry_size()
        .map_err(|_| InstallError::Invalid("manifest header size"))?;
    if size == 0
        || size > MAX_MANIFEST as u64
        || raw_header != *header(MANIFEST.as_bytes(), size, 0o644, b'0').as_bytes()
    {
        return Err(InstallError::Invalid("first canonical manifest header"));
    }
    let mut raw = vec![0; size as usize];
    decoded.read_exact(&mut raw)?;
    padding(&mut decoded, size)?;
    let manifest = parse_manifest(&raw, manifest_pin)?;
    let tree = expected_tree(&manifest)
        .map_err(|_| InstallError::Invalid("native portable tree policy"))?;
    let mut stage = Stage::create(
        &parent,
        parent_path,
        destination,
        (archive_pin, manifest_pin),
    )?;
    let work = (|| -> Result<InstallReceipt> {
        hook(Point::StageCreated(stage.parent_path.join(&stage.name)))?;
        stage.write_file(MANIFEST, size, 0o644, manifest_pin, &mut raw.as_slice())?;
        for (index, e) in manifest.entries.iter().enumerate() {
            expect_header(&mut decoded, &e.archive_path, e.size, e.mode)?;
            stage.write_file(&e.archive_path, e.size, e.mode, &e.sha256, &mut decoded)?;
            padding(&mut decoded, e.size)?;
            hook(Point::FileWritten(index))?;
        }
        zeros(&mut decoded, 1024)?;
        let mut byte = [0];
        if decoded.read(&mut byte)? != 0 {
            return Err(InstallError::Invalid("extra expanded tar bytes"));
        }
        let mut buffered = decoded.inner.into_inner();
        if !buffered.fill_buf()?.is_empty() {
            return Err(InstallError::Invalid(
                "trailing or concatenated compressed bytes",
            ));
        }
        drop(buffered);
        stage.verify(&tree)?;
        {
            let mut files: Vec<_> = stage
                .nodes
                .iter_mut()
                .filter_map(|(p, n)| (!n.directory && p != MANIFEST).then_some(&mut n.file))
                .collect();
            validate_staged_guard_closure(&manifest, &mut files)
                .map_err(|_| InstallError::Invalid("native generated guard closure"))?;
        }
        stage.verify(&tree)?;
        stage.sync_directories()?;
        if identity(&source)? != source_id
            || hash_file(&mut source, source_id.size, MAX_ARCHIVE)? != archive_pin
            || identity(&named_regular(&archive_parent, archive_name)?)? != source_id
            || !same_inode(
                &identity(&absolute_directory(archive_parent_path)?)?,
                &identity(&archive_parent)?,
            )
        {
            return Err(InstallError::Invalid("archive changed before commit"));
        }
        hook(Point::BeforeCommit)?;
        stage.verify(&tree)?;
        private_directory(&parent)?;
        if identity(&source)? != source_id
            || hash_file(&mut source, source_id.size, MAX_ARCHIVE)? != archive_pin
            || identity(&named_regular(&archive_parent, archive_name)?)? != source_id
            || !same_inode(
                &identity(&absolute_directory(archive_parent_path)?)?,
                &identity(&archive_parent)?,
            )
        {
            return Err(InstallError::Invalid("archive changed at commit boundary"));
        }
        if !same_inode(
            &identity(&absolute_directory(parent_path)?)?,
            &identity(&parent)?,
        ) || !same_inode(
            &identity(&directory_at(&parent, &stage.name)?)?,
            &stage.root_id,
        ) {
            return Err(InstallError::Invalid("install parent or stage replaced"));
        }
        rustix::fs::renameat_with(
            &parent,
            stage.name.as_str(),
            &parent,
            name,
            RenameFlags::NOREPLACE,
        )?;
        stage.published = true;
        hook(Point::AfterCommit)?;
        hook(Point::BeforeParentSync)?;
        parent.sync_all()?;
        hook(Point::BeforeAck)?;
        private_directory(&parent)?;
        private_directory(&stage.root)?;
        let installed = directory_at(&parent, name)?;
        if !same_inode(&identity(&installed)?, &stage.root_id)
            || !same_inode(
                &identity(&absolute_directory(parent_path)?)?,
                &identity(&parent)?,
            )
        {
            return Err(InstallError::Invalid("installed named identity changed"));
        }
        stage.verify(&tree)?;
        if hash_file(&mut source, source_id.size, MAX_ARCHIVE)? != archive_pin
            || identity(&source)? != source_id
            || identity(&named_regular(&archive_parent, archive_name)?)? != source_id
            || !same_inode(
                &identity(&absolute_directory(archive_parent_path)?)?,
                &identity(&archive_parent)?,
            )
        {
            return Err(InstallError::Invalid(
                "source changed before acknowledgement",
            ));
        }
        Ok(InstallReceipt {
            destination: destination.to_owned(),
            version: manifest.version.clone(),
            target: manifest.target.clone(),
            source_commit_label: manifest.source_commit.clone(),
            archive_sha256: archive_pin.into(),
            archive_bytes: source_id.size,
            manifest_raw_sha256: manifest_pin.into(),
            unsigned_manifest_sha256: manifest.manifest_sha256.clone(),
            installed_file_count: manifest.entries.len() as u32 + 1,
            installed_payload_bytes: manifest.entries.iter().map(|e| e.size).sum(),
            installed_device: stage.root_id.dev.to_string(),
            installed_inode: stage.root_id.ino.to_string(),
            installation_scope: "private_version_directory_only",
            installed_bytes: true,
            installer_complete: false,
            daemon_running: false,
            autostart: false,
            profile_changed: false,
            path_changed: false,
            publisher_authenticated: false,
            source_commit_verified: false,
            signed: false,
            published: false,
            execution_granted: false,
            acceptance_granted: false,
            native_execution_qualified: false,
            activated: false,
        })
    })();
    match work {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            if stage.published {
                Err(InstallError::CommitUncertain(Box::new(stage.info())))
            } else if stage.cleanup().is_err() {
                Err(InstallError::CleanupUncertain(Box::new(stage.info())))
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_archive::archive_native_package;
    use crate::native_package::{PackageInput, stage_native_package};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    struct Fixture {
        root: tempfile::TempDir,
        archive: PathBuf,
        destination: PathBuf,
        manifest: PackageManifest,
        archive_hash: String,
        manifest_hash: String,
    }
    fn fixture(long: bool) -> Fixture {
        let root = tempfile::Builder::new()
            .prefix("ni-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize().expect("canonical tmp"))
            .expect("private fixture");
        let mut sources=vec![
            ("target/release/admiralty".to_owned(),b"not executable provider code".to_vec(),0o755),
            ("apps/dashboard/dist/index.html".to_owned(),b"<main>installed synthetic bytes</main>".to_vec(),0o644),
            ("schemas/domain-event-v2.schema.json".to_owned(),include_bytes!("../../../schemas/domain-event-v2.schema.json").to_vec(),0o644),
            ("schemas/guards/v2/fixture.mjs".to_owned(),b"throw new Error('INSTALL_MUST_NOT_EVALUATE'); export const validateDomainEventV2=()=>false;".to_vec(),0o644),
            ("docs/operators/release.md".to_owned(),b"owned installation fixture".to_vec(),0o644),
        ];
        if long {
            sources.push((
                format!("docs/{}.md", "a".repeat(200)),
                b"long native GNU path".to_vec(),
                0o644,
            ));
        }
        let mut inputs = vec![];
        for (name, bytes, mode) in sources {
            let p = root.path().join(&name);
            fs::create_dir_all(p.parent().expect("parent")).expect("source dirs");
            fs::write(p, &bytes).expect("source data");
            inputs.push(PackageInput {
                source: name,
                expected_sha256: hash(&bytes),
                mode,
            });
        }
        let staged = stage_native_package(
            root.path(),
            &root.path().join("stage"),
            "2.0.0",
            "fixture-unix",
            &"a".repeat(40),
            &inputs,
        )
        .expect("actual Rust stage");
        let manifest_hash =
            hash(&fs::read(staged.output_dir.join(MANIFEST)).expect("raw manifest"));
        let archive = root.path().join("bundle.tar.gz");
        let archived = archive_native_package(
            &staged.output_dir,
            &staged.manifest,
            &manifest_hash,
            &archive,
        )
        .expect("actual Rust archive");
        Fixture {
            destination: root.path().join("admiralty-2.0.0-private"),
            root,
            archive,
            manifest: staged.manifest,
            archive_hash: archived.archive_sha256,
            manifest_hash,
        }
    }
    fn install(f: &Fixture) -> Result<InstallReceipt> {
        install_native_package(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
        )
    }
    fn no_temporary(f: &Fixture) {
        assert!(
            !fs::read_dir(f.root.path())
                .expect("private parent")
                .any(|e| e
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".admiralty-native-install-"))
        );
    }
    #[test]
    fn actual_native_stage_archive_install_roundtrip_preserves_bytes_modes_and_false_authority() {
        let f = fixture(false);
        let receipt = install(&f).expect("private installed bytes");
        assert!(receipt.installed_bytes);
        assert!(
            !receipt.installer_complete
                && !receipt.daemon_running
                && !receipt.publisher_authenticated
                && !receipt.execution_granted
                && !receipt.acceptance_granted
                && !receipt.activated
        );
        assert_eq!(receipt.archive_sha256, f.archive_hash);
        assert_eq!(receipt.manifest_raw_sha256, f.manifest_hash);
        assert_eq!(
            fs::metadata(&f.destination)
                .expect("private version")
                .mode()
                & 0o7777,
            0o700
        );
        for e in &f.manifest.entries {
            let p = f.destination.join(&e.archive_path);
            assert_eq!(hash(&fs::read(&p).expect("installed content")), e.sha256);
            assert_eq!(
                fs::metadata(p).expect("installed mode").mode() & 0o7777,
                e.mode
            );
        }
        assert_eq!(
            hash(&fs::read(f.destination.join(MANIFEST)).expect("manifest")),
            f.manifest_hash
        );
        no_temporary(&f);
    }
    #[test]
    fn bounded_native_gnu_longname_roundtrips_without_generic_extraction() {
        let f = fixture(true);
        install(&f).expect("actual long native archive installs");
        assert!(
            f.destination
                .join(format!("share/doc/admiralty/{}.md", "a".repeat(200)))
                .is_file()
        );
    }
    #[test]
    fn wrong_integrity_pins_create_no_stage_or_destination() {
        let f = fixture(false);
        for (a, m) in [
            ("0".repeat(64), f.manifest_hash.clone()),
            (f.archive_hash.clone(), "0".repeat(64)),
            ("bad".into(), f.manifest_hash.clone()),
        ] {
            assert!(install_native_package(&f.archive, &a, &m, &f.destination).is_err());
            assert!(!f.destination.exists());
            no_temporary(&f);
        }
    }
    #[test]
    fn existing_file_directory_or_symlink_destination_is_never_replaced() {
        for kind in 0..3 {
            let f = fixture(false);
            match kind {
                0 => fs::write(&f.destination, b"KEEP").expect("existing file"),
                1 => fs::create_dir(&f.destination).expect("existing dir"),
                _ => symlink(&f.archive, &f.destination).expect("existing link"),
            };
            let before = fs::symlink_metadata(&f.destination).expect("existing identity");
            assert!(install(&f).is_err());
            let after = fs::symlink_metadata(&f.destination).expect("preserved");
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
            if kind == 0 {
                assert_eq!(fs::read(&f.destination).expect("kept bytes"), b"KEEP");
            }
            no_temporary(&f);
        }
    }
    fn unpack_test_tar(f: &Fixture) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(File::open(&f.archive).expect("test archive"))
            .take(4 * 1024 * 1024)
            .read_to_end(&mut out)
            .expect("test-only decode");
        out
    }
    fn replace_archive(f: &mut Fixture, bytes: &[u8]) {
        fs::write(&f.archive, bytes).expect("owned malicious archive");
        f.archive_hash = hash(bytes);
    }
    fn replace_tar(f: &mut Fixture, tar: &[u8]) {
        let mut gz = flate2::GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            .write(Vec::new(), flate2::Compression::best());
        gz.write_all(tar).expect("test compress");
        replace_archive(f, &gz.finish().expect("test gzip"));
    }
    fn rejected(f: &Fixture, label: &str) {
        assert!(install(f).is_err(), "must refuse {label}");
        assert!(!f.destination.exists(), "no destination for {label}");
        no_temporary(f);
    }
    fn append_test_tar(tar: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, bytes: &[u8]) {
        let mut h = header(path.as_bytes(), bytes.len() as u64, mode, b'0');
        tar.append_data(&mut h, path, bytes).expect("test frame");
    }
    fn rewrite_manifest(
        f: &mut Fixture,
        manifest: &PackageManifest,
        raw: Vec<u8>,
        payloads: &BTreeMap<String, Vec<u8>>,
    ) {
        let mut tar = tar::Builder::new(Vec::new());
        append_test_tar(&mut tar, MANIFEST, 0o644, &raw);
        for e in &manifest.entries {
            let body = payloads
                .get(&e.archive_path)
                .map(Vec::as_slice)
                .unwrap_or(b"");
            append_test_tar(&mut tar, &e.archive_path, e.mode, body);
        }
        f.manifest_hash = hash(&raw);
        replace_tar(f, &tar.into_inner().expect("test finish"));
    }
    fn payloads(f: &Fixture) -> BTreeMap<String, Vec<u8>> {
        f.manifest
            .entries
            .iter()
            .map(|e| {
                (
                    e.archive_path.clone(),
                    fs::read(f.root.path().join("stage").join(&e.archive_path))
                        .expect("original payload"),
                )
            })
            .collect()
    }
    fn resign(m: &mut PackageManifest) -> Vec<u8> {
        #[derive(Serialize)]
        struct Unsigned<'a> {
            schema_version: u32,
            version: &'a str,
            target: &'a str,
            source_commit: &'a str,
            entries: &'a [PackageEntry],
        }
        m.manifest_sha256 = admiralty_core::content_digest(&Unsigned {
            schema_version: m.schema_version,
            version: &m.version,
            target: &m.target,
            source_commit: &m.source_commit,
            entries: &m.entries,
        })
        .expect("test unsigned digest");
        let mut raw = serde_json::to_vec(m).expect("test canonical");
        raw.push(b'\n');
        raw
    }
    #[test]
    fn single_canonical_gzip_crc_and_compressed_eof_are_required() {
        let mut f = fixture(false);
        let original = fs::read(&f.archive).expect("gzip");
        for label in [
            "optional-header",
            "compression-header",
            "crc",
            "truncated",
            "concatenated",
            "trailing",
        ] {
            let mut bad = original.clone();
            match label {
                "optional-header" => bad[3] = 8,
                "compression-header" => bad[8] = 0,
                "crc" => {
                    let i = bad.len() - 8;
                    bad[i] ^= 1
                }
                "truncated" => bad.truncate(bad.len() - 1),
                "concatenated" => bad.extend_from_slice(&original),
                _ => bad.push(0),
            }
            replace_archive(&mut f, &bad);
            rejected(&f, label);
        }
    }
    #[test]
    fn tar_special_types_paths_modes_and_metadata_are_never_extracted() {
        let mut f = fixture(false);
        let original = unpack_test_tar(&f);
        let first = 512
            + fs::read(f.root.path().join("stage/MANIFEST.json"))
                .expect("raw")
                .len()
                .div_ceil(512)
                * 512;
        for kind in [b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'x', b'g', b'S'] {
            let mut bad = original.clone();
            let mut h = tar::Header::from_byte_slice(&bad[first..first + 512]).clone();
            h.set_entry_type(tar::EntryType::new(kind));
            h.set_cksum();
            bad[first..first + 512].copy_from_slice(h.as_bytes());
            replace_tar(&mut f, &bad);
            rejected(&f, "link/device/FIFO/PAX/sparse/other type");
        }
        for mode in [0o777, 0o4755, 0o644] {
            let mut bad = original.clone();
            let mut h = tar::Header::from_byte_slice(&bad[first..first + 512]).clone();
            h.set_mode(mode);
            h.set_cksum();
            bad[first..first + 512].copy_from_slice(h.as_bytes());
            replace_tar(&mut f, &bad);
            rejected(&f, "declared executable mode mismatch");
        }
        for path in [
            "../outside",
            "/absolute",
            "bin/admiralty/child",
            "bin/other",
        ] {
            let mut bad = original.clone();
            let h = header(
                path.as_bytes(),
                f.manifest.entries[0].size,
                f.manifest.entries[0].mode,
                b'0',
            );
            bad[first..first + 512].copy_from_slice(h.as_bytes());
            replace_tar(&mut f, &bad);
            rejected(&f, "undeclared or traversal path");
        }
        let mut bad = original.clone();
        let mut h = tar::Header::from_byte_slice(&bad[first..first + 512]).clone();
        h.set_mtime(1);
        h.set_cksum();
        bad[first..first + 512].copy_from_slice(h.as_bytes());
        replace_tar(&mut f, &bad);
        rejected(&f, "noncanonical metadata");
    }
    #[test]
    fn exact_content_zero_padding_order_inventory_and_tar_end_are_required() {
        let mut f = fixture(false);
        let original = unpack_test_tar(&f);
        let first = 512
            + fs::read(f.root.path().join("stage/MANIFEST.json"))
                .expect("raw")
                .len()
                .div_ceil(512)
                * 512;
        let end = first + 512 + (f.manifest.entries[0].size as usize).div_ceil(512) * 512;
        for label in [
            "content",
            "padding",
            "duplicate",
            "missing",
            "extra",
            "nonzero-end",
            "excess-zero-end",
        ] {
            let mut bad = original.clone();
            match label {
                "content" => bad[first + 512] ^= 1,
                "padding" => bad[first + 512 + f.manifest.entries[0].size as usize] = 1,
                "duplicate" => {
                    bad.splice(end..end, bad[first..end].to_vec());
                }
                "missing" => {
                    bad.drain(first..end);
                }
                "extra" => {
                    let i = bad.len() - 1024;
                    bad.splice(
                        i..i,
                        header(b"unknown", 0, 0o644, b'0')
                            .as_bytes()
                            .iter()
                            .copied(),
                    );
                }
                "nonzero-end" => {
                    let i = bad.len() - 1;
                    bad[i] = 1
                }
                _ => bad.extend_from_slice(&[0; 512]),
            }
            replace_tar(&mut f, &bad);
            rejected(&f, label);
        }
    }
    #[test]
    fn bounded_gnu_longname_must_exactly_match_manifest_before_payload() {
        let mut f = fixture(true);
        let original = unpack_test_tar(&f);
        let offset = original
            .windows(13)
            .position(|w| w == b"././@LongLink")
            .expect("actual GNU record");
        for label in ["name", "nul", "size", "kind"] {
            let mut bad = original.clone();
            match label {
                "name" => bad[offset + 512] ^= 1,
                "nul" => {
                    let n = tar::Header::from_byte_slice(&bad[offset..offset + 512])
                        .entry_size()
                        .expect("GNU size") as usize;
                    bad[offset + 512 + n - 1] = b'x'
                }
                _ => {
                    let mut h = tar::Header::from_byte_slice(&bad[offset..offset + 512]).clone();
                    if label == "size" {
                        h.set_size(MAX_MANIFEST as u64 + 1)
                    } else {
                        h.set_entry_type(tar::EntryType::XHeader)
                    }
                    h.set_cksum();
                    bad[offset..offset + 512].copy_from_slice(h.as_bytes());
                }
            }
            replace_tar(&mut f, &bad);
            rejected(&f, label);
        }
    }
    #[test]
    fn manifest_unknown_duplicate_noncanonical_and_oversized_inputs_fail_before_stage() {
        let mut f = fixture(false);
        let data = payloads(&f);
        let m = f.manifest.clone();
        let canonical = fs::read(f.root.path().join("stage/MANIFEST.json")).expect("raw");
        let original = String::from_utf8(canonical.clone()).expect("JSON");
        let cases = [
            original.replacen('{', "{\"unknown\":true,", 1).into_bytes(),
            original
                .replacen('{', "{\"schema_version\":1,", 1)
                .into_bytes(),
            original
                .replacen("\"archive_path\":", "\"extra\":false,\"archive_path\":", 1)
                .into_bytes(),
            original.trim_end().as_bytes().to_vec(),
            format!(" {original}").into_bytes(),
            vec![b'x'; MAX_MANIFEST + 1],
        ];
        for raw in cases {
            rewrite_manifest(&mut f, &m, raw, &data);
            rejected(&f, "closed canonical byte-bounded manifest");
        }
        let mut value = serde_json::to_value(&m).expect("test value");
        value["entries"] =
            serde_json::Value::Array(vec![value["entries"][0].clone(); MAX_FILES + 1]);
        let raw = serde_json::to_vec(&value).expect("overcount JSON");
        rewrite_manifest(&mut f, &m, raw, &data);
        rejected(&f, "entry count");
    }
    #[test]
    fn matching_operator_pins_do_not_authorize_wrong_shape_collision_or_size() {
        let mut f = fixture(false);
        let data = payloads(&f);
        let original = f.manifest.clone();
        for label in [
            "traversal",
            "case-collision",
            "ancestor-collision",
            "unknown-kind",
            "file-size",
            "total-size",
            "schema",
        ] {
            let mut m = original.clone();
            match label {
                "traversal" => m.entries[0].archive_path = "../outside".into(),
                "case-collision" => {
                    let mut e = m.entries[0].clone();
                    e.archive_path = e.archive_path.to_ascii_uppercase();
                    m.entries.push(e)
                }
                "ancestor-collision" => {
                    let mut e = m.entries[0].clone();
                    e.archive_path.push_str("/nested");
                    m.entries.push(e)
                }
                "unknown-kind" => m.entries[0].archive_path = "private/auth.json".into(),
                "file-size" => m.entries[0].size = 512 * 1024 * 1024 + 1,
                "total-size" => {
                    for e in m.entries.iter_mut().take(3) {
                        e.size = 512 * 1024 * 1024
                    }
                }
                _ => m.schema_version = 2,
            };
            m.entries
                .sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
            let raw = resign(&mut m);
            // Use original frames: manifest policy must deny before payload interpretation.
            rewrite_manifest(&mut f, &original, raw, &data);
            rejected(&f, label);
        }
    }
    #[test]
    fn exact_guard_dependency_policy_is_reused_without_running_guard_code() {
        let mut f = fixture(false);
        let original = f.manifest.clone();
        let guard = "share/admiralty/guards/v2/fixture.mjs";
        for label in [
            "dynamic-custom",
            "generated-missing-sibling",
            "changed-generated-leaf",
        ] {
            let mut m = original.clone();
            let mut data = payloads(&f);
            let e = m
                .entries
                .iter_mut()
                .find(|e| e.archive_path == guard)
                .expect("custom guard");
            data.remove(guard);
            let bytes = match label {
                "dynamic-custom" => {
                    b"export const validateDomainEventV2=()=>import('./hidden.mjs');".to_vec()
                }
                "generated-missing-sibling" => {
                    e.archive_path = "share/admiralty/guards/v2/validators.mjs".into();
                    include_bytes!("../../../packages/contracts/generated/validators.mjs").to_vec()
                }
                _ => {
                    e.archive_path = "share/admiralty/guards/v2/validators-base.mjs".into();
                    let mut b =
                        include_bytes!("../../../packages/contracts/generated/validators-base.mjs")
                            .to_vec();
                    b.push(b' ');
                    b
                }
            };
            e.sha256 = hash(&bytes);
            e.size = bytes.len() as u64;
            data.insert(e.archive_path.clone(), bytes);
            m.entries
                .sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
            let raw = resign(&mut m);
            rewrite_manifest(&mut f, &m, raw, &data);
            rejected(&f, label);
        }
    }
    #[test]
    fn source_mutation_and_replacement_are_refused_with_original_name_preserved() {
        for at in [Point::ArchivePinned, Point::BeforeCommit, Point::BeforeAck] {
            let f = fixture(false);
            let mut changed = false;
            let outcome = install_with_hook(
                &f.archive,
                &f.archive_hash,
                &f.manifest_hash,
                &f.destination,
                |p| {
                    if p == at {
                        let mut b = fs::read(&f.archive)?;
                        b[12] ^= 1;
                        fs::write(&f.archive, b)?;
                        changed = true;
                    }
                    Ok(())
                },
            );
            assert!(changed && outcome.is_err());
            assert!(f.archive.is_file());
            if at == Point::BeforeAck {
                assert!(matches!(outcome, Err(InstallError::CommitUncertain(_))));
                assert!(f.destination.is_dir());
            } else {
                assert!(!f.destination.exists());
                no_temporary(&f);
            }
        }
        let f = fixture(false);
        let saved = f.root.path().join("original-pinned.gz");
        let outcome = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if p == Point::BeforeCommit {
                    fs::rename(&f.archive, &saved)?;
                    fs::copy(&saved, &f.archive)?;
                }
                Ok(())
            },
        );
        assert!(outcome.is_err());
        assert!(!f.destination.exists());
        no_temporary(&f);
        assert_eq!(
            hash(&fs::read(saved).expect("original source retained")),
            f.archive_hash
        );
    }
    #[test]
    fn staged_payload_mutation_and_ancestor_replacement_cannot_be_acknowledged() {
        for replace in [false, true] {
            let f = fixture(false);
            let mut stage = None;
            let mut mutated = false;
            let result = install_with_hook(
                &f.archive,
                &f.archive_hash,
                &f.manifest_hash,
                &f.destination,
                |p| {
                    if let Point::StageCreated(path) = &p {
                        stage = Some(path.clone());
                    }
                    if p == Point::BeforeCommit {
                        let path = stage.as_ref().expect("stage path").join("bin/admiralty");
                        if replace {
                            fs::rename(
                                path.parent().expect("parent"),
                                stage.as_ref().expect("root").join("replaced-bin"),
                            )?;
                            fs::create_dir(path.parent().expect("parent"))?;
                            fs::write(path, b"foreign")?;
                        } else {
                            fs::write(path, b"same owned inode changed")?;
                        }
                        mutated = true;
                    }
                    Ok(())
                },
            );
            assert!(mutated && result.is_err());
            assert!(!f.destination.exists());
            if replace {
                assert!(matches!(result, Err(InstallError::CleanupUncertain(_))));
                let root = stage.expect("preserved");
                assert_eq!(
                    fs::read(root.join("bin/admiralty")).expect("foreign preserved"),
                    b"foreign"
                );
                assert!(root.join("replaced-bin/admiralty").is_file());
            } else {
                no_temporary(&f);
            }
        }
    }
    #[test]
    fn only_known_created_entries_are_cleaned_and_unknown_entry_is_preserved() {
        let f = fixture(false);
        let failure = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if p == Point::FileWritten(0) {
                    Err(io::Error::other("injected before publication"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(failure.is_err());
        assert!(!f.destination.exists());
        no_temporary(&f);
        let f = fixture(false);
        let mut location = None;
        let result = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if let Point::StageCreated(path) = p {
                    fs::write(path.join("unrecognized-evidence"), b"KEEP")?;
                    location = Some(path);
                    return Err(io::Error::other("injected unknown entry"));
                }
                Ok(())
            },
        );
        let err = result.expect_err("unknown tree must preserve");
        assert!(matches!(err, InstallError::CleanupUncertain(_)));
        assert!(
            !err.reconciliation()
                .expect("reconciliation")
                .publication_may_have_occurred
        );
        assert_eq!(
            fs::read(location.expect("actual temp").join("unrecognized-evidence"))
                .expect("never deleted"),
            b"KEEP"
        );
    }
    #[test]
    fn no_replace_race_preserves_foreign_destination_and_cleans_only_own_stage() {
        let f = fixture(false);
        let outcome = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if p == Point::BeforeCommit {
                    fs::create_dir(&f.destination)?;
                    fs::write(f.destination.join("foreign"), b"KEEP")?;
                }
                Ok(())
            },
        );
        assert!(outcome.is_err());
        assert_eq!(
            fs::read(f.destination.join("foreign")).expect("foreign data"),
            b"KEEP"
        );
        no_temporary(&f);
    }
    #[test]
    fn post_rename_or_sync_acknowledgement_failure_preserves_identity_and_refuses_retry() {
        for at in [
            Point::AfterCommit,
            Point::BeforeParentSync,
            Point::BeforeAck,
        ] {
            let f = fixture(false);
            let outcome = install_with_hook(
                &f.archive,
                &f.archive_hash,
                &f.manifest_hash,
                &f.destination,
                |p| {
                    if p == at {
                        Err(io::Error::other("injected lost durable acknowledgement"))
                    } else {
                        Ok(())
                    }
                },
            );
            let err = outcome.expect_err("uncertain commit");
            assert!(matches!(err, InstallError::CommitUncertain(_)));
            let info = err.reconciliation().expect("reconcile");
            assert!(info.publication_may_have_occurred);
            let id = fs::metadata(&f.destination).expect("preserved version");
            assert_eq!(info.staged_device, Some(id.dev().to_string()));
            assert_eq!(info.staged_inode, Some(id.ino().to_string()));
            let raw = fs::read(f.destination.join(MANIFEST)).expect("original installed evidence");
            assert_eq!(hash(&raw), f.manifest_hash);
            assert!(install(&f).is_err());
            assert_eq!(
                fs::read(f.destination.join(MANIFEST)).expect("never overwritten"),
                raw
            );
            assert_eq!(
                hash(&fs::read(&f.archive).expect("archive remains")),
                f.archive_hash
            );
        }
    }
    fn nonregular(path: &Path) -> Option<std::os::unix::net::UnixListener> {
        #[cfg(target_os = "linux")]
        {
            rustix::fs::mkfifoat(rustix::fs::CWD, path, Mode::from_bits_truncate(0o600))
                .expect("owned FIFO");
            None
        }
        #[cfg(not(target_os = "linux"))]
        {
            Some(std::os::unix::net::UnixListener::bind(path).expect("owned nonregular socket"))
        }
    }
    #[test]
    fn source_symlink_hardlink_fifo_and_nonprivate_destination_are_refused() {
        for label in ["symlink", "hardlink", "fifo", "parent-mode"] {
            let f = fixture(false);
            let input = f.root.path().join("unsafe-input.gz");
            let _nonregular = match label {
                "symlink" => {
                    symlink(&f.archive, &input).expect("test symlink");
                    None
                }
                "hardlink" => {
                    fs::hard_link(&f.archive, &input).expect("test hardlink");
                    None
                }
                "fifo" => nonregular(&input),
                _ => {
                    fs::set_permissions(f.root.path(), fs::Permissions::from_mode(0o755))
                        .expect("nonprivate test parent");
                    None
                }
            };
            let input = if label == "parent-mode" {
                &f.archive
            } else {
                &input
            };
            assert!(
                install_native_package(input, &f.archive_hash, &f.manifest_hash, &f.destination)
                    .is_err()
            );
            assert!(!f.destination.exists());
            no_temporary(&f);
        }
    }
    #[test]
    fn expanded_byte_budget_refuses_before_additional_allocation() {
        let mut reader = Expanded {
            inner: &b"xx"[..],
            bytes: MAX_ARCHIVE,
        };
        assert!(reader.read(&mut [0; 2]).is_err());
        assert!(reader.read(&mut [0; 2]).is_err());
    }
    #[test]
    fn replacement_temporary_name_is_preserved_instead_of_deleting_foreign_tree() {
        let f = fixture(false);
        let original = f.root.path().join("retained-original-stage");
        let mut named = None;
        let outcome = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if let Point::StageCreated(path) = p {
                    fs::rename(&path, &original)?;
                    fs::create_dir(&path)?;
                    fs::write(path.join("foreign-evidence"), b"KEEP")?;
                    named = Some(path);
                    return Err(io::Error::other("injected root replacement"));
                }
                Ok(())
            },
        );
        let err = outcome.expect_err("replaced stage cannot be cleaned");
        assert!(matches!(err, InstallError::CleanupUncertain(_)));
        let named = named.expect("actual replacement");
        assert_eq!(
            fs::read(named.join("foreign-evidence")).expect("foreign name never touched"),
            b"KEEP"
        );
        assert!(original.is_dir());
        assert!(!f.destination.exists());
        assert_eq!(
            err.reconciliation().expect("reconcile").staged_inode,
            Some(
                fs::metadata(original)
                    .expect("original identity")
                    .ino()
                    .to_string()
            )
        );
    }
    #[test]
    fn source_parent_path_replacement_is_detected_on_both_sides_of_publication() {
        for at in [Point::BeforeCommit, Point::BeforeAck] {
            let mut f = fixture(false);
            let source_parent = f.root.path().join("source-only");
            fs::create_dir(&source_parent).expect("new source parent");
            fs::set_permissions(&source_parent, fs::Permissions::from_mode(0o700))
                .expect("private source parent");
            let source = source_parent.join("bundle.gz");
            fs::rename(&f.archive, &source).expect("owned source move");
            f.archive = source;
            let saved = f.root.path().join("original-source-parent");
            let result = install_with_hook(
                &f.archive,
                &f.archive_hash,
                &f.manifest_hash,
                &f.destination,
                |p| {
                    if p == at {
                        fs::rename(&source_parent, &saved)?;
                        fs::create_dir(&source_parent)?;
                        fs::copy(saved.join("bundle.gz"), &f.archive)?;
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert_eq!(
                hash(&fs::read(saved.join("bundle.gz")).expect("original archive")),
                f.archive_hash
            );
            if at == Point::BeforeAck {
                assert!(matches!(result, Err(InstallError::CommitUncertain(_))));
                assert!(f.destination.is_dir());
            } else {
                assert!(!f.destination.exists());
                no_temporary(&f);
            }
        }
    }
    #[test]
    fn published_bytes_changed_before_ack_are_uncertain_and_never_removed() {
        let f = fixture(false);
        let outcome = install_with_hook(
            &f.archive,
            &f.archive_hash,
            &f.manifest_hash,
            &f.destination,
            |p| {
                if p == Point::BeforeAck {
                    fs::write(
                        f.destination.join("bin/admiralty"),
                        b"post-publication mutation",
                    )?;
                }
                Ok(())
            },
        );
        assert!(matches!(outcome, Err(InstallError::CommitUncertain(_))));
        assert_eq!(
            fs::read(f.destination.join("bin/admiralty")).expect("preserved mutation evidence"),
            b"post-publication mutation"
        );
        assert_eq!(
            hash(&fs::read(&f.archive).expect("original archive")),
            f.archive_hash
        );
        assert!(install(&f).is_err());
    }
}
