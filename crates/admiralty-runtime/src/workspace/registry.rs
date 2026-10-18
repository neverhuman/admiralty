//! Read-only validation and observation of registered workspace custody.
//!
//! Registration records identity and observed Git metadata. It is not an
//! execution, mutation, or cleanup capability.

use admiralty_core::{ProjectId, RootId};
use std::{
    env, fmt, fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

pub const MAX_SCOPE_PATHS: usize = 1024;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_GIT_POINTER_BYTES: usize = 4096;
const MAX_PATH_COMPONENTS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub node_id: NodeRef,
    pub root_id: RootId,
    pub project_id: ProjectId,
    /// Must already be an absolute, existing directory with no symlink component.
    pub workspace: PathBuf,
    /// Relative paths this registration owns.
    pub owned_paths: Vec<PathBuf>,
    /// Relative paths excluded from ownership; exclusions always take precedence.
    pub forbidden_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedWorkspace {
    pub node_id: NodeRef,
    pub root_id: RootId,
    pub project_id: ProjectId,
    pub workspace: PathBuf,
    pub owned_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
    pub git_metadata: GitMetadata,
    pub custody: Custody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Custody {
    RegisteredOnly,
}

/// Typed node reference. Core has no persisted node ID type yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRef(String);

impl NodeRef {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkspaceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value == "."
            || value == ".."
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(WorkspaceError::InvalidNodeId);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitMetadata {
    Absent,
    Directory,
    WorktreeFile,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    WorkspaceMustBeAbsolute,
    WorkspaceDoesNotExist,
    WorkspaceMustBeDirectory,
    WorkspaceContainsSymlink,
    WorkspaceNotCanonical,
    ForbiddenWorkspaceRoot,
    InvalidRelativePath,
    InvalidNodeId,
    InputTooLarge,
    Io(io::ErrorKind),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for WorkspaceError {}

impl WorkspaceSpec {
    /// Validate an existing workspace without invoking Git or following symlinks.
    pub fn validate(&self) -> Result<ValidatedWorkspace, WorkspaceError> {
        let path = &self.workspace;
        if path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES
            || path.components().count() > MAX_PATH_COMPONENTS
            || self
                .owned_paths
                .len()
                .saturating_add(self.forbidden_paths.len())
                > MAX_SCOPE_PATHS
        {
            return Err(WorkspaceError::InputTooLarge);
        }
        for relative in self.owned_paths.iter().chain(self.forbidden_paths.iter()) {
            validate_relative(relative)?;
        }
        if !path.is_absolute() {
            return Err(WorkspaceError::WorkspaceMustBeAbsolute);
        }
        reject_symlink_components(path)?;
        let canonical = fs::canonicalize(path).map_err(map_not_found)?;
        if canonical.as_os_str() != path.as_os_str() {
            return Err(WorkspaceError::WorkspaceNotCanonical);
        }
        if !is_directory(path)? {
            return Err(WorkspaceError::WorkspaceMustBeDirectory);
        }
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .and_then(|p| fs::canonicalize(p).ok());
        if path.parent().is_none() || home.as_deref() == Some(path.as_path()) {
            return Err(WorkspaceError::ForbiddenWorkspaceRoot);
        }
        for relative in &self.owned_paths {
            reject_existing_scope_symlinks(&canonical.join(relative))?;
        }
        Ok(ValidatedWorkspace {
            node_id: self.node_id.clone(),
            root_id: self.root_id.clone(),
            project_id: self.project_id.clone(),
            workspace: canonical.clone(),
            owned_paths: self.owned_paths.clone(),
            forbidden_paths: self.forbidden_paths.clone(),
            git_metadata: observe_git_metadata(&canonical),
            custody: Custody::RegisteredOnly,
        })
    }
}

impl ValidatedWorkspace {
    /// Return whether a relative path falls within owned scope after exclusions.
    pub fn owns(&self, path: &Path) -> bool {
        if validate_relative(path).is_err() {
            return false;
        }
        !self
            .forbidden_paths
            .iter()
            .any(|deny| contains_case_insensitive(deny, path))
            && self.owned_paths.iter().any(|owned| contains(owned, path))
            && reject_existing_scope_symlinks(&self.workspace.join(path)).is_ok()
    }
}

fn validate_relative(path: &Path) -> Result<(), WorkspaceError> {
    let Some(value) = path.to_str() else {
        return Err(WorkspaceError::InvalidRelativePath);
    };
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || !value.is_ascii()
        || value.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        || value.chars().any(char::is_control)
        || value.split('/').any(|part| {
            let base = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || matches!(
                    base.as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
        || path.components().count() > MAX_PATH_COMPONENTS
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(WorkspaceError::InvalidRelativePath);
    }
    Ok(())
}

fn contains(parent: &Path, child: &Path) -> bool {
    child == parent || child.starts_with(parent)
}
fn contains_case_insensitive(parent: &Path, child: &Path) -> bool {
    // Conservative denial across ASCII case aliases, including case-insensitive
    // macOS volumes. Owned matching remains exact and therefore fail-closed.
    let parent = parent.as_os_str().to_string_lossy().to_ascii_lowercase();
    let child = child.as_os_str().to_string_lossy().to_ascii_lowercase();
    contains(Path::new(&parent), Path::new(&child))
}

fn is_directory(path: &Path) -> Result<bool, WorkspaceError> {
    fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .map_err(map_not_found)
}

fn reject_symlink_components(path: &Path) -> Result<(), WorkspaceError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map_err(map_not_found)?
            .file_type()
            .is_symlink()
        {
            return Err(WorkspaceError::WorkspaceContainsSymlink);
        }
    }
    Ok(())
}
fn reject_existing_scope_symlinks(path: &Path) -> Result<(), WorkspaceError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(info) if info.file_type().is_symlink() => {
                return Err(WorkspaceError::WorkspaceContainsSymlink);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(WorkspaceError::Io(error.kind())),
        }
    }
    Ok(())
}

fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute()
        || path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES
        || path.components().count() > MAX_PATH_COMPONENTS
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            _ => normalized.push(component.as_os_str()),
        }
    }
    Some(normalized)
}

