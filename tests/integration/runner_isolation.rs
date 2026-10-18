use admiralty_core::{ProjectId, RootId};
use admiralty_runtime::workspace::registry::{
    Custody, GitMetadata, NodeRef, WorkspaceError, WorkspaceSpec,
};
use std::{fs, path::PathBuf};

#[test]
fn registered_workspace_validation_is_scoped_read_only_and_fail_closed() {
    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let root = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let workspace = root.join("workspace");
    let sibling = root.join("sibling");
    fs::create_dir_all(workspace.join("src")).expect("workspace source dir");
    fs::create_dir_all(workspace.join(".git/hooks")).expect("git hooks fixture");
    fs::create_dir_all(&sibling).expect("sibling fixture");
    fs::write(workspace.join("src/lib.rs"), b"owned source").expect("owned file");
    fs::write(workspace.join(".git/hooks/pre-commit"), b"sentinel hook\n").expect("hook sentinel");
    fs::write(sibling.join("secret.key"), b"private sentinel\n").expect("sibling sentinel");
    let hook_before = fs::read(workspace.join(".git/hooks/pre-commit")).expect("hook bytes");
    let secret_before = fs::read(sibling.join("secret.key")).expect("secret bytes");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-local").expect("node ID"),
        root_id: RootId::new("root-local").expect("root ID"),
        project_id: ProjectId::new("project-local").expect("project ID"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let validated = spec.validate().expect("safe registration validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert_eq!(validated.git_metadata, GitMetadata::Directory);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/hooks/pre-commit")));
    assert!(
        !validated.owns(std::path::Path::new(".GIT/hooks/pre-commit")),
        "forbidden Git metadata remains denied through case aliases"
    );
    assert!(!validated.owns(std::path::Path::new("../sibling/secret.key")));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(
            sibling.join("secret.key"),
            workspace.join("src/secret-link"),
        )
        .expect("symlink fixture");
        assert!(!validated.owns(std::path::Path::new("src/secret-link")));
    }

    assert_eq!(
        fs::read(workspace.join(".git/hooks/pre-commit")).expect("hook remains readable"),
        hook_before,
        "registry validation must not rewrite Git hooks"
    );
    assert_eq!(
        fs::read(sibling.join("secret.key")).expect("sibling remains readable"),
        secret_before,
        "registry validation must not rewrite sibling/private data"
    );
    assert_eq!(
        spec.validate().expect("repeat observation"),
        validated,
        "repeated validation is deterministic and grants no new authority"
    );

    let mut invalid = spec;
    invalid.owned_paths.push(PathBuf::from("../sibling"));
    assert_eq!(
        invalid.validate().expect_err("expected error"),
        WorkspaceError::InvalidRelativePath
    );
}

#[test]
fn reserved_device_alias_is_rejected_as_owned_scope() {
    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-reserved").expect("node ID"),
        root_id: RootId::new("root-reserved").expect("root ID"),
        project_id: ProjectId::new("project-reserved").expect("project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: Vec::new(),
    };

    let mut invalid = spec;
    invalid.owned_paths.push(PathBuf::from("src/CON.txt"));
    assert_eq!(
        invalid
            .validate()
            .expect_err("reserved device alias must fail closed"),
        WorkspaceError::InvalidRelativePath
    );
}

#[test]
fn workspace_scope_path_limit_is_enforced_at_boundary() {
    use admiralty_runtime::workspace::registry::MAX_SCOPE_PATHS;

    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-limit").expect("node ID"),
        root_id: RootId::new("root-limit").expect("root ID"),
        project_id: ProjectId::new("project-limit").expect("project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src"); MAX_SCOPE_PATHS + 1],
        forbidden_paths: Vec::new(),
    };

    assert_eq!(
        spec.validate()
            .expect_err("scope larger than configured bound must fail closed"),
        WorkspaceError::InputTooLarge
    );
}

#[test]
fn workspace_scope_path_byte_limit_rejects_overflow() {
    use admiralty_runtime::workspace::registry::MAX_PATH_BYTES;

    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-path-bytes").expect("node ID"),
        root_id: RootId::new("root-path-bytes").expect("root ID"),
        project_id: ProjectId::new("project-path-bytes").expect("project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("a".repeat(MAX_PATH_BYTES + 1))],
        forbidden_paths: Vec::new(),
    };

    assert_eq!(
        spec.validate()
            .expect_err("overlong owned path must fail closed"),
        WorkspaceError::InvalidRelativePath
    );
}

#[test]
fn forbidden_scope_overrides_broader_owned_scope() {
    let temp = tempfile::tempdir().expect("temporary workspace fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-overlap").expect("node ID"),
        root_id: RootId::new("root-overlap").expect("root ID"),
        project_id: ProjectId::new("project-overlap").expect("project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let validated = spec.validate().expect("overlapping scope validates");

    assert!(validated.owns(std::path::Path::new("src/public.rs")));
    assert!(
        !validated.owns(std::path::Path::new("src/private/token")),
        "forbidden child scope overrides an owned parent"
    );
}
