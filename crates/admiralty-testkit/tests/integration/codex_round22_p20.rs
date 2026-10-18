#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::{
    artifacts::ArtifactStore,
    custody::{
        self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
        CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
    },
};
use rusqlite::Connection;
use std::{error::Error, fs, os::unix::fs::PermissionsExt};

fn observation(
    binding: &AssetBinding,
    registered: bool,
    revision: &str,
    tick: u64,
) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: revision.into(),
        clock_id: "round22-canary-clock".into(),
        observed_at: tick.to_string(),
        decision_at: tick.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round22-canary-release-package".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round22_canary_smoke_release_package_qualification() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let object_root = root.join("release-objects");
    fs::create_dir(&object_root)?;
    fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))?;

    // This canary payload models a package manifest bound to archival evidence.
    let package = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "release_id": "round22-p20-canary",
        "retirement_mode": "recorded_only",
        "destructive_permission": false,
        "archive_files": ["release/manifest.json", "release/receipt.json"]
    }))?;
    let artifact_store = ArtifactStore::open(&object_root, 4096)?;
    let reference = artifact_store.publish_bytes(&package)?;
    assert_eq!(artifact_store.read_verified(&reference)?, package);
    assert_eq!(artifact_store.publish_bytes(&package)?, reference);

    let database_path = root.join("canary-custody.sqlite");
    let mut db = Connection::open(&database_path)?;
    custody::initialize(&db)?;
    let binding = AssetBinding {
        project_id: ProjectId::new("round22-project")?,
        root_id: RootId::new("round22-retained-root")?,
        task_id: TaskId::new("round22-release-task")?,
        asset_id: "round22-package-canary".into(),
        owner_id: PrincipalId::new("round22-release-owner")?,
        sha256: reference.sha256.clone(),
        generation: "22".into(),
    };
    let registration = RegisterAssetRequest {
        command_id: "round22-register-canary".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: observation(&binding, false, "0", 10),
    };
    let registration_receipt = custody::register_asset(&mut db, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut db, registration)?,
        registration_receipt
    );

    let pin = AcquirePinRequest {
        command_id: "round22-pin-canary-reader".into(),
        binding: binding.clone(),
        pin_id: "canary-reader".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, "1", 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut db, pin)?, pin_receipt);
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round22-premature-retirement".into(),
                binding: binding.clone(),
                claim_id: "round22-retirement".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round22-release-canary-reader".into(),
        binding: binding.clone(),
        pin_id: "canary-reader".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, "2", 12),
    };
    let release_receipt = custody::release_pin(&mut db, release.clone())?;
    assert_eq!(custody::release_pin(&mut db, release)?, release_receipt);

    let retirement = ClaimRetirementRequest {
        command_id: "round22-claim-retirement".into(),
        binding: binding.clone(),
        claim_id: "round22-retirement".into(),
        expected_revision: "3".into(),
        observation: observation(&binding, true, "3", 13),
    };
    let receipt = custody::claim_retirement(&mut db, retirement.clone())?;
    let receipt_json = serde_json::to_value(&receipt)?;
    assert!(receipt_json["recorded_only"].as_bool().unwrap_or(false));
    assert!(
        !receipt_json["destructive_permission"]
            .as_bool()
            .unwrap_or(true)
    );
    assert_eq!(
        custody::claim_retirement(&mut db, retirement.clone())?,
        receipt
    );
    drop(db);

    let mut reopened = Connection::open(&database_path)?;
    custody::initialize(&reopened)?;
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        receipt
    );
    let reopened_artifacts = ArtifactStore::open(&object_root, 4096)?;
    assert_eq!(reopened_artifacts.read_verified(&reference)?, package);
    assert!(database_path.starts_with(&root));
    assert!(object_root.starts_with(&root));
    Ok(())
}
