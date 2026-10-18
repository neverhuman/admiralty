//! Round 8 Item 2: deterministic cross-packet worktree isolation qualification.

use admiralty_core::{ProjectId, RootId};
use admiralty_runtime::workspace::registry::{
    Custody, GitMetadata, NodeRef, WorkspaceSpec,
};
use std::{fs, path::PathBuf};

#[test]
fn p04_p10_round8_p08_worktree_isolation_qualification() {
    let temp = tempfile::tempdir().expect("temporary worktree fixture");
    let worktrees = temp.path().join("worktrees");
    let first_root = worktrees.join("worker-a");
    let second_root = worktrees.join("worker-b");
    fs::create_dir_all(worktrees.join(".git/worktrees/worker-a"))
        .expect("first worktree metadata");
    fs::create_dir_all(worktrees.join(".git/worktrees/worker-b"))
        .expect("second worktree metadata");
    fs::create_dir_all(first_root.join("src")).expect("first worktree source");
    fs::create_dir_all(second_root.join("src")).expect("second worktree source");
    fs::write(first_root.join("src/owned.rs"), b"worker-a").expect("first source");
    fs::write(second_root.join("src/owned.rs"), b"worker-b").expect("second source");
    // Worktree metadata is observed as a file, without resolving or invoking Git.
    fs::write(first_root.join(".git"), b"gitdir: ../.git/worktrees/worker-a\n")
        .expect("first git pointer");
    fs::write(second_root.join(".git"), b"gitdir: ../.git/worktrees/worker-b\n")
        .expect("second git pointer");
    let first_root = fs::canonicalize(first_root).expect("canonical first worktree");
    let second_root = fs::canonicalize(second_root).expect("canonical second worktree");

    let spec = |node: &str, root: &str, project: &str, workspace: PathBuf| WorkspaceSpec {
        node_id: NodeRef::new(node).expect("valid node ID"),
        root_id: RootId::new(root).expect("valid root ID"),
        project_id: ProjectId::new(project).expect("valid project ID"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let first_spec = spec("worker-a", "root-a", "project-a", first_root.clone());
    let second_spec = spec("worker-b", "root-b", "project-b", second_root.clone());
    let first = first_spec.validate().expect("first worktree validates");
    let second = second_spec.validate().expect("second worktree validates");

    assert_eq!(first.custody, Custody::RegisteredOnly);
    assert_eq!(second.custody, Custody::RegisteredOnly);
    assert_eq!(first.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(second.git_metadata, GitMetadata::WorktreeFile);
    assert!(first.owns(std::path::Path::new("src/owned.rs")));
    assert!(second.owns(std::path::Path::new("src/owned.rs")));
    assert!(!first.owns(std::path::Path::new("../worker-b/src/owned.rs")));
    assert!(!second.owns(std::path::Path::new("../worker-a/src/owned.rs")));
    assert!(!first.owns(std::path::Path::new(".git")));
    assert!(!second.owns(std::path::Path::new(".git")));

    assert_eq!(first_spec.validate().expect("repeat first observation"), first);
    assert_eq!(second_spec.validate().expect("repeat second observation"), second);
    assert_eq!(fs::read(first_root.join("src/owned.rs")).unwrap(), b"worker-a");
    assert_eq!(fs::read(second_root.join("src/owned.rs")).unwrap(), b"worker-b");
}
