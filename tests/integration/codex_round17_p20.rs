use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::custody::{
    self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
    CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{error::Error, fs};

fn evidence(
    binding: &AssetBinding,
    registered: bool,
    revision: &str,
    at: u64,
) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: revision.into(),
        clock_id: "round17-cross-packet-clock".into(),
        observed_at: at.to_string(),
        decision_at: at.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round17-post-item5-release-audit".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round17_post_item5_release_audit_cross_packet_invariants() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let archive = fixture.path().join("release-archive");
    fs::create_dir(&archive)?;
    let bytes = b"round17 cross-packet retirement evidence";
    let archive_path = archive.join("item-5-receipt.json");
    fs::write(&archive_path, bytes)?;
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let database = fixture.path().join("custody.sqlite");

    let binding = AssetBinding {
        project_id: ProjectId::new("round17-project")?,
        root_id: RootId::new("round17-root")?,
        task_id: TaskId::new("round17-task")?,
        asset_id: "round17-item5-receipt".into(),
        owner_id: PrincipalId::new("round17-owner")?,
        sha256: digest.clone(),
        generation: "11".into(),
    };
    let mut db = Connection::open(&database)?;
    custody::initialize(&db)?;
    let registration = RegisterAssetRequest {
        command_id: "round17-register".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: evidence(&binding, false, "0", 10),
    };
    let registration_receipt = custody::register_asset(&mut db, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut db, registration)?,
        registration_receipt
    );

    let pin = AcquirePinRequest {
        command_id: "round17-pin-downstream-consumer".into(),
        binding: binding.clone(),
        pin_id: "downstream-consumer".into(),
        expected_revision: "1".into(),
        observation: evidence(&binding, true, "1", 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut db, pin)?, pin_receipt);

    // P20 binding identity cannot be crossed by another packet's root or task.
    let mut foreign_binding = binding.clone();
    foreign_binding.root_id = RootId::new("round17-other-root")?;
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round17-foreign-root-retirement".into(),
                binding: foreign_binding.clone(),
                claim_id: "round17-claim".into(),
                expected_revision: "2".into(),
                observation: evidence(&foreign_binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ForeignAsset)
    ));

    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round17-pinned-retirement".into(),
                binding: binding.clone(),
                claim_id: "round17-claim".into(),
                expected_revision: "2".into(),
                observation: evidence(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));
    assert!(matches!(
        custody::release_pin(
            &mut db,
            ReleasePinRequest {
                command_id: "round17-stale-release".into(),
                binding: binding.clone(),
                pin_id: "downstream-consumer".into(),
                expected_revision: "1".into(),
                observation: evidence(&binding, true, "1", 12),
            },
        ),
        Err(CustodyError::StaleRevision)
    ));
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round17-still-pinned".into(),
                binding: binding.clone(),
                claim_id: "round17-claim".into(),
                expected_revision: "2".into(),
                observation: evidence(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round17-release-downstream-consumer".into(),
        binding: binding.clone(),
        pin_id: "downstream-consumer".into(),
        expected_revision: "2".into(),
        observation: evidence(&binding, true, "2", 12),
    };
    let release_receipt = custody::release_pin(&mut db, release.clone())?;
    assert_eq!(custody::release_pin(&mut db, release)?, release_receipt);

    let retirement = ClaimRetirementRequest {
        command_id: "round17-retire-item5".into(),
        binding: binding.clone(),
        claim_id: "round17-claim".into(),
        expected_revision: "3".into(),
        observation: evidence(&binding, true, "3", 13),
    };
    let receipt = custody::claim_retirement(&mut db, retirement.clone())?;
    let receipt_json = serde_json::to_value(&receipt)?;
    assert_eq!(receipt_json["recorded_only"], true);
    assert_eq!(receipt_json["destructive_permission"], false);
    assert_eq!(
        custody::claim_retirement(&mut db, retirement.clone())?,
        receipt
    );
    assert_eq!(fs::read(&archive_path)?, bytes);
    assert_eq!(
        format!("sha256:{:x}", Sha256::digest(fs::read(&archive_path)?)),
        digest
    );
    drop(db);

    let mut reopened = Connection::open(&database)?;
    custody::initialize(&reopened)?;
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        receipt,
        "archival receipt replay remains byte-stable after reopen"
    );
    assert_eq!(fs::read(&archive_path)?, bytes);
    Ok(())
}
