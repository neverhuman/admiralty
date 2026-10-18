//! Existing custody preconditions used by cleanup proposals.
//!
//! These tests exercise the read-only workspace registry and durable custody
//! pin/retirement APIs. There is no cleanup executor on this main commit, so
//! they do not claim deletion, quarantine, mount-swap, or process-liveness
//! qualification.

use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceError, WorkspaceSpec};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn private_fixture() -> tempfile::TempDir {
    // Resolve platform aliases such as macOS /var before creating the fixture:
    // the workspace registry deliberately rejects symlinked path components.
    let base = fs::canonicalize(std::env::temp_dir()).expect("canonical temporary base");
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-cleanup-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir_in(base)
        .expect("create owner-only disposable fixture")
}

fn private_directory(path: &Path) {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .expect("create private fixture directory");
}

fn binding() -> AssetBinding {
    AssetBinding {
        project_id: ProjectId::new("project-1").expect("valid fixture project"),
        root_id: RootId::new("root-1").expect("valid fixture root"),
        task_id: TaskId::new("task-1").expect("valid fixture task"),
        asset_id: "asset-1".into(),
        owner_id: PrincipalId::new("owner-1").expect("valid fixture owner"),
        sha256: format!("sha256:{}", "a".repeat(64)),
        generation: "1".into(),
    }
}

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "cleanup-fixture-clock".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:cleanup-custody".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn workspace_registry_excludes_forbidden_foreign_and_symlinked_paths() {
    let fixture = private_fixture();
    let root = fs::canonicalize(fixture.path()).expect("canonical workspace fixture root");
    let source = root.join("src");
    let ignored = source.join("ignored");
    let foreign = root.join("foreign");
    private_directory(&ignored);
    private_directory(&foreign);
    fs::write(source.join("keep.txt"), b"owned bytes").expect("write owned fixture bytes");
    fs::write(ignored.join("keep.txt"), b"ignored bytes").expect("write ignored fixture bytes");
    fs::write(foreign.join("keep.txt"), b"foreign bytes").expect("write foreign fixture bytes");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&foreign, source.join("linked")).expect("create symlink fixture");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-1").expect("valid fixture node"),
        root_id: RootId::new("root-1").expect("valid fixture root"),
        project_id: ProjectId::new("project-1").expect("valid fixture project"),
        workspace: root,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/ignored")],
    };
    let validated = spec.validate().expect("validate disposable workspace");

    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(&PathBuf::from("src/keep.txt")));
    assert!(!validated.owns(&PathBuf::from("src/ignored/keep.txt")));
    assert!(!validated.owns(&PathBuf::from("foreign/keep.txt")));
    #[cfg(unix)]
    assert!(!validated.owns(&PathBuf::from("src/linked/keep.txt")));

    assert_eq!(
        fs::read(source.join("keep.txt")).expect("read owned fixture"),
        b"owned bytes"
    );
    assert_eq!(
        fs::read(ignored.join("keep.txt")).expect("read ignored fixture"),
        b"ignored bytes"
    );
    assert_eq!(
        fs::read(foreign.join("keep.txt")).expect("read foreign fixture"),
        b"foreign bytes"
    );
}

#[cfg(unix)]
#[test]
fn workspace_registry_rejects_symlink_root_without_changing_contents() {
    let fixture = private_fixture();
    let real_workspace = fixture.path().join("real-workspace");
    fs::create_dir(&real_workspace).expect("create real workspace fixture");
    let sentinel = real_workspace.join("keep.txt");
    fs::write(&sentinel, b"must remain unchanged").expect("write sentinel fixture");
    let alias = fixture.path().join("workspace-alias");
    std::os::unix::fs::symlink(&real_workspace, &alias).expect("create workspace symlink");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("node-symlink").expect("valid fixture node"),
        root_id: RootId::new("root-symlink").expect("valid fixture root"),
        project_id: ProjectId::new("project-symlink").expect("valid fixture project"),
        workspace: alias,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: Vec::new(),
    };

    assert!(matches!(
        spec.validate(),
        Err(WorkspaceError::WorkspaceContainsSymlink)
    ));
    assert_eq!(
        fs::read(sentinel).expect("read untouched sentinel fixture"),
        b"must remain unchanged"
    );
}

#[test]
fn durable_active_pin_blocks_retirement_claim_until_release() {
    let fixture = private_fixture();
    let path = fixture.path().join("custody.sqlite");
    let mut connection = Connection::open(path).expect("open fixture database");
    custody::initialize(&connection).expect("initialize custody schema");
    let binding = binding();

    custody::register_asset(
        &mut connection,
        RegisterAssetRequest {
            command_id: "register-1".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        },
    )
    .expect("register fixture asset");
    assert!(matches!(
        custody::acquire_pin(
            &mut connection,
            AcquirePinRequest {
                command_id: "acquire-oversized-pin".into(),
                binding: binding.clone(),
                pin_id: "p".repeat(custody::MAX_ID_BYTES + 1),
                expected_revision: "1".into(),
                observation: observation(&binding, true, 11),
            }
        ),
        Err(CustodyError::Invalid("identifier bounds"))
    ));

    let acquire_request = AcquirePinRequest {
        command_id: "acquire-1".into(),
        binding: binding.clone(),
        pin_id: "pin-1".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 11),
    };
    let acquired =
        custody::acquire_pin(&mut connection, acquire_request.clone()).expect("record active pin");
    let replayed = custody::acquire_pin(&mut connection, acquire_request)
        .expect("exact replay returns recorded receipt");
    assert_eq!(replayed, acquired);

    assert!(matches!(
        custody::release_pin(
            &mut connection,
            ReleasePinRequest {
                command_id: "release-missing".into(),
                binding: binding.clone(),
                pin_id: "missing-pin".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::InvalidPin)
    ));

    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "claim-blocked".into(),
                binding: binding.clone(),
                claim_id: "claim-1".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::ActivePins)
    ));

    let mut foreign_binding = binding.clone();
    foreign_binding.root_id = RootId::new("foreign-root").expect("valid foreign root");
    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "claim-foreign".into(),
                binding: foreign_binding.clone(),
                claim_id: "foreign-claim".into(),
                expected_revision: "2".into(),
                observation: observation(&foreign_binding, true, 12),
            }
        ),
        Err(CustodyError::ForeignAsset)
    ));

    custody::release_pin(
        &mut connection,
        ReleasePinRequest {
            command_id: "release-1".into(),
            binding: binding.clone(),
            pin_id: "pin-1".into(),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        },
    )
    .expect("release active pin");
    custody::claim_retirement(
        &mut connection,
        ClaimRetirementRequest {
            command_id: "claim-after-release".into(),
            binding: binding.clone(),
            claim_id: "claim-2".into(),
            expected_revision: "3".into(),
            observation: observation(&binding, true, 13),
        },
    )
    .expect("claim retirement after final pin release");
}
