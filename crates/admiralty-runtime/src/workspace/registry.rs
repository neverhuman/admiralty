//! Read-only validation and observation of registered workspace custody.
//!
//! Registration records identity and observed Git metadata. It is not an
//! execution, mutation, or cleanup capability.

use admiralty_core::{ProjectId, RootId};
use std::{
    env, fmt, fs, io,
    path::{Component, Path, PathBuf},
};

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
            return Err(WorkspaceError::InvalidRelativePath);
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
        if !path.is_absolute() {
            return Err(WorkspaceError::WorkspaceMustBeAbsolute);
        }
        reject_symlink_components(path)?;
        let canonical = fs::canonicalize(path).map_err(map_not_found)?;
        if canonical != *path {
            return Err(WorkspaceError::WorkspaceNotCanonical);
        }
        if !is_directory(path)? {
            return Err(WorkspaceError::WorkspaceMustBeDirectory);
        }
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .and_then(|p| fs::canonicalize(p).ok());
        if path == Path::new("/") || home.as_deref() == Some(path.as_path()) {
            return Err(WorkspaceError::ForbiddenWorkspaceRoot);
        }
        for relative in self.owned_paths.iter().chain(self.forbidden_paths.iter()) {
            validate_relative(relative)?;
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
        !self.forbidden_paths.iter().any(|deny| contains(deny, path))
            && self.owned_paths.iter().any(|owned| contains(owned, path))
    }
}

fn validate_relative(path: &Path) -> Result<(), WorkspaceError> {
    if path.as_os_str().is_empty()
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

fn observe_git_metadata(workspace: &Path) -> GitMetadata {
    let path = workspace.join(".git");
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return GitMetadata::Absent;
    };
    if metadata.file_type().is_symlink() {
        return GitMetadata::Unknown;
    }
    if metadata.is_dir() {
        return GitMetadata::Directory;
    }
    if !metadata.is_file() {
        return GitMetadata::Unknown;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return GitMetadata::Unknown;
    };
    let Some(target) = contents
        .strip_prefix("gitdir: ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return GitMetadata::Unknown;
    };
    let target = Path::new(target);
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };
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
        let workspace = dir.path().join("work");
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
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
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
        let workspace = dir.path().join("work");
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
        let workspace = dir.path().join("work");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join(".git"), "not a gitdir file\n").unwrap();
        let valid = spec(&workspace).validate().unwrap();
        assert_eq!(valid.git_metadata, GitMetadata::Unknown);
        assert_eq!(valid.custody, Custody::RegisteredOnly);
    }
}
