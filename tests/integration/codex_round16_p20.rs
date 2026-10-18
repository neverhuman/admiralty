use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{error::Error, fs};

fn observation(binding: &AssetBinding, registered: bool, observed_at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: observed_at.saturating_sub(10).to_string(),
        clock_id: "round16-p20-cutover-clock".into(),
        observed_at: observed_at.to_string(),
        decision_at: observed_at.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round16-cutover-qualification".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round16_end_to_end_cutover_fault_containment_qualification() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let archive_dir = fixture.path().join("archive");
    fs::create_dir(&archive_dir)?;
    let archive_bytes = b"round16 immutable archived release evidence";
    let archive_path = archive_dir.join("release-evidence.json");
    fs::write(&archive_path, archive_bytes)?;
    let archive_digest = format!("sha256:{:x}", Sha256::digest(archive_bytes));
    let db_path = fixture.path().join("custody.sqlite");

    let binding = AssetBinding {
        project_id: ProjectId::new("round16-project")?,
        root_id: RootId::new("round16-root")?,
        task_id: TaskId::new("round16-task")?,
        asset_id: "round16-release-evidence".into(),
        owner_id: PrincipalId::new("round16-owner")?,
        sha256: archive_digest,
        generation: "7".into(),
    };
    let mut db = Connection::open(&db_path)?;
    custody::initialize(&db)?;

    let registration = RegisterAssetRequest {
        command_id: "round16-register".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: observation(&binding, false, 10),
    };
    let registration_receipt = custody::register_asset(&mut db, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut db, registration)?,
        registration_receipt,
        "replayed cutover registration must preserve its receipt"
    );

    let pin = AcquirePinRequest {
        command_id: "round16-pin-live-consumer".into(),
        binding: binding.clone(),
        pin_id: "live-consumer".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut db, pin)?, pin_receipt);

    let blocked_retirement = ClaimRetirementRequest {
        command_id: "round16-blocked-retirement".into(),
        binding: binding.clone(),
        claim_id: "round16-retirement".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, 12),
    };
    assert!(matches!(
        custody::claim_retirement(&mut db, blocked_retirement),
        Err(CustodyError::ActivePins)
    ));

    // A cutover command with a stale revision must fail without releasing the pin.
    let stale_release = ReleasePinRequest {
        command_id: "round16-stale-release".into(),
        binding: binding.clone(),
        pin_id: "live-consumer".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 12),
    };
    assert!(matches!(
        custody::release_pin(&mut db, stale_release),
        Err(CustodyError::StaleRevision)
    ));
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round16-still-blocked".into(),
                binding: binding.clone(),
                claim_id: "round16-retirement".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round16-release-consumer".into(),
        binding: binding.clone(),
        pin_id: "live-consumer".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, 12),
    };
    let release_receipt = custody::release_pin(&mut db, release.clone())?;
    assert_eq!(custody::release_pin(&mut db, release)?, release_receipt);

    let retirement = ClaimRetirementRequest {
        command_id: "round16-claim-retirement".into(),
        binding: binding.clone(),
        claim_id: "round16-retirement".into(),
        expected_revision: "3".into(),
        observation: observation(&binding, true, 13),
    };
    let retirement_receipt = custody::claim_retirement(&mut db, retirement.clone())?;
    let archived_receipt = serde_json::to_value(&retirement_receipt)?;
    assert_eq!(archived_receipt["recorded_only"], true);
    assert_eq!(archived_receipt["destructive_permission"], false);
    assert_eq!(
        custody::claim_retirement(&mut db, retirement)?,
        retirement_receipt,
        "retirement replay must be stable across cutover retries"
    );
    assert_eq!(fs::read(&archive_path)?, archive_bytes);
    drop(db);

    let mut reopened = Connection::open(&db_path)?;
    custody::initialize(&reopened)?;
    assert_eq!(fs::read(&archive_path)?, archive_bytes);
    let replayed_receipt = custody::claim_retirement(
        &mut reopened,
        ClaimRetirementRequest {
            command_id: "round16-claim-retirement".into(),
            binding: binding.clone(),
            claim_id: "round16-retirement".into(),
            expected_revision: "3".into(),
            observation: observation(&binding, true, 13),
        },
    )?;
    assert_eq!(replayed_receipt, retirement_receipt);

    Ok(())
}