fn observe_git_metadata(workspace: &Path) -> GitMetadata {
    let path = workspace.join(".git");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return GitMetadata::Absent,
        Err(_) => return GitMetadata::Unknown,
    };
    if metadata.file_type().is_symlink() {
        return GitMetadata::Unknown;
    }
    if metadata.is_dir() {
        return GitMetadata::Directory;
    }
    if !metadata.is_file() || metadata.len() > MAX_GIT_POINTER_BYTES as u64 {
        return GitMetadata::Unknown;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let Ok(file) = options.open(path) else {
        return GitMetadata::Unknown;
    };
    let Ok(opened) = file.metadata() else {
        return GitMetadata::Unknown;
    };
    if !opened.is_file() || opened.len() > MAX_GIT_POINTER_BYTES as u64 {
        return GitMetadata::Unknown;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
            return GitMetadata::Unknown;
        }
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_GIT_POINTER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_GIT_POINTER_BYTES
    {
        return GitMetadata::Unknown;
    }
    let Ok(contents) = String::from_utf8(bytes) else {
        return GitMetadata::Unknown;
    };
    let Some(target) = contents
        .trim_end_matches(['\n', '\r'])
        .strip_prefix("gitdir: ")
        .filter(|s| !s.is_empty() && *s == s.trim() && !s.chars().any(char::is_control))
    else {
        return GitMetadata::Unknown;
    };
    let target = Path::new(target);
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };
    let Some(resolved) = normalized_absolute(&resolved) else {
        return GitMetadata::Unknown;
    };
    if reject_symlink_components(&resolved).is_err() {
        return GitMetadata::Unknown;
    }
    match fs::symlink_metadata(resolved) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => GitMetadata::WorktreeFile,
        _ => GitMetadata::Unknown,
    }
}

