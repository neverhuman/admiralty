//! Deterministic archives of explicitly staged native packages. No build, discovery,
//! installation, publisher authentication, or platform qualification is performed.
use crate::native_package::{PackageManifest, validate_staged_manifest};
use flate2::{Compression, GzBuilder};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const MAX_MANIFEST: usize = 1024 * 1024;
const MAX_FILES: usize = 1024;
const MAX_NODES: usize = 8192;
const MAX_DEPTH: usize = 32;
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE: u64 = MAX_TOTAL + 16 * 1024 * 1024;
const MANIFEST: &str = "MANIFEST.json";
const CHUNK: usize = 1024 * 1024;

#[derive(Debug)]
pub enum ArchiveError {
    Invalid(&'static str),
    Io(io::Error),
    /// A final name may exist; preserve it and reconcile, never automatically retry.
    CommitUncertain,
}
impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(stage) => write!(f, "native archive refused: {stage}"),
            Self::Io(error) => write!(f, "native archive I/O failed: {error}"),
            Self::CommitUncertain => f.write_str("native archive commit requires reconciliation"),
        }
    }
}
impl std::error::Error for ArchiveError {}
impl From<io::Error> for ArchiveError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<rustix::io::Errno> for ArchiveError {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Io(io::Error::from(value))
    }
}
type Result<T> = std::result::Result<T, ArchiveError>;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ArchiveReceipt {
    pub output: PathBuf,
    /// Complete canonical MANIFEST.json bytes, including its trailing newline.
    pub manifest_raw_sha256: String,
    /// Existing sha256:<hex> digest of the unsigned structured native manifest.
    pub unsigned_manifest_sha256: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub entry_count: u32,
    pub archive_created: bool,
    pub installer_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
    mode: u32,
    uid: u32,
    links: u64,
}
fn identity(file: &File) -> io::Result<Identity> {
    let m = file.metadata()?;
    Ok(Identity {
        dev: m.dev(),
        ino: m.ino(),
        size: m.len(),
        mtime: m.mtime(),
        mtime_ns: m.mtime_nsec(),
        ctime: m.ctime(),
        ctime_ns: m.ctime_nsec(),
        mode: m.mode(),
        uid: m.uid(),
        links: m.nlink(),
    })
}
fn same_inode(a: &Identity, b: &Identity) -> bool {
    a.dev == b.dev && a.ino == b.ino
}
fn owned(file: &File, private: bool) -> Result<()> {
    let m = file.metadata()?;
    if m.uid() != rustix::process::geteuid().as_raw()
        || (private && m.mode() & 0o077 != 0)
        || (!private && m.mode() & 0o022 != 0)
    {
        return Err(ArchiveError::Invalid("directory ownership or permissions"));
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
fn absolute_directory(path: &Path) -> Result<(File, PathBuf)> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute.as_os_str().len() > 4096 {
        return Err(ArchiveError::Invalid("directory path bound"));
    }
    let mut dir = File::from(rustix::fs::openat(
        rustix::fs::CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or(ArchiveError::Invalid("directory path encoding"))?;
                dir = directory_at(&dir, name)?;
            }
            _ => return Err(ArchiveError::Invalid("directory path traversal")),
        }
    }
    Ok((dir, absolute))
}
fn regular_at(parent: &File, name: &str) -> Result<File> {
    let st = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile || st.st_nlink != 1 {
        return Err(ArchiveError::Invalid(
            "source must be a regular file with one link",
        ));
    }
    let file = File::from(rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let m = file.metadata()?;
    if i128::from(m.dev()) != i128::from(st.st_dev)
        || i128::from(m.ino()) != i128::from(st.st_ino)
        || !m.is_file()
        || m.nlink() != 1
        || m.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(ArchiveError::Invalid("source identity or ownership"));
    }
    Ok(file)
}
fn relative_file(root: &File, path: &str) -> Result<File> {
    let mut components = path.split('/').peekable();
    let mut directory = directory_at(root, ".")?;
    while let Some(name) = components.next() {
        if components.peek().is_none() {
            return regular_at(&directory, name);
        }
        directory = directory_at(&directory, name)?;
        owned(&directory, false)?;
    }
    Err(ArchiveError::Invalid("empty source path"))
}
fn portable(path: &str) -> bool {
    if path.is_empty() || path.len() > 4096 || !path.is_ascii() {
        return false;
    }
    let mut depth = 0;
    for part in path.split('/') {
        depth += 1;
        if depth > MAX_DEPTH
            || part.is_empty()
            || part.len() > 255
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || part
                .bytes()
                .any(|b| b < 32 || b == 127 || b"\\:<>\"|?*".contains(&b))
        {
            return false;
        }
        let base = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(base.as_str(), "con" | "prn" | "aux" | "nul")
            || (base.len() == 4
                && (base.starts_with("com") || base.starts_with("lpt"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            return false;
        }
    }
    true
}
struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MANIFEST.saturating_sub(self.0.len()) {
            return Err(io::Error::other("manifest byte bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn canonical_manifest(expected: &PackageManifest, raw_hash: &str) -> Result<Vec<u8>> {
    if !valid_hash(raw_hash) || expected.entries.len() > MAX_FILES {
        return Err(ArchiveError::Invalid("manifest digest or entry bound"));
    }
    if expected.version.len() > 128
        || expected.target.len() > 128
        || !matches!(expected.source_commit.len(), 40 | 64)
        || expected.manifest_sha256.len() != 71
    {
        return Err(ArchiveError::Invalid("manifest metadata bound"));
    }
    if expected
        .entries
        .iter()
        .any(|entry| entry.archive_path.len() > 4096 || entry.sha256.len() != 64)
    {
        return Err(ArchiveError::Invalid("manifest entry field bound"));
    }
    let mut raw = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut raw, expected)
        .map_err(|_| ArchiveError::Invalid("manifest serialization bound"))?;
    raw.write_all(b"\n")?;
    validate_staged_manifest(expected)
        .map_err(|_| ArchiveError::Invalid("native staged manifest policy"))?;
    if hash(&raw.0) != raw_hash {
        return Err(ArchiveError::Invalid("complete manifest digest"));
    }
    Ok(raw.0)
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn expected_tree(manifest: &PackageManifest) -> Result<BTreeMap<String, bool>> {
    let mut tree = BTreeMap::new();
    tree.insert(MANIFEST.to_owned(), false);
    let mut seen = BTreeSet::new();
    let mut folded_nodes = BTreeMap::new();
    folded_nodes.insert(MANIFEST.to_ascii_lowercase(), MANIFEST.to_owned());
    seen.insert(MANIFEST.to_ascii_lowercase());
    for entry in &manifest.entries {
        if !portable(&entry.archive_path) {
            return Err(ArchiveError::Invalid("portable native path subset"));
        }
        let folded = entry.archive_path.to_ascii_lowercase();
        if seen.iter().any(|other| {
            other == &folded
                || other.starts_with(&(folded.clone() + "/"))
                || folded.starts_with(&(other.clone() + "/"))
        }) {
            return Err(ArchiveError::Invalid("portable path collision"));
        }
        seen.insert(folded);
        let mut prefix = String::new();
        let mut parts = entry.archive_path.split('/').peekable();
        while let Some(part) = parts.next() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let is_dir = parts.peek().is_some();
            let folded_prefix = prefix.to_ascii_lowercase();
            if folded_nodes
                .get(&folded_prefix)
                .is_some_and(|old| old != &prefix)
            {
                return Err(ArchiveError::Invalid("portable directory case collision"));
            }
            folded_nodes.insert(folded_prefix, prefix.clone());
            if tree.get(&prefix).is_some_and(|old| *old != is_dir) {
                return Err(ArchiveError::Invalid("file and directory collision"));
            }
            if !tree.contains_key(&prefix) && tree.len() >= MAX_NODES {
                return Err(ArchiveError::Invalid("staged tree node bound"));
            }
            tree.insert(prefix.clone(), is_dir);
        }
    }
    Ok(tree)
}
fn inventory(root: &File, tree: &BTreeMap<String, bool>) -> Result<BTreeMap<String, Identity>> {
    fn walk(
        directory: &File,
        prefix: &str,
        tree: &BTreeMap<String, bool>,
        found: &mut BTreeMap<String, Identity>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(ArchiveError::Invalid("directory depth bound"));
        }
        let mut entries = rustix::fs::Dir::read_from(directory)?;
        for entry in &mut entries {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| ArchiveError::Invalid("unexpected directory entry"))?;
            if matches!(name, "." | "..") {
                continue;
            }
            if name.len() > 255 || found.len() >= tree.len() {
                return Err(ArchiveError::Invalid("directory inventory bound"));
            }
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let is_dir = *tree
                .get(&path)
                .ok_or(ArchiveError::Invalid("unexpected staged entry"))?;
            let fd = if is_dir {
                directory_at(directory, name)?
            } else {
                regular_at(directory, name)?
            };
            if is_dir {
                owned(&fd, false)?;
            }
            if found.insert(path.clone(), identity(&fd)?).is_some() {
                return Err(ArchiveError::Invalid("duplicate directory entry"));
            }
            if is_dir {
                walk(&fd, &path, tree, found, depth + 1)?;
            }
        }
        Ok(())
    }
    let mut found = BTreeMap::new();
    walk(root, "", tree, &mut found, 0)?;
    if found.len() != tree.len() {
        return Err(ArchiveError::Invalid("missing staged entry"));
    }
    Ok(found)
}
fn hash_file(file: &mut File, expected_size: u64, cap: u64) -> Result<String> {
    if expected_size > cap {
        return Err(ArchiveError::Invalid("file byte bound"));
    }
    let before = identity(file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0; CHUNK];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or(ArchiveError::Invalid("file size overflow"))?;
        if total > expected_size || total > cap {
            return Err(ArchiveError::Invalid("file grew while read"));
        }
        hasher.update(&buffer[..n]);
    }
    if total != expected_size {
        return Err(ArchiveError::Invalid("file shortened while read"));
    }
    if identity(file)? != before {
        return Err(ArchiveError::Invalid("file identity changed while read"));
    }
    Ok(format!("{:x}", hasher.finalize()))
}
struct Source {
    path: String,
    file: File,
    identity: Identity,
    sha256: String,
    size: u64,
    mode: u32,
}
struct HashWriter<'a> {
    file: &'a mut File,
    bytes: u64,
    hasher: Sha256,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > MAX_ARCHIVE.saturating_sub(self.bytes) {
            return Err(io::Error::other("archive byte bound"));
        }
        let n = self.file.write(bytes)?;
        self.bytes += n as u64;
        self.hasher.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
