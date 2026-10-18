use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{error::Error, fs};

fn observation(binding: &AssetBinding, registered: bool, at: u64) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: (at - 10).to_string(),
        clock_id: "round15-p20-clock".into(),
        observed_at: at.to_string(),
        decision_at: (at + 1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round15-release-smoke".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn codex_round15_p20_retirement_release_smoke_multistep_replay() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let workspace = fixture.path().join("work-orders");
    fs::create_dir(&workspace)?;
    let database = fixture.path().join("custody.sqlite");
    let mut db = Connection::open(&database)?;
    custody::initialize(&db)?;

    for index in 0..6_u8 {
        let archive = format!("round15 archived work-order evidence {index}").into_bytes();
        let archive_path = workspace.join(format!("release-{index}.json"));
        fs::write(&archive_path, &archive)?;
        let digest = format!("sha256:{:x}", Sha256::digest(&archive));
        let binding = AssetBinding {
            project_id: ProjectId::new("round15-project")?,
            root_id: RootId::new("round15-root")?,
            task_id: TaskId::new(format!("round15-task-{index}"))?,
            asset_id: format!("round15-asset-{index}"),
            owner_id: PrincipalId::new("round15-owner")?,
            sha256: digest,
            generation: "1".into(),
        };

        let registration = RegisterAssetRequest {
            command_id: format!("round15-register-{index}"),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation(&binding, false, 10),
        };
        let registered = custody::register_asset(&mut db, registration.clone())?;
        assert_eq!(
            custody::register_asset(&mut db, registration)?,
            registered,
            "registration replay must be byte-stable"
        );

        let pin_one = AcquirePinRequest {
            command_id: format!("round15-pin-a-{index}"),
            binding: binding.clone(),
            pin_id: format!("archive-pin-a-{index}"),
            expected_revision: "1".into(),
            observation: observation(&binding, true, 11),
        };
        let pin_one_receipt = custody::acquire_pin(&mut db, pin_one.clone())?;
        assert_eq!(custody::acquire_pin(&mut db, pin_one)?, pin_one_receipt);
        let pin_two = AcquirePinRequest {
            command_id: format!("round15-pin-b-{index}"),
            binding: binding.clone(),
            pin_id: format!("archive-pin-b-{index}"),
            expected_revision: "2".into(),
            observation: observation(&binding, true, 12),
        };
        custody::acquire_pin(&mut db, pin_two)?;

        assert!(matches!(
            custody::claim_retirement(
                &mut db,
                ClaimRetirementRequest {
                    command_id: format!("round15-blocked-a-{index}"),
                    binding: binding.clone(),
                    claim_id: format!("round15-claim-{index}"),
                    expected_revision: "3".into(),
                    observation: observation(&binding, true, 13),
                }
            ),
            Err(CustodyError::ActivePins)
        ));
        assert!(matches!(
            custody::release_pin(
                &mut db,
                ReleasePinRequest {
                    command_id: format!("round15-stale-release-{index}"),
                    binding: binding.clone(),
                    pin_id: format!("archive-pin-a-{index}"),
                    expected_revision: "2".into(),
                    observation: observation(&binding, true, 13),
                }
            ),
            Err(CustodyError::StaleRevision)
        ));

        custody::release_pin(
            &mut db,
            ReleasePinRequest {
                command_id: format!("round15-release-a-{index}"),
                binding: binding.clone(),
                pin_id: format!("archive-pin-a-{index}"),
                expected_revision: "3".into(),
                observation: observation(&binding, true, 13),
            },
        )?;
        assert!(matches!(
            custody::claim_retirement(
                &mut db,
                ClaimRetirementRequest {
                    command_id: format!("round15-blocked-b-{index}"),
                    binding: binding.clone(),
                    claim_id: format!("round15-claim-{index}"),
                    expected_revision: "4".into(),
                    observation: observation(&binding, true, 14),
                }
            ),
            Err(CustodyError::ActivePins)
        ));
        custody::release_pin(
            &mut db,
            ReleasePinRequest {
                command_id: format!("round15-release-b-{index}"),
                binding: binding.clone(),
                pin_id: format!("archive-pin-b-{index}"),
                expected_revision: "4".into(),
                observation: observation(&binding, true, 14),
            },
        )?;

        let retirement = ClaimRetirementRequest {
            command_id: format!("round15-retire-{index}"),
            binding: binding.clone(),
            claim_id: format!("round15-claim-{index}"),
            expected_revision: "5".into(),
            observation: observation(&binding, true, 15),
        };
        let archival_receipt = custody::claim_retirement(&mut db, retirement.clone())?;
        assert_eq!(
            custody::claim_retirement(&mut db, retirement.clone())?,
            archival_receipt,
            "exact replay must return the original terminal receipt"
        );
        let receipt_value = serde_json::to_value(&archival_receipt)?;
        assert_eq!(receipt_value["recorded_only"], true);
        assert_eq!(receipt_value["destructive_permission"], false);
        assert_eq!(fs::read(&archive_path)?, archive);

        drop(db);
        db = Connection::open(&database)?;
        custody::initialize(&db)?;
        assert_eq!(
            custody::claim_retirement(&mut db, retirement)?,
            archival_receipt,
            "reopened history must replay identically"
        );
        assert!(matches!(
            custody::acquire_pin(
                &mut db,
                AcquirePinRequest {
                    command_id: format!("round15-late-pin-{index}"),
                    binding: binding.clone(),
                    pin_id: format!("late-archive-pin-{index}"),
                    expected_revision: "6".into(),
                    observation: observation(&binding, true, 16),
                }
            ),
            Err(CustodyError::RetirementClaimed)
        ));
        let history_rows: i64 = db.query_row(
            "SELECT count(*) FROM main.custody_commands WHERE asset_id=?1",
            [&binding.asset_id],
            |row| row.get(0),
        )?;
        assert_eq!(history_rows, 6, "rejected paths must append no receipts");
        assert_eq!(fs::read(&archive_path)?, archive);
    }

    Ok(())
}
