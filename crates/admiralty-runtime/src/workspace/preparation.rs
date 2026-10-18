//! Create-only preparation of a local workspace from a pinned Git commit.
//!
//! A [`PreparedWorkspace`] is evidence of what was materialized. Its
//! [`Custody`](registry::Custody) remains `RegisteredOnly`; this module grants
//! no execution, cleanup, or sandbox capability.

use super::registry::{Custody, NodeRef, ValidatedWorkspace, WorkspaceError, WorkspaceSpec};
use admiralty_core::{AttemptId, ProjectId, RootId, TaskId};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use thiserror::Error;

const TRUSTED_GIT: [&str; 3] = ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"];

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
pub fn prepare_workspace(
    request: WorkspacePreparation,
) -> Result<PreparedWorkspace, PreparationError> {
    let git = trusted_git()?;
    validate_destination_name(&request.destination_name)?;
    let source = validate_source(&git, &request)?;
    validate_private_parent(&request.managed_parent)?;
    let parent = fs::canonicalize(&request.managed_parent)?;
    let destination = parent.join(&request.destination_name);
    if source.starts_with(&destination) || destination.starts_with(&source) {
        return Err(PreparationError::OverlappingPaths);
    }
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(PreparationError::DestinationExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    // create_dir is the exclusive claim; a racing creator makes this fail.
    fs::create_dir(&destination)?;
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))?;
    ensure_private_directory(&destination)?;

    let source_text = source.to_str().ok_or(PreparationError::InvalidSource)?;
    let destination_text = destination
        .to_str()
        .ok_or(PreparationError::InvalidDestinationName)?;
    run_git(
        &git,
        None,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--no-hardlinks",
            "--no-checkout",
            "--no-recurse-submodules",
            "--",
            source_text,
            destination_text,
        ],
    )?;

    let git_dir = destination.join(".git");
    let git_dir_metadata =
        fs::symlink_metadata(&git_dir).map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    let uid = rustix::process::geteuid().as_raw();
    if git_dir_metadata.file_type().is_symlink()
        || !git_dir_metadata.is_dir()
        || git_dir_metadata.uid() != uid
    {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    fs::set_permissions(&git_dir, fs::Permissions::from_mode(0o700))?;
    let config_path = git_dir.join("config");
    let config_meta = fs::symlink_metadata(&config_path)
        .map_err(|_| PreparationError::UnsafeDestinationMetadata)?;
    if config_meta.file_type().is_symlink() || !config_meta.is_file() || config_meta.uid() != uid {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))?;
    let config = read_safe_config(&config_path)?;
    if git_dir.join("objects/info/alternates").exists() {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }

    let commit = request.pinned_commit.as_str();
    run_git(
        &git,
        Some(&destination),
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "checkout",
            "--detach",
            "--no-recurse-submodules",
            commit,
        ],
    )?;

    let after_config = read_safe_config(&config_path)?;
    if config != after_config {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    ensure_private_directory(&destination)?;
    let root = git_text(&git, Some(&destination), &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    if root != destination {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let head = git_text(&git, Some(&destination), &["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    if head != request.pinned_commit {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let tree = git_text(&git, Some(&destination), &["rev-parse", "HEAD^{tree}"])?
        .trim()
        .to_owned();
    if !valid_oid_for(&head, head.len()) || !valid_oid_for(&tree, head.len()) {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    let status = git_text(
        &git,
        Some(&destination),
        &[
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
    Ok(PreparedWorkspace {
        workspace,
        head_oid: head,
        tree_oid: tree,
        task_id: request.task_id,
        attempt_id: request.attempt_id,
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
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_PROTOCOL_FROM_USER", "0");
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }
    command
}

fn run_git(git: &Path, cwd: Option<&Path>, args: &[&str]) -> Result<Output, PreparationError> {
    let output = base_command(git, cwd).args(args).output()?;
    if output.status.success() {
        Ok(output)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        Err(PreparationError::Git(detail.chars().take(512).collect()))
    }
}

fn git_text(git: &Path, cwd: Option<&Path>, args: &[&str]) -> Result<String, PreparationError> {
    let output = run_git(git, cwd, args)?;
    String::from_utf8(output.stdout).map_err(|_| PreparationError::Git("non-UTF-8 output".into()))
}

fn validate_source(
    git: &Path,
    request: &WorkspacePreparation,
) -> Result<PathBuf, PreparationError> {
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
    let git_dir = source.join(".git");
    let meta = fs::symlink_metadata(&git_dir).map_err(|_| PreparationError::InvalidSource)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(PreparationError::InvalidSource);
    }
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    let config = read_safe_config(&git_dir.join("config"))
        .map_err(|_| PreparationError::UnsafeSourceMetadata)?;
    let top = git_text(git, Some(&source), &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim()) != source {
        return Err(PreparationError::InvalidSource);
    }
    let status = git_text(
        git,
        Some(&source),
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
    let format = git_text(git, Some(&source), &["rev-parse", "--show-object-format"])?;
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
        Some(&source),
        &["cat-file", "-t", &request.pinned_commit],
    )
    .map_err(|_| PreparationError::CommitUnavailable)?;
    if kind.trim() != "commit" {
        return Err(PreparationError::CommitUnavailable);
    }
    let resolved = git_text(
        git,
        Some(&source),
        &["rev-parse", "--verify", &request.pinned_commit],
    )?;
    if resolved.trim() != request.pinned_commit {
        return Err(PreparationError::InvalidObjectId);
    }
    let config_after = read_safe_config(&git_dir.join("config"))
        .map_err(|_| PreparationError::UnsafeSourceMetadata)?;
    if config != config_after {
        return Err(PreparationError::UnsafeSourceMetadata);
    }
    // Git arguments are passed as individual argv values, never through a shell.
    Ok(source)
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
    if lower.contains("[include") || lower.contains("[filter") || lower.contains("fsmonitor") {
        return Err(PreparationError::UnsafeDestinationMetadata);
    }
    Ok(bytes)
}

fn validate_private_parent(path: &Path) -> Result<(), PreparationError> {
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
    spec.validate()
        .map_err(|_| PreparationError::InvalidManagedParent)?;
    ensure_private_directory(path)
}

fn ensure_private_directory(path: &Path) -> Result<(), PreparationError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| PreparationError::InvalidManagedParent)?;
    let uid = rustix::process::geteuid().as_raw();
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(PreparationError::InvalidManagedParent);
    }
    Ok(())
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
        fs::set_permissions(source.join(".git"), fs::Permissions::from_mode(0o700))
            .expect("private source metadata");
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

    #[test]
    fn creates_private_detached_workspace_with_exact_commit_and_scope() {
        let fixture = fixture();
        let prepared = prepare_workspace(request(&fixture, "attempt-one")).expect("prepare");
        let path = prepared.workspace().workspace.clone();
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
}