struct HashReader<'a> {
    file: &'a mut File,
    bytes: u64,
    expected: u64,
    hasher: Sha256,
}
impl Read for HashReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(bytes)?;
        self.bytes = self
            .bytes
            .checked_add(n as u64)
            .ok_or_else(|| io::Error::other("source size overflow"))?;
        if self.bytes > self.expected {
            return Err(io::Error::other("source grew during archive"));
        }
        self.hasher.update(&bytes[..n]);
        Ok(n)
    }
}
fn append<W: Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    size: u64,
    mode: u32,
    data: impl Read,
) -> io::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("")?;
    header.set_groupname("")?;
    tar.append_data(&mut header, name, data)
}
struct TempOutput {
    parent: File,
    name: String,
    file: File,
    committed: bool,
}
impl TempOutput {
    fn create(parent: &File) -> Result<Self> {
        let name = format!(".admiralty-native-archive-{}.tmp", uuid::Uuid::new_v4());
        let file = File::from(rustix::fs::openat(
            parent,
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        Ok(Self {
            parent: parent.try_clone()?,
            name,
            file,
            committed: false,
        })
    }
}
impl Drop for TempOutput {
    fn drop(&mut self) {
        if !self.committed
            && let (Ok(st), Ok(m)) = (
                rustix::fs::statat(&self.parent, self.name.as_str(), AtFlags::SYMLINK_NOFOLLOW),
                self.file.metadata(),
            )
            && i128::from(st.st_dev) == i128::from(m.dev())
            && i128::from(st.st_ino) == i128::from(m.ino())
        {
            let _ = rustix::fs::unlinkat(&self.parent, self.name.as_str(), AtFlags::empty());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    SourcesPinned,
    FileAppended(usize),
    BeforeFinish,
    BeforeCommit,
    AfterCommit,
}

/// Archive only an existing native stage. Expected manifest and digests are operator
/// inputs; matching bytes do not authenticate a publisher or prove source provenance.
/// Complete raw hashes are 64 lowercase hex digits; the existing unsigned manifest
/// hash retains its distinct sha256:<hex> format. Unknown commit errors never retry.
pub fn archive_native_package(
    staged_dir: &Path,
    expected: &PackageManifest,
    expected_manifest_raw_sha256: &str,
    output: &Path,
) -> Result<ArchiveReceipt> {
    archive_with_hook(
        staged_dir,
        expected,
        expected_manifest_raw_sha256,
        output,
        |_| Ok(()),
    )
}
fn archive_with_hook(
    staged_dir: &Path,
    expected: &PackageManifest,
    raw_hash: &str,
    output: &Path,
    mut hook: impl FnMut(Point) -> io::Result<()>,
) -> Result<ArchiveReceipt> {
    let manifest_bytes = canonical_manifest(expected, raw_hash)?;
    let tree = expected_tree(expected)?;
    let (root, root_path) = absolute_directory(staged_dir)?;
    owned(&root, true)?;
    let root_identity = identity(&root)?;
    let before_inventory = inventory(&root, &tree)?;
    let mut manifest_file = relative_file(&root, MANIFEST)?;
    let manifest_identity = identity(&manifest_file)?;
    if manifest_identity.size != manifest_bytes.len() as u64
        || manifest_identity.mode & 0o7777 != 0o644
    {
        return Err(ArchiveError::Invalid("manifest size or mode"));
    }
    let mut actual_manifest = Vec::with_capacity(manifest_bytes.len());
    (&mut manifest_file)
        .take(MAX_MANIFEST as u64 + 1)
        .read_to_end(&mut actual_manifest)?;
    if actual_manifest != manifest_bytes || identity(&manifest_file)? != manifest_identity {
        return Err(ArchiveError::Invalid("manifest bytes or identity"));
    }
    let mut sources = Vec::with_capacity(expected.entries.len());
    let mut total = 0_u64;
    for entry in &expected.entries {
        let mut file = relative_file(&root, &entry.archive_path)?;
        let id = identity(&file)?;
        if id.size != entry.size || id.mode & 0o7777 != entry.mode || entry.size > MAX_FILE {
            return Err(ArchiveError::Invalid("entry size or mode"));
        }
        total = total
            .checked_add(id.size)
            .ok_or(ArchiveError::Invalid("total size overflow"))?;
        if total > MAX_TOTAL
            || hash_file(&mut file, entry.size, MAX_FILE)? != entry.sha256
            || identity(&file)? != id
        {
            return Err(ArchiveError::Invalid("entry content or identity"));
        }
        sources.push(Source {
            path: entry.archive_path.clone(),
            file,
            identity: id,
            sha256: entry.sha256.clone(),
            size: entry.size,
            mode: entry.mode,
        });
    }
    hook(Point::SourcesPinned)?;
    let parent_path = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(ArchiveError::Invalid("output parent"))?;
    let name = output
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(ArchiveError::Invalid("output filename"))?;
    if !portable(name) || name.contains('/') {
        return Err(ArchiveError::Invalid("output filename"));
    }
    let (parent, parent_path) = absolute_directory(parent_path)?;
    owned(&parent, true)?;
    match rustix::fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => return Err(ArchiveError::Invalid("output already exists")),
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let mut temp = TempOutput::create(&parent)?;
    let writer = HashWriter {
        file: &mut temp.file,
        bytes: 0,
        hasher: Sha256::new(),
    };
    let gzip = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(writer, Compression::best());
    let mut tar = tar::Builder::new(gzip);
    tar.follow_symlinks(false);
    tar.sparse(false);
    append(
        &mut tar,
        MANIFEST,
        manifest_bytes.len() as u64,
        0o644,
        manifest_bytes.as_slice(),
    )?;
    for (index, source) in sources.iter_mut().enumerate() {
        source.file.seek(SeekFrom::Start(0))?;
        let mut reader = HashReader {
            file: &mut source.file,
            bytes: 0,
            expected: source.size,
            hasher: Sha256::new(),
        };
        append(
            &mut tar,
            &source.path,
            source.size,
            source.mode,
            &mut reader,
        )?;
        if reader.bytes != source.size
            || format!("{:x}", reader.hasher.finalize()) != source.sha256
            || identity(&source.file)? != source.identity
        {
            return Err(ArchiveError::Invalid("source changed during archive"));
        }
        hook(Point::FileAppended(index))?;
    }
    hook(Point::BeforeFinish)?;
    tar.finish()?;
    let gzip = tar.into_inner()?;
    let mut writer = gzip.finish()?;
    writer.flush()?;
    let archive_bytes = writer.bytes;
    let archive_sha256 = format!("{:x}", writer.hasher.finalize());
    temp.file.sync_all()?;
    rustix::fs::fchmod(&temp.file, Mode::from_bits_truncate(0o644))?;
    temp.file.sync_all()?;
    if hash_file(&mut temp.file, archive_bytes, MAX_ARCHIVE)? != archive_sha256 {
        return Err(ArchiveError::Invalid("completed archive content"));
    }
    // All original handles and exact named identities remain bound through acknowledgement.
    if identity(&root)? != root_identity
        || inventory(&root, &tree)? != before_inventory
        || identity(&manifest_file)? != manifest_identity
        || hash_file(
            &mut manifest_file,
            manifest_bytes.len() as u64,
            MAX_MANIFEST as u64,
        )? != raw_hash
    {
        return Err(ArchiveError::Invalid("staged tree changed"));
    }
    for source in &mut sources {
        let named = relative_file(&root, &source.path)?;
        if identity(&named)? != source.identity
            || identity(&source.file)? != source.identity
            || hash_file(&mut source.file, source.size, MAX_FILE)? != source.sha256
        {
            return Err(ArchiveError::Invalid("source changed before commit"));
        }
    }
    let (named_root, _) = absolute_directory(&root_path)?;
    if identity(&named_root)? != root_identity {
        return Err(ArchiveError::Invalid("staged root replaced"));
    }
    let (named_parent, _) = absolute_directory(&parent_path)?;
    if !same_inode(&identity(&named_parent)?, &identity(&parent)?) {
        return Err(ArchiveError::Invalid("output parent replaced"));
    }
    hook(Point::BeforeCommit)?;
    let temporary_id = identity(&temp.file)?;
    let named_temp = regular_at(&parent, &temp.name)?;
    if identity(&named_temp)? != temporary_id {
        return Err(ArchiveError::Invalid("temporary output replaced"));
    }
    rustix::fs::renameat_with(
        &parent,
        temp.name.as_str(),
        &parent,
        name,
        RenameFlags::NOREPLACE,
    )?;
    temp.committed = true;
    // Any failure beyond the atomic rename has an explicitly uncertain commit result.
    if hook(Point::AfterCommit).is_err() || rustix::fs::fsync(&parent).is_err() {
        return Err(ArchiveError::CommitUncertain);
    }
    let (published_parent, _) =
        absolute_directory(&parent_path).map_err(|_| ArchiveError::CommitUncertain)?;
    if !same_inode(
        &identity(&published_parent).map_err(|_| ArchiveError::CommitUncertain)?,
        &identity(&parent).map_err(|_| ArchiveError::CommitUncertain)?,
    ) {
        return Err(ArchiveError::CommitUncertain);
    }
    let mut final_file = regular_at(&parent, name).map_err(|_| ArchiveError::CommitUncertain)?;
    let final_identity = identity(&final_file).map_err(|_| ArchiveError::CommitUncertain)?;
    if !same_inode(&final_identity, &temporary_id)
        || final_identity.mode & 0o7777 != 0o644
        || final_identity.size != archive_bytes
        || hash_file(&mut final_file, archive_bytes, MAX_ARCHIVE)
            .map_err(|_| ArchiveError::CommitUncertain)?
            != archive_sha256
    {
        return Err(ArchiveError::CommitUncertain);
    }
    Ok(ArchiveReceipt {
        output: parent_path.join(name),
        manifest_raw_sha256: raw_hash.to_owned(),
        unsigned_manifest_sha256: expected.manifest_sha256.clone(),
        archive_sha256,
        archive_bytes,
        entry_count: expected.entries.len() as u32,
        archive_created: true,
        installer_complete: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_package::{PackageInput, PackageReceipt, stage_native_package};
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, SystemTime};

    fn fixture(long: bool) -> (tempfile::TempDir, Vec<PackageInput>) {
        let root = tempfile::Builder::new()
            .prefix("native-archive-")
            .tempdir_in(std::env::temp_dir().canonicalize().expect("canonical tmp"))
            .expect("private fixture");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("fixture mode");
        let mut names = vec![
            (
                "target/release/admiralty".to_owned(),
                b"owned toy binary".to_vec(),
                0o755,
            ),
            (
                "apps/dashboard/dist/index.html".to_owned(),
                b"<main>actual fixture</main>".to_vec(),
                0o644,
            ),
            (
                "packages/contracts/generated/validators.mjs".to_owned(),
                b"export const validateDomainEventV2 = () => true;".to_vec(),
                0o644,
            ),
            (
                "schemas/domain-event-v2.schema.json".to_owned(),
                include_bytes!("../../../schemas/domain-event-v2.schema.json").to_vec(),
                0o644,
            ),
            (
                "docs/operators/release.md".to_owned(),
                b"explicit release notes".to_vec(),
                0o644,
            ),
        ];
        if long {
            names.push((
                format!("docs/{}.md", "a".repeat(200)),
                b"long portable path".to_vec(),
                0o644,
            ));
        }
        let mut inputs = Vec::new();
        for (name, bytes, mode) in names {
            let path = root.path().join(&name);
            fs::create_dir_all(path.parent().expect("source parent"))
                .expect("create source parent");
            fs::write(&path, &bytes).expect("source bytes");
            inputs.push(PackageInput {
                source: name,
                expected_sha256: hash(&bytes),
                mode,
            });
        }
        (root, inputs)
    }
    fn stage(root: &Path, name: &str, inputs: &[PackageInput]) -> PackageReceipt {
        stage_native_package(
            root,
            &root.join(name),
            "2.0.0",
            "fixture-unix",
            &"a".repeat(40),
            inputs,
        )
        .expect("actual native staging")
    }
    fn raw_digest(stage: &PackageReceipt) -> String {
        hash(&fs::read(stage.output_dir.join(MANIFEST)).expect("staged manifest"))
    }
    fn archive(root: &Path, stage: &PackageReceipt, name: &str) -> Result<ArchiveReceipt> {
        archive_native_package(
            &stage.output_dir,
            &stage.manifest,
            &raw_digest(stage),
            &root.join(name),
        )
    }
    fn special(path: &Path) -> Option<std::os::unix::net::UnixListener> {
        #[cfg(target_os = "linux")]
        {
            let (parent, _) =
                absolute_directory(path.parent().expect("special parent")).expect("parent");
            rustix::fs::mkfifoat(
                &parent,
                path.file_name().expect("special name"),
                Mode::from_bits_truncate(0o600),
            )
            .expect("owned FIFO");
            None
        }
        #[cfg(not(target_os = "linux"))]
        {
            Some(std::os::unix::net::UnixListener::bind(path).expect("owned nonregular socket"))
        }
    }
    fn resign(manifest: &mut PackageManifest) {
        #[derive(Serialize)]
        struct Unsigned<'a> {
            schema_version: u32,
            version: &'a str,
            target: &'a str,
            source_commit: &'a str,
            entries: &'a [crate::native_package::PackageEntry],
        }
        manifest.manifest_sha256 = admiralty_core::content_digest(&Unsigned {
            schema_version: manifest.schema_version,
            version: &manifest.version,
            target: &manifest.target,
            source_commit: &manifest.source_commit,
            entries: &manifest.entries,
        })
        .expect("test unsigned digest");
    }
    #[test]
    fn actual_staged_roundtrip_manifest_headers_hashes_and_private_extraction() {
        let (root, inputs) = fixture(true);
        let stage = stage(root.path(), "stage", &inputs);
        let receipt = archive(root.path(), &stage, "bundle.tar.gz").expect("archive");
        let bytes = fs::read(&receipt.output).expect("archive bytes");
        assert_eq!(receipt.archive_sha256, hash(&bytes));
        assert_eq!(receipt.archive_bytes, bytes.len() as u64);
        assert_eq!(receipt.manifest_raw_sha256, raw_digest(&stage));
        assert_eq!(
            receipt.unsigned_manifest_sha256,
            stage.manifest.manifest_sha256
        );
        assert!(receipt.archive_created);
        assert!(!receipt.installer_complete);
        assert_eq!(&bytes[4..8], &[0; 4]);
        assert_eq!(bytes[3], 0);
        assert_eq!(bytes[9], 255);
        let mut reader = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let mut seen = Vec::new();
        for entry in reader.entries().expect("tar entries") {
            let mut entry = entry.expect("entry");
            let path = entry
                .path()
                .expect("entry path")
                .to_str()
                .expect("ASCII")
                .to_owned();
            let header = entry.header();
            assert!(header.entry_type().is_file());
            assert_eq!(header.uid().expect("uid"), 0);
            assert_eq!(header.gid().expect("gid"), 0);
            assert_eq!(header.mtime().expect("mtime"), 0);
            assert_eq!(header.username().expect("username"), Some(""));
            assert_eq!(header.groupname().expect("groupname"), Some(""));
            let mode = header.mode().expect("mode");
            let mut actual = Vec::new();
            entry
                .read_to_end(&mut actual)
                .expect("bounded fixture bytes");
            if path == MANIFEST {
                assert_eq!(mode, 0o644);
                assert_eq!(
                    actual,
                    fs::read(stage.output_dir.join(MANIFEST)).expect("original manifest")
                );
            } else {
                let expected = stage
                    .manifest
                    .entries
                    .iter()
                    .find(|e| e.archive_path == path)
                    .expect("only explicit entry");
                assert_eq!(actual.len() as u64, expected.size);
                assert_eq!(hash(&actual), expected.sha256);
                assert_eq!(mode, expected.mode);
            }
            seen.push(path);
        }
        let expected: Vec<_> = std::iter::once(MANIFEST.to_owned())
            .chain(
                stage
                    .manifest
                    .entries
                    .iter()
                    .map(|e| e.archive_path.clone()),
            )
            .collect();
        assert_eq!(seen, expected);
        let extracted = root.path().join("extracted");
        fs::create_dir(&extracted).expect("new extraction fixture");
        fs::set_permissions(&extracted, fs::Permissions::from_mode(0o700))
            .expect("private extraction");
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()))
            .unpack(&extracted)
            .expect("test extraction");
        for e in &stage.manifest.entries {
            let p = extracted.join(&e.archive_path);
            assert_eq!(hash(&fs::read(&p).expect("extracted bytes")), e.sha256);
            assert_eq!(
                fs::metadata(p).expect("extracted metadata").mode() & 0o777,
                e.mode
            );
        }
    }
    #[test]
    fn deterministic_across_real_stages_input_order_and_source_mtime() {
        let (root, mut inputs) = fixture(false);
        let first = stage(root.path(), "first", &inputs);
        let a = archive(root.path(), &first, "a.tar.gz").expect("first archive");
        File::open(root.path().join(&inputs[0].source))
            .expect("source")
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(42)),
            )
            .expect("source mtime");
        inputs.reverse();
        let second = stage(root.path(), "second", &inputs);
        let b = archive(root.path(), &second, "b.tar.gz").expect("second archive");
        assert_eq!(a.archive_sha256, b.archive_sha256);
        eprintln!(
            "native-archive determinism before={} after={} bytes={}",
            a.archive_sha256, b.archive_sha256, a.archive_bytes
        );
        assert_eq!(
            fs::read(a.output).expect("a"),
            fs::read(b.output).expect("b")
        );
    }
    #[test]
    fn both_manifest_digests_and_exact_disk_json_are_required() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let output = root.path().join("bad.gz");
        assert!(
            archive_native_package(
                &staged.output_dir,
                &staged.manifest,
                &"0".repeat(64),
                &output
            )
            .is_err()
        );
        let mut wrong = staged.manifest.clone();
        wrong.manifest_sha256 = "sha256:".to_owned() + &"0".repeat(64);
        assert!(
            archive_native_package(&staged.output_dir, &wrong, &raw_digest(&staged), &output)
                .is_err()
        );
        let canonical = fs::read(staged.output_dir.join(MANIFEST)).expect("canonical");
        let mut value: serde_json::Value = serde_json::from_slice(&canonical).expect("test JSON");
        value["unknown"] = true.into();
        fs::write(
            staged.output_dir.join(MANIFEST),
            serde_json::to_vec(&value).expect("modified JSON"),
        )
        .expect("write unknown");
        assert!(
            archive_native_package(
                &staged.output_dir,
                &staged.manifest,
                &hash(&canonical),
                &output
            )
            .is_err()
        );
        assert!(!output.exists());
    }
    #[test]
    fn missing_extra_and_unexpected_special_entries_fail_without_content_capture() {
        for case in 0..4 {
            let (root, inputs) = fixture(false);
            let staged = stage(root.path(), "stage", &inputs);
            let mut special_handle = None;
            match case {
                0 => {
                    fs::remove_file(staged.output_dir.join("bin/admiralty")).expect("remove");
                }
                1 => {
                    fs::write(
                        staged.output_dir.join("unexpected"),
                        b"never an archive input",
                    )
                    .expect("extra");
                }
                2 => {
                    fs::create_dir(staged.output_dir.join("unexpected-dir"))
                        .expect("extra directory");
                }
                _ => {
                    special_handle = special(&staged.output_dir.join("unexpected-special"));
                }
            }
            assert!(archive(root.path(), &staged, "bad.gz").is_err());
            assert!(!root.path().join("bad.gz").exists());
            drop(special_handle);
        }
    }
    #[test]
    fn changed_bytes_modes_sizes_symlink_hardlink_and_special_are_rejected() {
        for case in 0..6 {
            let (root, inputs) = fixture(false);
            let staged = stage(root.path(), "stage", &inputs);
            let binary = staged.output_dir.join("bin/admiralty");
            let mut special_handle = None;
            match case {
                0 => fs::write(&binary, b"different bytes!").expect("bytes"),
                1 => fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).expect("mode"),
                2 => File::options()
                    .write(true)
                    .open(&binary)
                    .expect("binary")
                    .set_len(MAX_FILE + 1)
                    .expect("sparse oversized"),
                3 => {
                    fs::remove_file(&binary).expect("remove");
                    symlink(root.path().join("target/release/admiralty"), &binary)
                        .expect("symlink");
                }
                4 => {
                    fs::hard_link(&binary, root.path().join("linked")).expect("hardlink");
                }
                _ => {
                    fs::remove_file(&binary).expect("remove");
                    special_handle = special(&binary);
                }
            }
            assert!(archive(root.path(), &staged, "bad.gz").is_err());
            assert!(!root.path().join("bad.gz").exists());
            drop(special_handle);
        }
    }
    #[test]
    fn root_parent_links_unsafe_manifests_and_bounds_are_rejected() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let link = root.path().join("root-link");
        symlink(&staged.output_dir, &link).expect("root symlink");
        assert!(
            archive_native_package(
                &link,
                &staged.manifest,
                &raw_digest(&staged),
                &root.path().join("bad.gz")
            )
            .is_err()
        );
        let outlink = root.path().join("out-link");
        symlink(root.path(), &outlink).expect("output parent symlink");
        assert!(
            archive_native_package(
                &staged.output_dir,
                &staged.manifest,
                &raw_digest(&staged),
                &outlink.join("bad.gz")
            )
            .is_err()
        );
        for name in [
            "../escape",
            "share/doc/admiralty/.env",
            "share/doc/admiralty/CON.txt",
            "share/doc/admiralty/café.md",
            "share/doc/admiralty/a/../b",
            "MANIFEST.json",
            "share/doc/admiralty/a\\b",
        ] {
            let mut changed = staged.manifest.clone();
            changed.entries[0].archive_path = name.to_owned();
            changed
                .entries
                .sort_by(|a, b| a.archive_path.cmp(&b.archive_path));
            resign(&mut changed);
            assert!(
                archive_native_package(
                    &staged.output_dir,
                    &changed,
                    &raw_digest(&staged),
                    &root.path().join("bad.gz")
                )
                .is_err()
            );
        }
        assert!(!portable(&format!("share/{}end", "a/".repeat(MAX_DEPTH))));
        let mut huge = staged.manifest.clone();
        huge.version = "a".repeat(MAX_MANIFEST + 1);
        assert!(canonical_manifest(&huge, &"0".repeat(64)).is_err());
        let mut many = staged.manifest.clone();
        many.entries = vec![many.entries[0].clone(); MAX_FILES + 1];
        assert!(canonical_manifest(&many, &"0".repeat(64)).is_err());
        let mut large = staged.manifest.clone();
        large.entries[0].size = MAX_FILE + 1;
        resign(&mut large);
        assert!(canonical_manifest(&large, &"0".repeat(64)).is_err());
    }
    #[test]
    fn directory_case_and_file_ancestor_collisions_cannot_be_normalized_away() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let mut changed = staged.manifest.clone();
        let mut a = changed.entries.last().expect("entry").clone();
        a.archive_path = "share/doc/admiralty/Case/a.md".to_owned();
        let mut b = a.clone();
        b.archive_path = "share/doc/admiralty/case/b.md".to_owned();
        changed.entries.extend([a, b]);
        assert!(expected_tree(&changed).is_err());
        let mut changed = staged.manifest.clone();
        let mut a = changed.entries.last().expect("entry").clone();
        a.archive_path = changed.entries[0].archive_path.clone() + "/child";
        changed.entries.push(a);
        assert!(expected_tree(&changed).is_err());
    }
    #[test]
    fn source_mutation_after_preflight_and_after_append_is_never_acknowledged() {
        for point in [Point::SourcesPinned, Point::FileAppended(0)] {
            let (root, inputs) = fixture(false);
            let staged = stage(root.path(), "stage", &inputs);
            let output = root.path().join("bad.gz");
            let result = archive_with_hook(
                &staged.output_dir,
                &staged.manifest,
                &raw_digest(&staged),
                &output,
                |actual| {
                    if actual == point {
                        fs::write(
                            staged.output_dir.join("bin/admiralty"),
                            b"changed in capture",
                        )?;
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(!output.exists());
        }
    }
    #[test]
    fn pinned_ancestor_swap_never_reads_foreign_source_or_acknowledges() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        fs::write(outside.join("admiralty"), b"FOREIGN").expect("foreign");
        let result = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &root.path().join("bad.gz"),
            |point| {
                if point == Point::SourcesPinned {
                    fs::rename(
                        staged.output_dir.join("bin"),
                        root.path().join("original-bin"),
                    )?;
                    symlink(&outside, staged.output_dir.join("bin"))?;
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read(outside.join("admiralty")).expect("foreign unchanged"),
            b"FOREIGN"
        );
        assert!(!root.path().join("bad.gz").exists());
    }
    #[test]
    fn no_replace_preserves_existing_files_links_and_creation_race() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let output = root.path().join("keep.gz");
        fs::write(&output, b"KEEP").expect("existing");
        assert!(archive(root.path(), &staged, "keep.gz").is_err());
        assert_eq!(fs::read(&output).expect("preserved"), b"KEEP");
        fs::remove_file(&output).expect("remove own fixture");
        symlink(root.path().join("absent"), &output).expect("dangling output");
        assert!(archive(root.path(), &staged, "keep.gz").is_err());
        assert!(
            fs::symlink_metadata(&output)
                .expect("link preserved")
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&output).expect("remove own link");
        let result = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &output,
            |point| {
                if point == Point::BeforeCommit {
                    fs::write(&output, b"RACER")?;
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(output).expect("race preserved"), b"RACER");
    }
    #[test]
    fn precommit_failure_cleans_only_owned_temp_and_unknown_commit_is_preserved() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let output = root.path().join("a.gz");
        let before = fs::read_dir(root.path()).expect("before").count();
        let fail = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &output,
            |point| {
                if point == Point::BeforeFinish {
                    Err(io::Error::other("injected candidate failure"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(fail.is_err());
        assert!(!output.exists());
        assert_eq!(before, fs::read_dir(root.path()).expect("after").count());
        let unknown = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &output,
            |point| {
                if point == Point::AfterCommit {
                    Err(io::Error::other("injected acknowledgement loss"))
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(unknown, Err(ArchiveError::CommitUncertain)));
        assert!(output.is_file());
        let original = fs::read(&output).expect("retained artifact");
        assert!(archive(root.path(), &staged, "a.gz").is_err());
        assert_eq!(fs::read(&output).expect("never replaced"), original);
    }
    #[test]
    fn streaming_writer_and_exact_reader_bounds_propagate_failures() {
        let root =
            tempfile::tempdir_in(std::env::temp_dir().canonicalize().expect("canonical tmp"))
                .expect("tmp");
        let mut file = File::create(root.path().join("writer")).expect("writer");
        let mut writer = HashWriter {
            file: &mut file,
            bytes: MAX_ARCHIVE,
            hasher: Sha256::new(),
        };
        assert!(writer.write_all(b"x").is_err());
        let mut raw = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(root.path().join("reader"))
            .expect("reader");
        raw.write_all(b"too long").expect("fixture");
        raw.rewind().expect("rewind");
        let mut reader = HashReader {
            file: &mut raw,
            bytes: 0,
            expected: 2,
            hasher: Sha256::new(),
        };
        assert!(reader.read(&mut [0; 16]).is_err());
    }
    #[test]
    fn exact_total_tree_and_serialization_bounds_fail_at_their_own_checks() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let mut large = staged.manifest.clone();
        for e in large.entries.iter_mut().take(3) {
            e.size = MAX_FILE;
        }
        resign(&mut large);
        let mut raw = serde_json::to_vec(&large).expect("expected bytes");
        raw.push(b'\n');
        assert!(matches!(
            canonical_manifest(&large, &hash(&raw)),
            Err(ArchiveError::Invalid("native staged manifest policy"))
        ));
        let mut many = staged.manifest.clone();
        let template = many.entries[0].clone();
        many.entries.clear();
        for i in 0..MAX_FILES {
            let mut e = template.clone();
            e.archive_path = format!("share/doc/admiralty/{i}/a/b/c/d/e/f/g/h.md");
            many.entries.push(e);
        }
        assert!(matches!(
            expected_tree(&many),
            Err(ArchiveError::Invalid("staged tree node bound"))
        ));
        let mut huge = staged.manifest.clone();
        huge.entries = vec![huge.entries[0].clone(); MAX_FILES];
        for entry in &mut huge.entries {
            entry.archive_path = "x".repeat(4096);
        }
        assert!(matches!(
            canonical_manifest(&huge, &"0".repeat(64)),
            Err(ArchiveError::Invalid("manifest serialization bound"))
        ));
        // Same-length disk reordering must be refused even when valid JSON and full expected hash are unchanged.
        let canonical = fs::read(staged.output_dir.join(MANIFEST)).expect("canonical");
        let value: serde_json::Value = serde_json::from_slice(&canonical).expect("test JSON");
        let mut reordered = serde_json::to_vec(&value).expect("reordered");
        reordered.push(b'\n');
        assert_eq!(reordered.len(), canonical.len());
        assert_ne!(reordered, canonical);
        fs::write(staged.output_dir.join(MANIFEST), reordered).expect("write same-sized mutation");
        assert!(
            archive_native_package(
                &staged.output_dir,
                &staged.manifest,
                &hash(&canonical),
                &root.path().join("bad.gz")
            )
            .is_err()
        );
        assert!(!root.path().join("bad.gz").exists());
    }
    #[test]
    fn output_parent_swap_never_acknowledges_a_stale_published_path() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let parent = root.path().join("output");
        fs::create_dir(&parent).expect("output parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("private output");
        let moved = root.path().join("moved-output");
        let destination = parent.join("bundle.gz");
        let result = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &destination,
            |point| {
                if point == Point::BeforeCommit {
                    fs::rename(&parent, &moved)?;
                    fs::create_dir(&parent)?;
                    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(ArchiveError::CommitUncertain)));
        assert!(!destination.exists());
        assert!(moved.join("bundle.gz").is_file());
    }
    #[test]
    fn replacement_temp_is_rejected_and_foreign_name_is_never_cleaned_up() {
        let (root, inputs) = fixture(false);
        let staged = stage(root.path(), "stage", &inputs);
        let output = root.path().join("bad.gz");
        let mut replacement = None;
        let result = archive_with_hook(
            &staged.output_dir,
            &staged.manifest,
            &raw_digest(&staged),
            &output,
            |point| {
                if point == Point::BeforeCommit {
                    let path = fs::read_dir(root.path())?
                        .filter_map(std::result::Result::ok)
                        .find(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .starts_with(".admiralty-native-archive-")
                        })
                        .expect("owned temporary candidate")
                        .path();
                    fs::remove_file(&path)?;
                    fs::write(&path, b"FOREIGN REPLACEMENT")?;
                    replacement = Some(path);
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!output.exists());
        assert_eq!(
            fs::read(replacement.expect("replacement path")).expect("replacement preserved"),
            b"FOREIGN REPLACEMENT"
        );
    }
}