fn map_not_found(error: io::Error) -> WorkspaceError {
    if error.kind() == io::ErrorKind::NotFound {
        WorkspaceError::WorkspaceDoesNotExist
    } else {
        WorkspaceError::Io(error.kind())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixture construction only; production paths return typed errors.
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn spec(path: &Path) -> WorkspaceSpec {
        WorkspaceSpec {
            node_id: NodeRef::new("node-1").unwrap(),
            root_id: RootId::new("root-1").unwrap(),
            project_id: ProjectId::new("project-1").unwrap(),
            workspace: path.to_path_buf(),
            owned_paths: vec![PathBuf::from("src"), PathBuf::from("src-old")],
            forbidden_paths: vec![PathBuf::from("src/private")],
        }
    }

    #[test]
    fn exact_nested_sibling_and_forbidden_component_boundaries() {
        let dir = tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap().join("work");
        fs::create_dir(&workspace).unwrap();
        let valid = spec(&workspace).validate().unwrap();
        assert!(valid.owns(Path::new("src")));
        assert!(valid.owns(Path::new("src/module.rs")));
        assert!(!valid.owns(Path::new("src-private/file")));
        assert!(!valid.owns(Path::new("src/private/key")));
        assert!(valid.owns(Path::new("src-old/item")));
        assert_eq!(valid.custody, Custody::RegisteredOnly);
    }

    #[test]
    fn rejects_workspace_root_home_and_symlink() {
        let root = spec(Path::new("/")).validate();
        assert_eq!(root.unwrap_err(), WorkspaceError::ForbiddenWorkspaceRoot);
        if let Some(home) = env::var_os("HOME") {
            assert_eq!(
                spec(&fs::canonicalize(home).unwrap())
                    .validate()
                    .unwrap_err(),
                WorkspaceError::ForbiddenWorkspaceRoot
            );
        }
        let dir = tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let real = base.join("real");
        fs::create_dir(&real).unwrap();
        let link = base.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &link).unwrap();
        assert_eq!(
            spec(&link).validate().unwrap_err(),
            WorkspaceError::WorkspaceContainsSymlink
        );
    }

    #[test]
    fn rejects_traversal_and_absolute_scope_paths() {
        let dir = tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap().join("work");
        fs::create_dir(&workspace).unwrap();
        let mut bad = spec(&workspace);
        bad.owned_paths.push(PathBuf::from("../escape"));
        assert_eq!(
            bad.validate().unwrap_err(),
            WorkspaceError::InvalidRelativePath
        );
        let mut bad = spec(&workspace);
        bad.forbidden_paths.push(PathBuf::from("/etc"));
        assert_eq!(
            bad.validate().unwrap_err(),
            WorkspaceError::InvalidRelativePath
        );
    }

    #[test]
    fn unknown_git_metadata_is_observed_without_git_execution() {
        let dir = tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap().join("work");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join(".git"), "not a gitdir file\n").unwrap();
        let valid = spec(&workspace).validate().unwrap();
        assert_eq!(valid.git_metadata, GitMetadata::Unknown);
        assert_eq!(valid.custody, Custody::RegisteredOnly);
    }

    #[test]
    fn bounded_portable_scopes_reject_aliases_and_case_exclusions_win() {
        let dir = tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let workspace = base.join("work");
        fs::create_dir(&workspace).unwrap();
        let valid = spec(&workspace).validate().unwrap();
        assert!(!valid.owns(Path::new("src/PRIVATE/key")));
        assert!(
            !valid.owns(Path::new("SRC/module.rs")),
            "ownership remains exact"
        );
        for path in [
            "C:/escape",
            "src\\private",
            "src/./module",
            "src//module",
            "src/module.",
            "src/CON.txt",
            "src/file:stream",
            "src/雪.rs",
            "src/*.rs",
            "src\n/forged",
        ] {
            let mut value = spec(&workspace);
            value.owned_paths = vec![path.into()];
            assert_eq!(
                value.validate().unwrap_err(),
                WorkspaceError::InvalidRelativePath,
                "{path}"
            );
        }
        let mut oversized = spec(&workspace);
        oversized.owned_paths = vec![PathBuf::from("src"); MAX_SCOPE_PATHS + 1];
        assert_eq!(
            oversized.validate().unwrap_err(),
            WorkspaceError::InputTooLarge
        );
        let mut alias = spec(&workspace);
        alias.workspace = PathBuf::from(format!("{}/./work", base.display()));
        assert_eq!(
            alias.validate().unwrap_err(),
            WorkspaceError::WorkspaceNotCanonical
        );
        assert!(NodeRef::new("bad/node").is_err());
    }

    #[test]
    fn git_pointer_observation_is_bounded_and_accepts_only_resolved_directory_shapes() {
        let dir = tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let workspace = base.join("work");
        fs::create_dir(&workspace).unwrap();
        let metadata = base.join("metadata");
        fs::create_dir(&metadata).unwrap();
        fs::write(workspace.join(".git"), "gitdir: ../metadata\n").unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::WorktreeFile
        );
        fs::write(
            workspace.join(".git"),
            "x".repeat(MAX_GIT_POINTER_BYTES + 1),
        )
        .unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
        fs::write(
            workspace.join(".git"),
            "gitdir: ../metadata\nforged second line\n",
        )
        .unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
        fs::write(workspace.join(".git"), [255]).unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_scope_and_git_pointer_ancestor_are_never_adopted() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let workspace = base.join("work");
        fs::create_dir(&workspace).unwrap();
        let outside = base.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, workspace.join("src")).unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap_err(),
            WorkspaceError::WorkspaceContainsSymlink
        );
        fs::remove_file(workspace.join("src")).unwrap();
        let valid = spec(&workspace).validate().unwrap();
        fs::create_dir(workspace.join("src")).unwrap();
        symlink(&outside, workspace.join("src/alias")).unwrap();
        assert!(!valid.owns(Path::new("src/alias/file")));
        symlink(&outside, base.join("metadata-alias")).unwrap();
        fs::create_dir(outside.join("nested")).unwrap();
        fs::write(workspace.join(".git"), "gitdir: ../metadata-alias/nested\n").unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
        fs::remove_file(workspace.join(".git")).unwrap();
        symlink(&outside, workspace.join(".git")).unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fifo_git_metadata_is_unknown_without_blocking() {
        let dir = tempdir().unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap().join("work");
        fs::create_dir(&workspace).unwrap();
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            workspace.join(".git"),
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_git_metadata_is_unknown_without_opening_it() {
        let dir = tempfile::Builder::new()
            .prefix("custody-")
            .tempdir_in("/tmp")
            .unwrap();
        let workspace = fs::canonicalize(dir.path()).unwrap().join("work");
        fs::create_dir(&workspace).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(workspace.join(".git")).unwrap();
        assert_eq!(
            spec(&workspace).validate().unwrap().git_metadata,
            GitMetadata::Unknown
        );
    }
}
