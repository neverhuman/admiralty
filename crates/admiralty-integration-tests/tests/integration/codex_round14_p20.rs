use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use std::{error::Error, fs};

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "round14-p20-clock".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round14-archival-replay".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn codex_round14_p20_release_readiness_deterministic_replay_qualification()
-> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let workspace = fixture.path().join("workspace");
    fs::create_dir(&workspace)?;
    let retained = workspace.join("release-archive.json");
    let archive_bytes = br#"{"release":"round14","status":"recorded","authority":false}"#;
    fs::write(&retained, archive_bytes)?;

    let binding = AssetBinding {
        project_id: ProjectId::new("round14-project")?,
        root_id: RootId::new("round14-root")?,
        task_id: TaskId::new("round14-work-order")?,
        asset_id: "round14-release-archive".into(),
        owner_id: PrincipalId::new("round14-owner")?,
        sha256: format!("sha256:{}", "a".repeat(64)),
        generation: "1".into(),
    };
    let db_path = fixture.path().join("custody.sqlite");
    let mut db = Connection::open(&db_path)?;
    custody::initialize(&db)?;

    let registration = RegisterAssetRequest {
        command_id: "round14-register".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: observation(&binding, false, 10),
    };
    let registration_receipt = custody::register_asset(&mut db, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut db, registration)?,
        registration_receipt
    );

    let pin = AcquirePinRequest {
        command_id: "round14-archive-pin".into(),
        binding: binding.clone(),
        pin_id: "release-archive-retention".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut db, pin)?, pin_receipt);
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round14-blocked-retirement".into(),
                binding: binding.clone(),
                claim_id: "round14-retirement".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, 12),
            }
        ),
        Err(CustodyError::ActivePins)
    ));

    custody::release_pin(
        &mut db,
        ReleasePinRequest {
            command_id: "round14-unpin-archive".into(),
            binding: binding.clone(),
            pin_id: "release-archive-retention".into(),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        },
    )?;
    let retirement = ClaimRetirementRequest {
        command_id: "round14-retirement".into(),
        binding: binding.clone(),
        claim_id: "round14-retirement-claim".into(),
        expected_revision: "3".into(),
        observation: observation(&binding, true, 13),
    };
    let archival_receipt = custody::claim_retirement(&mut db, retirement.clone())?;
    assert_eq!(
        custody::claim_retirement(&mut db, retirement.clone())?,
        archival_receipt,
        "exact retirement replay must preserve the original archival receipt"
    );
    let receipt_value = serde_json::to_value(&archival_receipt)?;
    assert_eq!(receipt_value["recorded_only"], true);
    assert_eq!(receipt_value["destructive_permission"], false);
    assert_eq!(archival_receipt.request_digest.len(), 71);
    assert_eq!(fs::read(&retained)?, archive_bytes);
    drop(db);

    let mut reopened = Connection::open(&db_path)?;
    custody::initialize(&reopened)?;
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        archival_receipt,
        "reopen must replay the same terminal receipt"
    );
    assert!(matches!(
        custody::acquire_pin(
            &mut reopened,
            AcquirePinRequest {
                command_id: "round14-late-reference".into(),
                binding: binding.clone(),
                pin_id: "late-release-reference".into(),
                expected_revision: "4".into(),
                observation: observation(&binding, true, 14),
            }
        ),
        Err(CustodyError::RetirementClaimed)
    ));

    reopened.execute(
        "UPDATE main.custody_commands SET request_digest=?1 WHERE command_id=?2",
        rusqlite::params![format!("sha256:{}", "0".repeat(64)), "round14-retirement"],
    )?;
    assert!(matches!(
        custody::acquire_pin(
            &mut reopened,
            AcquirePinRequest {
                command_id: "round14-after-tamper".into(),
                binding: binding.clone(),
                pin_id: "tampered-history-reference".into(),
                expected_revision: "4".into(),
                observation: observation(&binding, true, 14),
            }
        ),
        Err(CustodyError::CorruptState)
    ));
    let command_count: i64 =
        reopened.query_row("SELECT count(*) FROM main.custody_commands", [], |row| {
            row.get(0)
        })?;
    assert_eq!(command_count, 4, "negative paths must not append receipts");
    assert_eq!(fs::read(&retained)?, archive_bytes);
    Ok(())
}
