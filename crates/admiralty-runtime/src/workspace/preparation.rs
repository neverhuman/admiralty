//! Create-only preparation of a local workspace from a pinned Git commit.
//!
//! A [`PreparedWorkspace`] is evidence of what was materialized. Its
//! [`Custody`](registry::Custody) remains `RegisteredOnly`; this module grants
//! no execution, cleanup, or sandbox capability.
//!
//! Preparation assumes a trusted local caller and a private mode-0700 parent.
//! Descriptor-relative creation and validation narrow pathname races, but do
//! not isolate against a hostile same-UID process or provide a sandbox. Git
//! work and output are deadline- and size-bounded; the pinned snapshot is
//! limited to 100,000 objects and 512 MiB of object and materialized file data.

use super::registry::{Custody, NodeRef, ValidatedWorkspace, WorkspaceError, WorkspaceSpec};
use admiralty_core::{AttemptId, ProjectId, RootId, TaskId};
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const TRUSTED_GIT: [&str; 3] = ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"];
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_GIT_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_GIT_STDERR: usize = 64 * 1024;
const MAX_TREE_LISTING: usize = 8 * 1024 * 1024;
const MAX_OBJECTS: usize = 100_000;
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const DISK_RESERVE_BYTES: u64 = 16 * 1024 * 1024;
const DISK_BYTES_PER_OBJECT: u64 = 4 * 1024;
const DISK_BYTES_PER_FILE: u64 = 4 * 1024;
const DISK_INODE_RESERVE: u64 = 64;

/// All caller supplied identity and scope for one create-only preparation.
#[derive(Clone, Debug)]
pub struct WorkspacePreparation {
    pub source: PathBuf,
    /// Existing, caller-owned directory with mode 0700.
    pub managed_parent: PathBuf,
    /// A single new directory name beneath `managed_parent`.
    pub destination_name: String,
    pub pinned_commit: String,
    pub node_id: NodeRef,
    pub root_id: RootId,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub owned_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
}

/// Opaque evidence for an actually cloned and detached workspace.
///
/// Fields are private and this type deliberately does not implement
/// `Deserialize`; callers cannot manufacture evidence from persisted data.
#[derive(Debug)]
pub struct PreparedWorkspace {
    workspace: ValidatedWorkspace,
    head_oid: String,
    tree_oid: String,
    task_id: TaskId,
    attempt_id: AttemptId,
    _managed_parent: OwnedFd,
    _workspace_dir: OwnedFd,
}

impl PreparedWorkspace {
    pub fn workspace(&self) -> &ValidatedWorkspace {
        &self.workspace
    }
    pub fn head_oid(&self) -> &str {
        &self.head_oid
    }
    pub fn tree_oid(&self) -> &str {
        &self.tree_oid
    }
    pub fn project_id(&self) -> &ProjectId {
        &self.workspace.project_id
    }
    pub fn root_id(&self) -> &RootId {
        &self.workspace.root_id
    }
    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }
    pub fn attempt_id(&self) -> &AttemptId {
        &self.attempt_id
    }
    pub fn owned_paths(&self) -> &[PathBuf] {
        &self.workspace.owned_paths
    }
    pub fn forbidden_paths(&self) -> &[PathBuf] {
        &self.workspace.forbidden_paths
    }
}

