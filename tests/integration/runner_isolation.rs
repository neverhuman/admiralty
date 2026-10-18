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
        spec.validate().expect("repeat observation").custody,
        Custody::RegisteredOnly,
        "validation does not grant runner or cleanup authority"
    );

    let mut invalid = spec;
    invalid.owned_paths.push(PathBuf::from("../sibling"));
    assert_eq!(
        invalid.validate().expect_err("expected error"),
        WorkspaceError::InvalidRelativePath
    );
}
