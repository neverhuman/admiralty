//! Round 8 Item 2 cross-packet qualification for P09 and P20.

use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceSpec};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use std::{fs, path::PathBuf};

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "round8-p09-p20".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round8-qualification".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round8_p09_orchestrator_retirement_qualification() {
    let fixture = tempfile::tempdir().expect("temporary owned fixture");
    let root = fs::canonicalize(fixture.path()).expect("canonical fixture root");
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("asset.txt"), b"fixture asset").expect("write asset");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round8-node").expect("node id"),
        root_id: RootId::new("round8-root").expect("root id"),
        project_id: ProjectId::new("round8-project").expect("project id"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("asset.txt")],
        forbidden_paths: Vec::new(),
    };
    let validated = spec.validate().expect("P09 workspace registration");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(&PathBuf::from("asset.txt")));

    let binding = AssetBinding {
        project_id: ProjectId::new("round8-project").expect("project id"),
        root_id: RootId::new("round8-root").expect("root id"),
        task_id: TaskId::new("round8-task").expect("task id"),
        asset_id: "asset-1".into(),
        owner_id: PrincipalId::new("round8-owner").expect("owner id"),
        sha256: format!("sha256:{}", "b".repeat(64)),
        generation: "1".into(),
    };
    let mut db = Connection::open_in_memory().expect("open deterministic custody db");
    custody::initialize(&db).expect("initialize custody db");
    custody::register_asset(
        &mut db,
        RegisterAssetRequest {
            command_id: "register".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        },
    )
    .expect("register orchestrated asset");

    let pin = AcquirePinRequest {
        command_id: "pin".into(),
        binding: binding.clone(),
        pin_id: "active-orchestrator".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone()).expect("acquire active pin");
    assert_eq!(
        custody::acquire_pin(&mut db, pin).expect("idempotent pin replay"),
        pin_receipt
    );

    let blocked = ClaimRetirementRequest {
        command_id: "retire-blocked".into(),
        binding: binding.clone(),
        claim_id: "retirement-1".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, 12),
    };
    assert!(matches!(
        custody::claim_retirement(&mut db, blocked),
        Err(CustodyError::ActivePins)
    ));
    custody::release_pin(
        &mut db,
        ReleasePinRequest {
            command_id: "unpin".into(),
            binding: binding.clone(),
            pin_id: "active-orchestrator".into(),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        },
    )
    .expect("release orchestrator pin");
    custody::claim_retirement(
        &mut db,
        ClaimRetirementRequest {
            command_id: "retire".into(),
            binding: binding.clone(),
            claim_id: "retirement-1".into(),
            expected_revision: "3".into(),
            observation: observation(&binding, true, 13),
        },
    )
    .expect("retirement claim after release");
    assert_eq!(
        fs::read(workspace.join("asset.txt")).expect("asset remains"),
        b"fixture asset"
    );
}
