//! Registered, bounded local scratch retirement. See docs/operators/cleanup.md.
use admiralty_core::retirement::RetirementScope;
use fs2::FileExt;
use rustix::fs::{self as at, AtFlags, Mode, OFlags, RenameFlags, StatxFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

const MAX_FILES: usize = 128;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_JOURNAL: u64 = 128 * 1024;
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("cleanup filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("cleanup journal error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("preserve asset: {0}")]
    Preserve(&'static str),
    #[error("durable custody fence refused retirement: {0}")]
    Custody(String),
}
pub type Result<T> = std::result::Result<T, CleanupError>;

/// The store adapter validates the durable claim and its complete pin history.
/// An implementation is a trusted local authority, never a worker assertion.
pub trait RetirementFence {
    fn recheck(&mut self, scope: &RetirementScope, registration_digest: &str) -> Result<()>;
}

/// Fresh assertions from the trusted local lifecycle/backup owners. Unknown is
/// a hold. Filesystem identities, contents and registered local processes are also
/// checked by the executor; these assertions do not replace those checks.
#[derive(Clone, Debug, Default)]
pub struct Preservation {
    pub dirty: Option<bool>,
    pub ignored_private: Option<bool>,
    pub unpushed: Option<bool>,
    pub unmerged: Option<bool>,
    pub process_held: Option<bool>,
    pub foreign: Option<bool>,
    pub backup_candidate_held: Option<bool>,
    pub unique_data: Option<bool>,
}
impl Preservation {
    fn check(&self) -> Result<()> {
        if [
            self.dirty,
            self.ignored_private,
            self.unpushed,
            self.unmerged,
            self.process_held,
            self.foreign,
            self.backup_candidate_held,
            self.unique_data,
        ]
        .iter()
        .any(|value| *value != Some(false))
        {
            return Err(CleanupError::Preserve(
                "unknown or active preservation hold",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    mount: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    identity: Identity,
    bytes: u64,
    digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    scope: RetirementScope,
    name: String,
    root: Identity,
    directory: Identity,
    members: BTreeMap<String, Member>,
    processes: Vec<ProcessIdentity>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Registered,
    Claimed,
    Quarantining,
    Quarantined,
    Deleting,
    Deleted,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    registration: Registration,
    stage: Stage,
}
#[derive(Debug, Serialize)]
pub struct CleanupReceipt {
    pub scope: RetirementScope,
    pub stage: Stage,
    pub registration_digest: String,
    pub registered_bytes: String,
    pub disposed_bytes: String,
    pub retained_evidence: bool,
}

/// One writer holds the separate private journal lock for its whole lifetime.
/// Roots must be disjoint. No recursive removal, Git invocation or process kill.
pub struct CleanupExecutor {
    root_path: PathBuf,
    root: File,
    ledger_path: PathBuf,
    ledger: File,
    _lock: File,
}
impl CleanupExecutor {
    pub fn open(root: &Path, ledger: &Path) -> Result<Self> {
        if root.starts_with(ledger) || ledger.starts_with(root) {
            return Err(CleanupError::Preserve(
                "managed root and evidence ledger overlap",
            ));
        }
        let root_fd = open_directory(root)?;
        let ledger_fd = open_directory(ledger)?;
        private(&root_fd)?;
        private(&ledger_fd)?;
        let lock = file_at(&ledger_fd, ".lock", OFlags::RDWR | OFlags::CREATE)?;
        private(&lock)?;
        lock.try_lock_exclusive()?;
        Ok(Self {
            root_path: root.into(),
            root: root_fd,
            ledger_path: ledger.into(),
            ledger: ledger_fd,
            _lock: lock,
        })
    }

    /// Explicit enrollment by the asset owner, at scratch creation. Discovery
    /// never calls this method. Only flat scratch with exact known bytes is
    /// supported; directories, Git metadata and links are preservation holds.
    pub fn register(
        &self,
        scope: RetirementScope,
        name: &str,
        process_ids: &[u32],
    ) -> Result<CleanupReceipt> {
        component(name)?;
        self.roots_current()?;
        let id = journal_id(&scope)?;
        if exists(&self.ledger, &format!("{id}.json"))? {
            return Err(CleanupError::Preserve("asset already registered"));
        }
        let directory = file_at(&self.root, name, DIRECTORY)?;
        private(&directory)?;
        let root = identity(&self.root)?;
        let dir_identity = identity(&directory)?;
        if root.mount != dir_identity.mount {
            return Err(CleanupError::Preserve("asset crosses a mount boundary"));
        }
        let members = inventory(&directory, root.mount)?;
        if process_ids.len() > MAX_FILES {
            return Err(CleanupError::Preserve("process registration limit"));
        }
        let processes = process_ids
            .iter()
            .map(|pid| process_identity(*pid))
            .collect::<Result<Vec<_>>>()?;
        let journal = Journal {
            registration: Registration {
                scope,
                name: name.into(),
                root,
                directory: dir_identity,
                members,
                processes,
            },
            stage: Stage::Registered,
        };
        self.save(&journal)?;
        receipt(&journal)
    }

    pub fn preview(&self, scope: &RetirementScope) -> Result<CleanupReceipt> {
        let journal = self.load(scope)?;
        self.validate(&journal)?;
        receipt(&journal)
    }

    pub fn claim(
        &self,
        scope: &RetirementScope,
        facts: &Preservation,
        fence: &mut impl RetirementFence,
    ) -> Result<CleanupReceipt> {
        let mut journal = self.load(scope)?;
        if !matches!(journal.stage, Stage::Registered | Stage::Claimed) {
            return Err(CleanupError::Preserve("invalid claim stage"));
        }
        self.recheck(&journal, facts, fence)?;
        journal.stage = Stage::Claimed;
        self.save(&journal)?;
        receipt(&journal)
    }

    pub fn quarantine(
        &self,
        scope: &RetirementScope,
        facts: &Preservation,
        fence: &mut impl RetirementFence,
    ) -> Result<CleanupReceipt> {
        let mut journal = self.load(scope)?;
        if !matches!(
            journal.stage,
            Stage::Claimed | Stage::Quarantining | Stage::Quarantined
        ) {
            return Err(CleanupError::Preserve("invalid quarantine stage"));
        }
        self.recheck(&journal, facts, fence)?;
        let destination = quarantine_name(scope)?;
        if journal.stage == Stage::Claimed {
            journal.stage = Stage::Quarantining;
            self.save(&journal)?;
        }
        if exists(&self.root, &journal.registration.name)? {
            if exists(&self.root, &destination)? {
                return Err(CleanupError::Preserve("quarantine destination occupied"));
            }
            at::renameat_with(
                &self.root,
                &journal.registration.name,
                &self.root,
                &destination,
                RenameFlags::NOREPLACE,
            )
            .map_err(io::Error::from)?;
            self.root.sync_all()?;
        }
        journal.stage = Stage::Quarantined;
        self.save(&journal)?;
        self.validate(&journal)?;
        receipt(&journal)
    }

    /// Reopening the executor and repeating quarantine/delete recovers durable
    /// intent. Only an interrupted deleting stage permits missing members.
    pub fn delete(
        &self,
        scope: &RetirementScope,
        facts: &Preservation,
        fence: &mut impl RetirementFence,
    ) -> Result<CleanupReceipt> {
        let mut journal = self.load(scope)?;
        if !matches!(
            journal.stage,
            Stage::Quarantined | Stage::Deleting | Stage::Deleted
        ) {
            return Err(CleanupError::Preserve("quarantine required before delete"));
        }
        self.recheck(&journal, facts, fence)?;
        if journal.stage == Stage::Deleted {
            return receipt(&journal);
        }
        journal.stage = Stage::Deleting;
        self.save(&journal)?;
        let name = quarantine_name(scope)?;
        if exists(&self.root, &name)? {
            let directory = file_at(&self.root, &name, DIRECTORY)?;
            for member in journal.registration.members.keys() {
                // Recheck all remaining identities and holds at each effect.
                self.recheck(&journal, facts, fence)?;
                if exists(&directory, member)? {
                    at::unlinkat(&directory, member, AtFlags::empty()).map_err(io::Error::from)?;
                    directory.sync_all()?;
                }
            }
            self.recheck(&journal, facts, fence)?;
            at::unlinkat(&self.root, &name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
            self.root.sync_all()?;
        }
        journal.stage = Stage::Deleted;
        self.save(&journal)?;
        receipt(&journal)
    }

    fn recheck(
        &self,
        journal: &Journal,
        facts: &Preservation,
        fence: &mut impl RetirementFence,
    ) -> Result<()> {
        facts.check()?;
        fence.recheck(
            &journal.registration.scope,
            &registration_digest(&journal.registration)?,
        )?;
        self.validate(journal)?;
        if journal.stage != Stage::Deleted {
            no_process_holds(&journal.registration.processes)?;
        }
        Ok(())
    }
    fn roots_current(&self) -> Result<()> {
        for (path, fd) in [
            (&self.root_path, &self.root),
            (&self.ledger_path, &self.ledger),
        ] {
            let current = open_directory(path)?;
            private(&current)?;
            if identity(&current)? != identity(fd)? {
                return Err(CleanupError::Preserve("root identity changed"));
            }
        }
        Ok(())
    }
    fn validate(&self, journal: &Journal) -> Result<()> {
        self.roots_current()?;
        let r = &journal.registration;
        if identity(&self.root)? != r.root {
            return Err(CleanupError::Preserve("registered root changed"));
        }
        let quarantine = quarantine_name(&r.scope)?;
        let original = exists(&self.root, &r.name)?;
        let quarantined = exists(&self.root, &quarantine)?;
        let name = match journal.stage {
            Stage::Registered | Stage::Claimed if original && !quarantined => &r.name,
            Stage::Quarantining if original != quarantined => {
                if original {
                    &r.name
                } else {
                    &quarantine
                }
            }
            Stage::Quarantined if !original && quarantined => &quarantine,
            Stage::Deleting if !original && quarantined => &quarantine,
            Stage::Deleting | Stage::Deleted if !original && !quarantined => return Ok(()),
            _ => {
                return Err(CleanupError::Preserve(
                    "ambiguous or missing asset location",
                ));
            }
        };
        let directory = file_at(&self.root, name, DIRECTORY)?;
        private(&directory)?;
        if identity(&directory)? != r.directory {
            return Err(CleanupError::Preserve("asset identity or mount changed"));
        }
        let current = inventory(&directory, r.root.mount)?;
        if journal.stage != Stage::Deleting && current.len() != r.members.len() {
            return Err(CleanupError::Preserve("registered contents missing"));
        }
        for (name, member) in current {
            let Some(expected) = r.members.get(&name) else {
                return Err(CleanupError::Preserve("unregistered or ignored content"));
            };
            if member.identity != expected.identity
                || member.digest != expected.digest
                || member.bytes != expected.bytes
            {
                return Err(CleanupError::Preserve("registered content changed"));
            }
        }
        Ok(())
    }
    fn load(&self, scope: &RetirementScope) -> Result<Journal> {
        self.roots_current()?;
        let mut input = file_at(
            &self.ledger,
            &format!("{}.json", journal_id(scope)?),
            OFlags::RDONLY,
        )?;
        private(&input)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut input)
            .take(MAX_JOURNAL + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL {
            return Err(CleanupError::Preserve("oversized journal"));
        }
        let journal: Journal = serde_json::from_slice(&bytes)?;
        if journal.registration.scope != *scope {
            return Err(CleanupError::Preserve("foreign scope"));
        }
        component(&journal.registration.name)?;
        Ok(journal)
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        self.roots_current()?;
        let anchor = fd_path(&self.ledger);
        let mut next = tempfile::NamedTempFile::new_in(&anchor)?;
        serde_json::to_writer(next.as_file_mut(), journal)?;
        next.flush()?;
        next.as_file().sync_all()?;
        let target = anchor.join(format!("{}.json", journal_id(&journal.registration.scope)?));
        next.persist(target).map_err(|error| error.error)?;
        self.ledger.sync_all()?;
        Ok(())
    }
}

fn component(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('.')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_- .".contains(&b))
    {
        return Err(CleanupError::Preserve("unsafe or private asset name"));
    }
    Ok(())
}
fn file_at(parent: &File, name: &str, flags: OFlags) -> Result<File> {
    Ok(at::openat(
        parent,
        name,
        flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(io::Error::from)?
    .into())
}
fn open_directory(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        return Err(CleanupError::Preserve("absolute root required"));
    }
    let mut fd: File = at::open("/", DIRECTORY, Mode::empty())
        .map_err(io::Error::from)?
        .into();
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                fd = at::openat(&fd, name, DIRECTORY, Mode::empty())
                    .map_err(io::Error::from)?
                    .into()
            }
            _ => return Err(CleanupError::Preserve("noncanonical root")),
        }
    }
    Ok(fd)
}
fn private(file: &File) -> Result<()> {
    let meta = file.metadata()?;
    if meta.uid() != rustix::process::getuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(CleanupError::Preserve(
            "object is not caller-owned and private",
        ));
    }
    Ok(())
}
fn identity(file: &File) -> Result<Identity> {
    let stat = at::statx(
        file,
        "",
        AtFlags::EMPTY_PATH,
        StatxFlags::BASIC_STATS | StatxFlags::MNT_ID,
    )
    .map_err(io::Error::from)?;
    if !StatxFlags::from_bits_retain(stat.stx_mask).contains(StatxFlags::MNT_ID) {
        return Err(CleanupError::Preserve("kernel mount identity unavailable"));
    }
    let meta = file.metadata()?;
    Ok(Identity {
        device: meta.dev(),
        inode: meta.ino(),
        mount: stat.stx_mnt_id,
    })
}
fn exists(parent: &File, name: &str) -> Result<bool> {
    match at::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(io::Error::from(error).into()),
    }
}
fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}
fn inventory(directory: &File, mount: u64) -> Result<BTreeMap<String, Member>> {
    let mut members = BTreeMap::new();
    let mut total = 0u64;
    for entry in fs::read_dir(fd_path(directory))? {
        if members.len() >= MAX_FILES {
            return Err(CleanupError::Preserve("too many scratch members"));
        }
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| CleanupError::Preserve("non-UTF8 member"))?;
        component(&name)?;
        let mut input = file_at(directory, &name, OFlags::RDONLY)?;
        private(&input)?;
        let before = input.metadata()?;
        let id = identity(&input)?;
        if !before.is_file() || before.nlink() != 1 || id.mount != mount || before.len() > MAX_BYTES
        {
            return Err(CleanupError::Preserve(
                "linked, nested, special, mounted or oversized member",
            ));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut input)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        total = total.saturating_add(bytes.len() as u64);
        let after = input.metadata()?;
        if total > MAX_BYTES
            || after.len() != before.len()
            || after.mtime_nsec() != before.mtime_nsec()
            || after.mtime() != before.mtime()
            || after.ctime() != before.ctime()
            || after.ctime_nsec() != before.ctime_nsec()
        {
            return Err(CleanupError::Preserve(
                "scratch changed while reading or exceeds limit",
            ));
        }
        members.insert(
            name,
            Member {
                identity: id,
                bytes: bytes.len() as u64,
                digest: format!("{:x}", Sha256::digest(&bytes)),
            },
        );
    }
    Ok(members)
}
fn journal_id(scope: &RetirementScope) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(scope)?)))
}
fn quarantine_name(scope: &RetirementScope) -> Result<String> {
    Ok(format!(".retired-{}", journal_id(scope)?))
}
fn registration_digest(registration: &Registration) -> Result<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(registration)?)
    ))
}
fn receipt(journal: &Journal) -> Result<CleanupReceipt> {
    let bytes: u64 = journal.registration.members.values().map(|v| v.bytes).sum();
    Ok(CleanupReceipt {
        scope: journal.registration.scope.clone(),
        stage: journal.stage,
        registration_digest: registration_digest(&journal.registration)?,
        registered_bytes: bytes.to_string(),
        disposed_bytes: if journal.stage == Stage::Deleted {
            bytes.to_string()
        } else {
            "0".into()
        },
        retained_evidence: true,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessIdentity {
    pid: u32,
    boot_id: String,
    start_ticks: String,
}

fn process_identity(pid: u32) -> Result<ProcessIdentity> {
    if pid == 0 {
        return Err(CleanupError::Preserve("invalid managed PID"));
    }
    let mut stat = String::new();
    File::open(format!("/proc/{pid}/stat"))?
        .take(16 * 1024)
        .read_to_string(&mut stat)?;
    let fields = stat
        .rsplit_once(')')
        .ok_or(CleanupError::Preserve("unknown process identity"))?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let start_ticks = fields
        .get(19)
        .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(CleanupError::Preserve("unknown process start time"))?
        .to_string();
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(ProcessIdentity {
        pid,
        boot_id: boot_id.trim().into(),
        start_ticks,
    })
}

fn no_process_holds(processes: &[ProcessIdentity]) -> Result<()> {
    for process in processes {
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        if boot_id.trim() != process.boot_id {
            return Err(CleanupError::Preserve(
                "registered process boot identity changed",
            ));
        }
        match process_identity(process.pid) {
            // Even PID reuse or a reboot is a hold, never authority to kill or
            // dispose another process's bytes. Reconcile registration instead.
            Ok(_) => {
                return Err(CleanupError::Preserve(
                    "registered process PID is still live",
                ));
            }
            Err(CleanupError::Io(error)) if error.kind() == io::ErrorKind::NotFound => (),
            Err(_) => {
                return Err(CleanupError::Preserve(
                    "registered process identity unknown",
                ));
            }
        }
    }
    Ok(())
}
