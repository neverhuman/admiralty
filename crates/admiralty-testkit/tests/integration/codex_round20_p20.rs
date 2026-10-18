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

fn observed(
    binding: &AssetBinding,
    registered: bool,
    revision: &str,
    at: u64,
) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: revision.into(),
        clock_id: "round20-post-pr57-clock".into(),
        observed_at: at.to_string(),
        decision_at: at.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round19-20-post-pr57-release-qualification".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round20_post_pr57_release_qualification_and_strict_audit() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let object_root = fixture.path().join("release-objects");
    fs::create_dir(&object_root)?;
    fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))?;
    let release_bundle = b"PR-57 post-merge P20 release and archival receipt";
    let artifact_store = ArtifactStore::open(&object_root, 1024)?;
    let reference = artifact_store.publish_bytes(release_bundle)?;
    assert_eq!(artifact_store.read_verified(&reference)?, release_bundle);
    assert_eq!(artifact_store.publish_bytes(release_bundle)?, reference);

    let database = fixture.path().join("release-custody.sqlite");
    let mut connection = Connection::open(&database)?;
    custody::initialize(&connection)?;
    let binding = AssetBinding {
        project_id: ProjectId::new("round20-project")?,
        root_id: RootId::new("round20-root")?,
        task_id: TaskId::new("round20-pr57-release")?,
        asset_id: "round20-release-bundle".into(),
        owner_id: PrincipalId::new("round20-release-owner")?,
        sha256: reference.sha256.clone(),
        generation: "20".into(),
    };

    let registration = RegisterAssetRequest {
        command_id: "round20-register-release".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: observed(&binding, false, "0", 10),
    };
    let registered = custody::register_asset(&mut connection, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut connection, registration)?,
        registered,
        "exact registration replay preserves the original archival receipt"
    );

    let pin = AcquirePinRequest {
        command_id: "round20-pin-release-consumer".into(),
        binding: binding.clone(),
        pin_id: "post-pr57-consumer".into(),
        expected_revision: "1".into(),
        observation: observed(&binding, true, "1", 11),
    };
    let pin_receipt = custody::acquire_pin(&mut connection, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut connection, pin)?, pin_receipt);

    // Retirement remains closed while a downstream consumer pin is live.
    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "round20-retire-while-pinned".into(),
                binding: binding.clone(),
                claim_id: "round20-retirement".into(),
                expected_revision: "2".into(),
                observation: observed(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    // Invalid release evidence cannot remove the live pin or open retirement.
    assert!(matches!(
        custody::release_pin(
            &mut connection,
            ReleasePinRequest {
                command_id: "round20-stale-release".into(),
                binding: binding.clone(),
                pin_id: "post-pr57-consumer".into(),
                expected_revision: "1".into(),
                observation: observed(&binding, true, "1", 12),
            },
        ),
        Err(CustodyError::StaleRevision)
    ));
    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "round20-retirement-still-pinned".into(),
                binding: binding.clone(),
                claim_id: "round20-retirement".into(),
                expected_revision: "2".into(),
                observation: observed(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round20-release-consumer".into(),
        binding: binding.clone(),
        pin_id: "post-pr57-consumer".into(),
        expected_revision: "2".into(),
        observation: observed(&binding, true, "2", 12),
    };
    let released = custody::release_pin(&mut connection, release.clone())?;
    assert_eq!(custody::release_pin(&mut connection, release)?, released);

    let retirement = ClaimRetirementRequest {
        command_id: "round20-retirement-claim".into(),
        binding: binding.clone(),
        claim_id: "round20-retirement".into(),
        expected_revision: "3".into(),
        observation: observed(&binding, true, "3", 13),
    };
    let retirement_receipt = custody::claim_retirement(&mut connection, retirement.clone())?;
    let receipt_json = serde_json::to_value(&retirement_receipt)?;
    assert!(receipt_json["recorded_only"].as_bool().unwrap_or(false));
    assert!(
        !receipt_json["destructive_permission"]
            .as_bool()
            .unwrap_or(true)
    );
    assert_eq!(
        custody::claim_retirement(&mut connection, retirement.clone())?,
        retirement_receipt
    );

    // A component-external trigger is a schema mismatch; initialization fails closed.
    connection.execute_batch(
        "CREATE TRIGGER unexpected_custody_effect AFTER INSERT ON custody_assets \
         BEGIN SELECT 1; END;",
    )?;
    assert!(matches!(
        custody::initialize(&connection),
        Err(CustodyError::SchemaMismatch)
    ));
    connection.execute_batch("DROP TRIGGER unexpected_custody_effect")?;
    drop(connection);

    let mut reopened = Connection::open(&database)?;
    custody::initialize(&reopened)?;
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        retirement_receipt,
        "durable release receipt survives restart and exact replay"
    );
    let reopened_store = ArtifactStore::open(&object_root, 1024)?;
    assert_eq!(reopened_store.read_verified(&reference)?, release_bundle);
    Ok(())
}