#[derive(Debug, Error)]
pub enum PreparationError {
    #[error("workspace validation failed: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("trusted Git executable is unavailable or unsafe")]
    GitUnavailable,
    #[error("source must be a canonical, self-contained Git working tree")]
    InvalidSource,
    #[error("source has tracked, untracked, ignored, or submodule changes")]
    DirtySource,
    #[error("source Git configuration contains executable or redirecting settings")]
    UnsafeSourceMetadata,
    #[error("pinned commit is not a full object ID for this repository")]
    InvalidObjectId,
    #[error("pinned object is not a commit in the source repository")]
    CommitUnavailable,
    #[error("managed parent must be a canonical caller-owned mode-0700 directory")]
    InvalidManagedParent,
    #[error("destination name must be a safe single path component")]
    InvalidDestinationName,
    #[error("destination already exists")]
    DestinationExists,
    #[error("source and destination must be disjoint")]
    OverlappingPaths,
    #[error("destination Git metadata changed or is unsafe")]
    UnsafeDestinationMetadata,
    #[error("Git operation exceeded its deadline")]
    GitTimeout,
    #[error("Git operation exceeded its output limit")]
    GitOutputLimit,
    #[error("pinned snapshot exceeds the configured object or byte limit")]
    SnapshotLimit,
    #[error("managed filesystem has insufficient admitted free space")]
    InsufficientSpace,
    #[error("Git operation failed: {0}")]
    Git(String),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Materialize a clean local source at exactly `pinned_commit`.
///
/// The destination is created atomically beneath an existing private managed
/// parent. Failed operations preserve the newly created destination for
/// inspection; this function never deletes, resets, cleans, or overwrites it.
/// This operation is for trusted local preparation and does not grant execution
/// or protect against a hostile process running as the same user.
pub fn prepare_workspace(
    request: WorkspacePreparation,
) -> Result<PreparedWorkspace, PreparationError> {
    let git = trusted_git()?;
    validate_destination_name(&request.destination_name)?;
    let source = validate_source(&git, &request)?;
    let (parent_path, parent_fd) = validate_private_parent(&request.managed_parent)?;
    if source.path.starts_with(&parent_path) || parent_path.starts_with(&source.path) {
        return Err(PreparationError::OverlappingPaths);
    }
    let (available, available_inodes) = available_capacity(&parent_fd)?;
    let admitted_bytes = source
        .snapshot_bytes
        .checked_add(source.working_tree_bytes)
        .and_then(|bytes| {
            bytes.checked_add((source.object_count as u64).checked_mul(DISK_BYTES_PER_OBJECT)?)
        })
        .and_then(|bytes| {
            bytes.checked_add((source.file_count as u64).checked_mul(DISK_BYTES_PER_FILE)?)
        })
        .and_then(|bytes| bytes.checked_add(DISK_RESERVE_BYTES))
        .ok_or(PreparationError::SnapshotLimit)?;
    let admitted_inodes = (source.object_count as u64)
        .checked_add(source.file_count as u64)
        .and_then(|count| count.checked_add(DISK_INODE_RESERVE))
        .ok_or(PreparationError::SnapshotLimit)?;
    if available < admitted_bytes || available_inodes < admitted_inodes {
        return Err(PreparationError::InsufficientSpace);
    }
    ensure_path_points_to_fd(&parent_path, &parent_fd)?;
    match statat_entry(&parent_fd, &request.destination_name) {
        Ok(_) => return Err(PreparationError::DestinationExists),
        Err(error) if error == rustix::io::Errno::NOENT => {}
        Err(error) => return Err(io::Error::from(error).into()),
    }
    rustix::fs::mkdirat(
        &parent_fd,
        request.destination_name.as_str(),
        rustix::fs::Mode::from_bits_truncate(0o700),
    )
    .map_err(io::Error::from)?;
    let destination_fd = rustix::fs::openat(
        &parent_fd,
        request.destination_name.as_str(),
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(io::Error::from)?;
    rustix::fs::fchmod(&destination_fd, rustix::fs::Mode::from_bits_truncate(0o700))
        .map_err(io::Error::from)?;
    ensure_private_fd(&destination_fd)?;
    ensure_same_directory(&parent_fd, &request.destination_name, &destination_fd)?;
    let destination = parent_path.join(&request.destination_name);
    let destination_cwd = fd_path(&destination_fd);
    run_git(&git, Some(&destination_cwd), &["init", "--quiet"])?;
    let git_dir = destination_cwd.join(".git");
    let git_dir_meta =
        fs::symlink_metadata(&git_dir).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    if git_dir_meta.file_type().is_symlink()
        || !git_dir_meta.is_dir()
        || git_dir_meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    fs::set_permissions(&git_dir, fs::Permissions::from_mode(0o700))?;
    let destination_config = git_dir.join("config");
    let destination_config_meta = fs::symlink_metadata(&destination_config)
        .map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    if destination_config_meta.file_type().is_symlink()
        || !destination_config_meta.is_file()
        || destination_config_meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    fs::set_permissions(&destination_config, fs::Permissions::from_mode(0o600))?;
    let initial_config = read_safe_config(&destination_config)?;
    let source_path_text = source.path.to_string_lossy().into_owned();

    run_git_with_input(
        &git,
        Some(&destination_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "--quiet",
            "--depth=1",
            "--no-tags",
            "--no-recurse-submodules",
            &source_path_text,
            &request.pinned_commit,
        ],
        None,
        GIT_TRANSFER_TIMEOUT,
        MAX_GIT_OUTPUT,
    )?;
    run_git(
        &git,
        Some(&destination_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "checkout",
            "--quiet",
            "--detach",
            "--no-recurse-submodules",
            &request.pinned_commit,
        ],
    )?;

    let git_dir = destination_cwd.join(".git");
    let git_dir_meta =
        fs::symlink_metadata(&git_dir).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    if git_dir_meta.file_type().is_symlink() || !git_dir_meta.is_dir() || git_dir_meta.uid() != uid
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let config_path = git_dir.join("config");
    let config = read_safe_config(&config_path)?;
    if config != initial_config {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    if has_incomplete_object_metadata(&git_dir)? {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    if git_text(
        &git,
        Some(&destination_cwd),
        &["rev-parse", "--is-shallow-repository"],
    )?
    .trim()
        != "true"
        || git_text(
            &git,
            Some(&destination_cwd),
            &["rev-list", "--count", "HEAD"],
        )?
        .trim()
            != "1"
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let object_stats = git_text(&git, Some(&destination_cwd), &["count-objects", "-v"])?;
    validate_object_store_stats(&object_stats, source.snapshot_bytes, source.object_count)?;
    let after_config = read_safe_config(&config_path)?;
    if config != after_config {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    ensure_private_fd(&destination_fd)?;
    ensure_same_directory(&parent_fd, &request.destination_name, &destination_fd)?;
    ensure_source_unchanged(&git, &source)?;
    let head = git_text(&git, Some(&destination_cwd), &["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    if head != request.pinned_commit {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let tree = git_text(&git, Some(&destination_cwd), &["rev-parse", "HEAD^{tree}"])?
        .trim()
        .to_owned();
    if tree != source.tree_oid || !valid_oid_for(&head, source.oid_len) {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let status = git_text(
        &git,
        Some(&destination_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    if !status.is_empty() {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }

    let spec = WorkspaceSpec {
        node_id: request.node_id,
        root_id: request.root_id,
        project_id: request.project_id,
        workspace: destination,
        owned_paths: request.owned_paths,
        forbidden_paths: request.forbidden_paths,
    };
    let workspace = spec.validate()?;
    if workspace.custody != Custody::RegisteredOnly {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    ensure_path_points_to_fd(&parent_path, &parent_fd)?;
    ensure_same_directory(&parent_fd, &request.destination_name, &destination_fd)?;
    Ok(PreparedWorkspace {
        workspace,
        head_oid: head,
        tree_oid: tree,
        task_id: request.task_id,
        attempt_id: request.attempt_id,
        _managed_parent: parent_fd,
        _workspace_dir: destination_fd,
    })
}

fn trusted_git() -> Result<PathBuf, PreparationError> {
    let uid = rustix::process::geteuid().as_raw();
    for candidate in TRUSTED_GIT {
        let path = Path::new(candidate);
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || meta.uid() != 0 && meta.uid() != uid
            || meta.mode() & 0o022 != 0
            || meta.mode() & 0o111 == 0
        {
            continue;
        }
        let Ok(canonical) = fs::canonicalize(path) else {
            continue;
        };
        if canonical == path {
            return Ok(canonical);
        }
    }
    Err(PreparationError::GitUnavailable)
}

fn base_command(git: &Path, cwd: Option<&Path>) -> Command {
    let mut command = Command::new(git);
    command.env_clear();
    command
        .env("HOME", "/nonexistent")
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_ALLOW_PROTOCOL", "file");
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }
    command
}

fn run_git(git: &Path, cwd: Option<&Path>, args: &[&str]) -> Result<Output, PreparationError> {
    run_git_with_input(git, cwd, args, None, GIT_TIMEOUT, MAX_GIT_OUTPUT)
}

fn run_git_with_input(
    git: &Path,
    cwd: Option<&Path>,
    args: &[&str],
    input: Option<&[u8]>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Output, PreparationError> {
    run_git_with_options(git, cwd, args, input, timeout, output_limit)
}

fn run_git_with_options(
    git: &Path,
    cwd: Option<&Path>,
    args: &[&str],
    input: Option<&[u8]>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Output, PreparationError> {
    let mut command = base_command(git, cwd);
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PreparationError::Git("missing stdout pipe".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PreparationError::Git("missing stderr pipe".into()))?;
    let oversized = Arc::new(AtomicBool::new(false));
    let stdout_reader = read_bounded(stdout, output_limit, Arc::clone(&oversized));
    let stderr_reader = read_bounded(stderr, MAX_GIT_STDERR, Arc::clone(&oversized));
    let input_writer = if let Some(bytes) = input {
        let Some(mut pipe) = child.stdin.take() else {
            terminate_git_group(&mut child);
            return Err(PreparationError::Git("missing stdin pipe".into()));
        };
        let bytes = bytes.to_vec();
        Some(thread::spawn(move || pipe.write_all(&bytes)))
    } else {
        None
    };
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        if oversized.load(Ordering::Relaxed) {
            terminate_git_group(&mut child);
            break child.wait()?;
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate_git_group(&mut child);
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| PreparationError::Git("stdout reader panicked".into()))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| PreparationError::Git("stderr reader panicked".into()))??;
    if let Some(writer) = input_writer {
        writer
            .join()
            .map_err(|_| PreparationError::Git("stdin writer panicked".into()))?
            .map_err(PreparationError::Io)?;
    }
    if timed_out {
        return Err(PreparationError::GitTimeout);
    }
    if oversized.load(Ordering::Relaxed) {
        return Err(PreparationError::GitOutputLimit);
    }
    let output = Output {
        status,
        stdout,
        stderr,
    };
    if output.status.success() {
        Ok(output)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        let operation = args.join(" ");
        Err(PreparationError::Git(format!(
            "{}: {}",
            operation.chars().take(128).collect::<String>(),
            detail.chars().take(320).collect::<String>()
        )))
    }
}

fn read_bounded<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    oversized: Arc<AtomicBool>,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut retained = Vec::with_capacity(limit.min(8192));
        let mut buffer = [0u8; 8192];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let available = limit.saturating_sub(retained.len());
            let keep = count.min(available);
            retained.extend_from_slice(&buffer[..keep]);
            if keep < count {
                oversized.store(true, Ordering::Relaxed);
            }
        }
        Ok(retained)
    })
}

fn terminate_git_group(child: &mut Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    if let Ok(raw_pid) = i32::try_from(child.id())
        && let Some(pid) = Pid::from_raw(raw_pid)
        && raw_pid > 1
    {
        let _ = kill_process_group(pid, Signal::KILL);
    }
    let _ = child.kill();
}

fn git_text(git: &Path, cwd: Option<&Path>, args: &[&str]) -> Result<String, PreparationError> {
    let output = run_git(git, cwd, args)?;
    String::from_utf8(output.stdout).map_err(|_| PreparationError::Git("non-UTF-8 output".into()))
}

struct ValidatedSource {
    path: PathBuf,
    source_fd: OwnedFd,
    config: Vec<u8>,
    source_head: String,
    tree_oid: String,
    oid_len: usize,
    snapshot_bytes: u64,
    object_count: usize,
    working_tree_bytes: u64,
    file_count: usize,
}

fn validate_source(
    git: &Path,
    request: &WorkspacePreparation,
) -> Result<ValidatedSource, PreparationError> {
    let source = request.source.as_path();
    if !source.is_absolute() {
        return Err(PreparationError::InvalidSource);
    }
    source.to_str().ok_or(PreparationError::InvalidSource)?;
    let validated = WorkspaceSpec {
        node_id: request.node_id.clone(),
        root_id: request.root_id.clone(),
        project_id: request.project_id.clone(),
        workspace: source.to_path_buf(),
        owned_paths: request.owned_paths.clone(),
        forbidden_paths: request.forbidden_paths.clone(),
    }
    .validate()
    .map_err(|_| PreparationError::InvalidSource)?;
    let source = validated.workspace;
    let source_fd = open_directory_chain(&source).map_err(|_| PreparationError::InvalidSource)?;
    let source_cwd = fd_path(&source_fd);
    let git_dir = source_cwd.join(".git");
    let meta = fs::symlink_metadata(&git_dir).map_err(|_| PreparationError::InvalidSource)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(PreparationError::InvalidSource);
    }
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    let config = read_safe_config(&git_dir.join("config"))
        .map_err(|_| PreparationError::UnsafeSourceMetadata)?;
    validate_source_git_metadata(&git_dir, &config)?;
    let top = git_text(git, Some(&source_cwd), &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim()) != source {
        return Err(PreparationError::InvalidSource);
    }
    let status = git_text(
        git,
        Some(&source_cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    )?;
    if !status.is_empty() {
        return Err(PreparationError::DirtySource);
    }
    let format = git_text(
        git,
        Some(&source_cwd),
        &["rev-parse", "--show-object-format"],
    )?;
    let oid_len = match format.trim() {
        "sha1" => 40,
        "sha256" => 64,
        _ => return Err(PreparationError::InvalidObjectId),
    };
    if !valid_oid_for(&request.pinned_commit, oid_len) {
        return Err(PreparationError::InvalidObjectId);
    }
    let kind = git_text(
        git,
        Some(&source_cwd),
        &["cat-file", "-t", &request.pinned_commit],
    )
    .map_err(|_| PreparationError::CommitUnavailable)?;
    if kind.trim() != "commit" {
        return Err(PreparationError::CommitUnavailable);
    }
    let resolved = git_text(
        git,
        Some(&source_cwd),
        &["rev-parse", "--verify", &request.pinned_commit],
    )?;
    if resolved.trim() != request.pinned_commit {
        return Err(PreparationError::InvalidObjectId);
    }
    let source_head = git_text(git, Some(&source_cwd), &["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    let tree_oid = git_text(
        git,
        Some(&source_cwd),
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{tree}}", request.pinned_commit),
        ],
    )?
    .trim()
    .to_owned();
    let listing = run_git_with_input(
        git,
        Some(&source_cwd),
        &["ls-tree", "-r", "-t", "-z", "--full-tree", &tree_oid],
        None,
        GIT_TIMEOUT,
        MAX_TREE_LISTING,
    )?
    .stdout;
    let (snapshot_bytes, object_count, working_tree_bytes, file_count) = measure_object_closure(
        git,
        &source_cwd,
        &request.pinned_commit,
        &tree_oid,
        &listing,
        oid_len,
    )?;
    let config_after = read_safe_config(&git_dir.join("config"))
        .map_err(|_| PreparationError::UnsafeSourceMetadata)?;
    if config != config_after {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    if snapshot_bytes > MAX_SNAPSHOT_BYTES || working_tree_bytes > MAX_SNAPSHOT_BYTES {
        return Err(PreparationError::SnapshotLimit);
    }
    Ok(ValidatedSource {
        path: source,
        source_fd,
        config,
        source_head,
        tree_oid,
        oid_len,
        snapshot_bytes,
        object_count,
        working_tree_bytes,
        file_count,
    })
}

fn read_safe_config(path: &Path) -> Result<Vec<u8>, PreparationError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let bytes = fs::read(path)?;
    let lower = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if lower.contains("[include")
        || lower.contains("[filter")
        || lower.contains("fsmonitor")
        || lower.contains("promisor")
        || lower.contains("partialclone")
        || lower.contains("protocol.")
        || lower.contains("insteadof")
        || lower.contains("credential.helper")
        || lower.contains("sshcommand")
        || lower.contains("ext::")
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(bytes)
}

fn validate_source_git_metadata(git_dir: &Path, config: &[u8]) -> Result<(), PreparationError> {
    let lower = String::from_utf8_lossy(config).to_ascii_lowercase();
    if lower.contains("remote.") && lower.contains("promisor")
        || lower.contains("partialclone")
        || lower.contains("partialclonefilter")
    {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    for relative in [
        Path::new("shallow"),
        Path::new("objects/info/alternates"),
        Path::new("objects/info/http-alternates"),
    ] {
        if path_entry_exists(&git_dir.join(relative))? {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
    }
    for relative in [Path::new("HEAD"), Path::new("index")] {
        let metadata = fs::symlink_metadata(git_dir.join(relative))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o022 != 0
        {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
    }
    let objects = git_dir.join("objects");
    check_owned_git_directory(&objects)?;
    let mut entries_seen = 0usize;
    for entry in fs::read_dir(&objects)? {
        entries_seen += 1;
        if entries_seen > MAX_OBJECTS {
            return Err(PreparationError::SnapshotLimit);
        }
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        let path = entry.path();
        if name_text == "info"
            || name_text == "pack"
            || (name_text.len() == 2 && is_lower_hex(&name_text))
        {
            check_owned_git_directory(&path)?;
            if name_text == "pack" {
                for pack_entry in fs::read_dir(&path)? {
                    entries_seen += 1;
                    if entries_seen > MAX_OBJECTS {
                        return Err(PreparationError::SnapshotLimit);
                    }
                    let pack_entry = pack_entry?;
                    let metadata = fs::symlink_metadata(pack_entry.path())?;
                    if pack_entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".promisor")
                        || metadata.file_type().is_symlink()
                        || !metadata.is_file()
                        || metadata.uid() != rustix::process::geteuid().as_raw()
                        || metadata.mode() & 0o022 != 0
                    {
                        return Err(PreparationError::UnsafeSourceMetadata);
                    }
                }
            } else if name_text != "info" {
                for object in fs::read_dir(&path)? {
                    entries_seen += 1;
                    if entries_seen > MAX_OBJECTS {
                        return Err(PreparationError::SnapshotLimit);
                    }
                    let object = object?;
                    let metadata = fs::symlink_metadata(object.path())?;
                    let object_name = object.file_name();
                    let object_name = object_name.to_string_lossy();
                    if metadata.file_type().is_symlink()
                        || !metadata.is_file()
                        || metadata.uid() != rustix::process::geteuid().as_raw()
                        || metadata.mode() & 0o022 != 0
                        || object_name.len() != 38
                        || !is_lower_hex(&object_name)
                    {
                        return Err(PreparationError::UnsafeSourceMetadata);
                    }
                }
            }
        } else {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
    }
    Ok(())
}

fn check_owned_git_directory(path: &Path) -> Result<(), PreparationError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn has_incomplete_object_metadata(git_dir: &Path) -> Result<bool, PreparationError> {
    if path_entry_exists(&git_dir.join("objects/info/alternates"))?
        || path_entry_exists(&git_dir.join("objects/info/http-alternates"))?
    {
        return Ok(true);
    }
    let pack_dir = git_dir.join("objects/pack");
    match fs::symlink_metadata(&pack_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => return Ok(true),
        Ok(_) => {
            for entry in fs::read_dir(pack_dir)? {
                if entry?.file_name().to_string_lossy().ends_with(".promisor") {
                    return Ok(true);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(false)
}

fn path_entry_exists(path: &Path) -> Result<bool, PreparationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn measure_object_closure(
    git: &Path,
    source_cwd: &Path,
    commit: &str,
    tree: &str,
    listing: &[u8],
    oid_len: usize,
) -> Result<(u64, usize, u64, usize), PreparationError> {
    if listing.len() > MAX_TREE_LISTING {
        return Err(PreparationError::SnapshotLimit);
    }
    let mut objects = std::collections::BTreeSet::new();
    let mut blob_references = std::collections::BTreeMap::<String, usize>::new();
    let mut file_count = 0usize;
    objects.insert(commit.to_owned());
    objects.insert(tree.to_owned());
    for record in listing
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        let header = std::str::from_utf8(&record[..tab])
            .map_err(|_| PreparationError::UnsafeSourceMetadata)?;
        let mut fields = header.split_ascii_whitespace();
        let _mode = fields
            .next()
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        let kind = fields
            .next()
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        let oid = fields
            .next()
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        if fields.next().is_some() || !valid_oid_for(oid, oid_len) || kind == "commit" {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
        if !matches!(kind, "blob" | "tree") {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
        if kind == "blob" {
            file_count = file_count
                .checked_add(1)
                .ok_or(PreparationError::SnapshotLimit)?;
            if file_count > MAX_OBJECTS {
                return Err(PreparationError::SnapshotLimit);
            }
            let multiplicity = blob_references.entry(oid.to_owned()).or_default();
            *multiplicity = multiplicity
                .checked_add(1)
                .ok_or(PreparationError::SnapshotLimit)?;
        }
        objects.insert(oid.to_owned());
        if objects.len() > MAX_OBJECTS {
            return Err(PreparationError::SnapshotLimit);
        }
    }
    let mut input = Vec::with_capacity(objects.len().saturating_mul(oid_len + 1));
    for oid in &objects {
        input.extend_from_slice(oid.as_bytes());
        input.push(b'\n');
    }
    let output = run_git_with_input(
        git,
        Some(source_cwd),
        &[
            "cat-file",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
        Some(&input),
        GIT_TIMEOUT,
        MAX_GIT_OUTPUT,
    )?;
    let mut total = 0u64;
    let mut count = 0usize;
    let mut sizes = std::collections::BTreeMap::new();
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let line = std::str::from_utf8(line).map_err(|_| PreparationError::UnsafeSourceMetadata)?;
        let mut fields = line.split_ascii_whitespace();
        let oid = fields
            .next()
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        let kind = fields
            .next()
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        let size = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(PreparationError::UnsafeSourceMetadata)?;
        if fields.next().is_some()
            || !objects.contains(oid)
            || !matches!(kind, "blob" | "tree" | "commit")
        {
            return Err(PreparationError::UnsafeSourceMetadata);
        }
        total = total
            .checked_add(size)
            .ok_or(PreparationError::SnapshotLimit)?;
        sizes.insert(oid.to_owned(), size);
        count += 1;
        if total > MAX_SNAPSHOT_BYTES || count > MAX_OBJECTS {
            return Err(PreparationError::SnapshotLimit);
        }
    }
    if count != objects.len() {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    let working_tree_bytes = blob_references
        .iter()
        .try_fold(0u64, |sum, (oid, copies)| {
            let size = sizes
                .get(oid)
                .copied()
                .ok_or(PreparationError::UnsafeSourceMetadata)?;
            sum.checked_add(
                size.checked_mul(*copies as u64)
                    .ok_or(PreparationError::SnapshotLimit)?,
            )
            .ok_or(PreparationError::SnapshotLimit)
        })?;
    Ok((total, count, working_tree_bytes, file_count))
}

fn ensure_source_unchanged(git: &Path, source: &ValidatedSource) -> Result<(), PreparationError> {
    let cwd = fd_path(&source.source_fd);
    let config_path = cwd.join(".git/config");
    let config_after =
        read_safe_config(&config_path).map_err(|_| PreparationError::UnsafeSourceMetadata)?;
    if config_after != source.config {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    let status = git_text(
        git,
        Some(&cwd),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    )?;
    let source_head = git_text(git, Some(&cwd), &["rev-parse", "HEAD"])?;
    if !status.is_empty() || source_head.trim() != source.source_head {
        return Err(PreparationError::DirtySource);
    }
    Ok(())
}

fn validate_object_store_stats(
    stats: &str,
    expected_bytes: u64,
    expected_objects: usize,
) -> Result<(), PreparationError> {
    let values = stats
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()));
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in values {
        map.insert(
            key,
            value
                .parse::<u64>()
                .map_err(|_| PreparationError::UnsafeDestinationMetadata)?,
        );
    }
    let objects = map
        .get("count")
        .copied()
        .unwrap_or(0)
        .checked_add(map.get("in-pack").copied().unwrap_or(0))
        .ok_or(PreparationError::SnapshotLimit)?;
    let loose_kib = map.get("size").copied().unwrap_or(0);
    let packed_kib = map.get("size-pack").copied().unwrap_or(0);
    let stored_bytes = loose_kib
        .checked_add(packed_kib)
        .and_then(|kib| kib.checked_mul(1024))
        .ok_or(PreparationError::SnapshotLimit)?;
    if objects > expected_objects as u64
        || stored_bytes > expected_bytes.saturating_add(2 * 1024 * 1024)
        || map.get("garbage").copied().unwrap_or(0) != 0
        || map.get("size-garbage").copied().unwrap_or(0) != 0
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(())
}

fn validate_private_parent(path: &Path) -> Result<(PathBuf, OwnedFd), PreparationError> {
    if !path.is_absolute() {
        return Err(PreparationError::InvalidManagedParent);
    }
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("preparation")?,
        root_id: RootId::new("preparation").map_err(|_| PreparationError::InvalidManagedParent)?,
        project_id: ProjectId::new("preparation")
            .map_err(|_| PreparationError::InvalidManagedParent)?,
        workspace: path.to_path_buf(),
        owned_paths: Vec::new(),
        forbidden_paths: Vec::new(),
    };
    let validated = spec
        .validate()
        .map_err(|_| PreparationError::InvalidManagedParent)?;
    let canonical = validated.workspace;
    let directory =
        open_directory_chain(&canonical).map_err(|_| PreparationError::InvalidManagedParent)?;
    ensure_private_fd(&directory).map_err(|_| PreparationError::InvalidManagedParent)?;
    Ok((canonical, directory))
}

fn open_directory_chain(path: &Path) -> Result<OwnedFd, PreparationError> {
    let mut directory = rustix::fs::open(
        "/",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(io::Error::from)?;
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = rustix::fs::openat(
                    &directory,
                    name,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(io::Error::from)?;
            }
            _ => return Err(PreparationError::InvalidManagedParent),
        }
    }
    Ok(directory)
}

fn fd_path(fd: &OwnedFd) -> PathBuf {
    #[cfg(target_os = "linux")]
    let prefix = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let prefix = "/dev/fd";
    PathBuf::from(format!("{prefix}/{}", fd.as_raw_fd()))
}

fn ensure_private_fd(fd: &OwnedFd) -> Result<(), PreparationError> {
    let metadata = rustix::fs::fstat(fd).map_err(io::Error::from)?;
    let is_directory =
        rustix::fs::FileType::from_raw_mode(metadata.st_mode) == rustix::fs::FileType::Directory;
    if !is_directory
        || metadata.st_uid != rustix::process::geteuid().as_raw()
        || metadata.st_mode & 0o7777 != 0o700
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(())
}

fn statat_entry(parent: &OwnedFd, name: &str) -> Result<rustix::fs::Stat, rustix::io::Errno> {
    rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)
}

fn ensure_same_directory(
    parent: &OwnedFd,
    name: &str,
    directory: &OwnedFd,
) -> Result<(), PreparationError> {
    let path_stat =
        statat_entry(parent, name).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    let fd_stat = rustix::fs::fstat(directory).map_err(io::Error::from)?;
    if path_stat.st_dev != fd_stat.st_dev
        || path_stat.st_ino != fd_stat.st_ino
        || path_stat.st_uid != fd_stat.st_uid
        || rustix::fs::FileType::from_raw_mode(path_stat.st_mode) != rustix::fs::FileType::Directory
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(())
}

fn ensure_path_points_to_fd(path: &Path, fd: &OwnedFd) -> Result<(), PreparationError> {
    let current =
        open_directory_chain(path).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    let expected_stat = rustix::fs::fstat(fd).map_err(io::Error::from)?;
    let current_stat = rustix::fs::fstat(&current).map_err(io::Error::from)?;
    if expected_stat.st_dev != current_stat.st_dev || expected_stat.st_ino != current_stat.st_ino {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(())
}

fn available_capacity(directory: &OwnedFd) -> Result<(u64, u64), PreparationError> {
    let usage = rustix::fs::fstatvfs(directory).map_err(io::Error::from)?;
    let bytes = usage
        .f_bavail
        .checked_mul(usage.f_frsize)
        .ok_or(PreparationError::SnapshotLimit)?;
    Ok((bytes, usage.f_favail))
}

fn validate_destination_name(name: &str) -> Result<(), PreparationError> {
    if name.is_empty()
        || name.len() > 128
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(PreparationError::InvalidDestinationName);
    }
    Ok(())
}

fn valid_oid_for(oid: &str, len: usize) -> bool {
    oid.len() == len
        && oid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    use tempfile::TempDir;

    struct Fixture {
        _temp: TempDir,
        source: PathBuf,
        parent: PathBuf,
        commit: String,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let parent = temp.path().join("managed");
        fs::create_dir(&source).expect("source dir");
        fs::create_dir(&parent).expect("managed parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("private mode");
        let git = trusted_git().expect("trusted git");
        test_git(&git, &source, &["init", "-q"]);
        test_git(&git, &source, &["config", "user.name", "test"]);
        test_git(
            &git,
            &source,
            &["config", "user.email", "test@example.invalid"],
        );
        fs::set_permissions(
            source.join(".git/config"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("private source config");
        fs::write(source.join("tracked.txt"), "pinned content\n").expect("write source");
        test_git(&git, &source, &["add", "tracked.txt"]);
        test_git(&git, &source, &["commit", "-qm", "fixture"]);
        let commit = git_text(&git, Some(&source), &["rev-parse", "HEAD"])
            .expect("read commit")
            .trim()
            .to_owned();
        privatize_git_metadata(&source.join(".git")).expect("private source metadata");
        Fixture {
            _temp: temp,
            source,
            parent,
            commit,
        }
    }

    fn request(fixture: &Fixture, name: &str) -> WorkspacePreparation {
        WorkspacePreparation {
            source: fixture.source.clone(),
            managed_parent: fixture.parent.clone(),
            destination_name: name.into(),
            pinned_commit: fixture.commit.clone(),
            node_id: NodeRef::new("node-a").expect("node id"),
            root_id: RootId::new("root-a").expect("root id"),
            project_id: ProjectId::new("project-a").expect("project id"),
            task_id: TaskId::new("task-a").expect("task id"),
            attempt_id: AttemptId::new("attempt-a").expect("attempt id"),
            owned_paths: vec![PathBuf::from("tracked.txt")],
            forbidden_paths: vec![PathBuf::from("secrets")],
        }
    }

    fn test_git(git: &Path, cwd: &Path, args: &[&str]) {
        let output = base_command(git, Some(cwd))
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn privatize_git_metadata(path: &Path) -> std::io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symlink in fixture Git metadata",
            ));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                privatize_git_metadata(&entry?.path())?;
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        } else if metadata.is_file() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    #[test]
    fn creates_private_detached_workspace_with_exact_commit_and_scope() {
        let fixture = fixture();
        let prepared = prepare_workspace(request(&fixture, "attempt-one")).expect("prepare");
        let path = prepared.workspace().workspace.clone();
        let git = trusted_git().expect("trusted git");
        assert_eq!(prepared.head_oid(), fixture.commit);
        assert_eq!(prepared.workspace().custody, Custody::RegisteredOnly);
        assert_eq!(prepared.project_id().as_str(), "project-a");
        assert_eq!(prepared.root_id().as_str(), "root-a");
        assert_eq!(prepared.task_id().as_str(), "task-a");
        assert_eq!(prepared.attempt_id().as_str(), "attempt-a");
        assert_eq!(prepared.owned_paths(), &[PathBuf::from("tracked.txt")]);
        assert_eq!(prepared.forbidden_paths(), &[PathBuf::from("secrets")]);
        assert_eq!(
            fs::read(path.join("tracked.txt")).expect("read clone"),
            b"pinned content\n"
        );
        assert_eq!(
            fs::metadata(&path).expect("destination metadata").mode() & 0o777,
            0o700
        );
        assert!(!path.join(".git/objects/info/alternates").exists());
        assert_eq!(
            git_text(&git, Some(&path), &["rev-list", "--count", "HEAD"])
                .expect("shallow history count")
                .trim(),
            "1"
        );
        assert_eq!(
            git_text(&git, Some(&path), &["rev-parse", "--is-shallow-repository"])
                .expect("shallow repository state")
                .trim(),
            "true"
        );
    }

    #[test]
    fn refuses_duplicate_destination_without_changing_existing_files() {
        let fixture = fixture();
        let prepared = prepare_workspace(request(&fixture, "attempt-one")).expect("prepare");
        let file = prepared.workspace().workspace.join("tracked.txt");
        fs::write(&file, "caller data\n").expect("write destination");
        assert!(matches!(
            prepare_workspace(request(&fixture, "attempt-one")),
            Err(PreparationError::DestinationExists)
        ));
        assert_eq!(fs::read(file).expect("preserved file"), b"caller data\n");
    }

    #[test]
    fn rejects_dirty_or_ignored_source_and_short_object_id() {
        let fixture = fixture();
        fs::write(fixture.source.join("untracked"), "ambiguous\n").expect("untracked file");
        assert!(matches!(
            prepare_workspace(request(&fixture, "dirty")),
            Err(PreparationError::DirtySource)
        ));
        fs::remove_file(fixture.source.join("untracked")).expect("remove fixture file");
        let mut invalid = request(&fixture, "bad-oid");
        invalid.pinned_commit = fixture.commit[..12].to_owned();
        assert!(matches!(
            prepare_workspace(invalid),
            Err(PreparationError::InvalidObjectId)
        ));
    }

    #[test]
    fn rejects_symlink_source_destination_escape_and_scope_escape() {
        let fixture = fixture();
        let alias = fixture.parent.join("source-link");
        std::os::unix::fs::symlink(&fixture.source, &alias).expect("source symlink");
        let mut linked = request(&fixture, "linked");
        linked.source = alias;
        assert!(matches!(
            prepare_workspace(linked),
            Err(PreparationError::InvalidSource)
        ));

        let destination_link = fixture.parent.join("destination-link");
        std::os::unix::fs::symlink(&fixture.source, &destination_link)
            .expect("destination symlink");
        assert!(matches!(
            prepare_workspace(request(&fixture, "destination-link")),
            Err(PreparationError::DestinationExists)
        ));

        let mut escaped = request(&fixture, "../escape");
        assert!(matches!(
            prepare_workspace(escaped.clone()),
            Err(PreparationError::InvalidDestinationName)
        ));
        escaped.destination_name = "scope-escape".into();
        escaped.owned_paths = vec![PathBuf::from("../outside")];
        assert!(prepare_workspace(escaped).is_err());
    }

    #[test]
    fn rejects_source_configuration_that_can_inject_filters() {
        let fixture = fixture();
        let git = trusted_git().expect("trusted git");
        test_git(
            &git,
            &fixture.source,
            &["config", "filter.evil.smudge", "touch injected"],
        );
        assert!(matches!(
            prepare_workspace(request(&fixture, "unsafe-config")),
            Err(PreparationError::UnsafeSourceMetadata)
        ));
        assert!(!fixture.source.join("injected").exists());
    }

    #[test]
    fn rejects_alternates_shallow_and_promisor_source_metadata() {
        let alternate_fixture = fixture();
        let info = alternate_fixture.source.join(".git/objects/info");
        fs::create_dir_all(&info).expect("object info directory");
        fs::write(info.join("alternates"), "/foreign/objects\n").expect("alternates file");
        assert!(matches!(
            prepare_workspace(request(&alternate_fixture, "alternate-source")),
            Err(PreparationError::UnsafeSourceMetadata)
        ));

        let promisor_fixture = fixture();
        let pack = promisor_fixture.source.join(".git/objects/pack");
        fs::create_dir_all(&pack).expect("pack directory");
        fs::set_permissions(&pack, fs::Permissions::from_mode(0o700))
            .expect("private pack directory");
        fs::write(pack.join("pack-fixture.promisor"), "").expect("promisor marker");
        assert!(matches!(
            prepare_workspace(request(&promisor_fixture, "promisor-source")),
            Err(PreparationError::UnsafeSourceMetadata)
        ));

        let shallow_fixture = fixture();
        fs::write(shallow_fixture.source.join(".git/shallow"), "incomplete\n")
            .expect("shallow marker");
        assert!(matches!(
            prepare_workspace(request(&shallow_fixture, "shallow-source")),
            Err(PreparationError::UnsafeSourceMetadata)
        ));
    }

    #[test]
    fn output_capture_is_bounded_and_reports_overflow() {
        let oversized = Arc::new(AtomicBool::new(false));
        let captured = read_bounded(
            std::io::Cursor::new(b"abcdef".to_vec()),
            3,
            Arc::clone(&oversized),
        )
        .join()
        .expect("reader thread")
        .expect("read output");
        assert_eq!(captured, b"abc");
        assert!(oversized.load(Ordering::Relaxed));
    }

    #[test]
    fn parent_descriptor_keeps_creation_anchored_after_path_replacement() {
        let fixture = fixture();
        let parent_fd = open_directory_chain(&fixture.parent).expect("open parent handle");
        let moved = fixture._temp.path().join("managed-moved");
        fs::rename(&fixture.parent, &moved).expect("rename private parent");
        std::os::unix::fs::symlink(&fixture.source, &fixture.parent)
            .expect("replace path with symlink");
        rustix::fs::mkdirat(
            &parent_fd,
            "pinned-child",
            rustix::fs::Mode::from_bits_truncate(0o700),
        )
        .expect("create beneath pinned parent");
        assert!(moved.join("pinned-child").is_dir());
        assert!(!fixture.source.join("pinned-child").exists());
    }
}
