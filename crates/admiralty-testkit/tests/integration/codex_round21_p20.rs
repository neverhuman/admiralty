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
        asset_revision: revision.to_owned(),
        clock_id: "round21-hermetic-release-clock".into(),
        observed_at: tick.to_string(),
        decision_at: tick.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round21-post-item6-ci-qualification".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

#[test]
fn p20_round21_post_item6_hermetic_ci_cross_packet_release_qualification()
-> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let fixture_root = fixture.path().canonicalize()?;
    let object_root = fixture_root.join("content-addressed-archive");
    fs::create_dir(&object_root)?;
    fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))?;
    let bundle = b"round21 post-item-6 release bundle with archival receipt";
    let artifact_store = ArtifactStore::open(&object_root, 2048)?;
    let artifact = artifact_store.publish_bytes(bundle)?;
    assert_eq!(artifact_store.read_verified(&artifact)?, bundle);
    assert_eq!(artifact_store.publish_bytes(bundle)?, artifact);

    let database_path = fixture_root.join("custody.sqlite");
    let mut db = Connection::open(&database_path)?;
    custody::initialize(&db)?;
    let binding = AssetBinding {
        project_id: ProjectId::new("round21-cross-packet-project")?,
        root_id: RootId::new("round21-retained-root")?,
        task_id: TaskId::new("round21-release-task")?,
        asset_id: "round21-item6-bundle".into(),
        owner_id: PrincipalId::new("round21-custodian")?,
        sha256: artifact.sha256.clone(),
        generation: "21".into(),
    };
    let registration = RegisterAssetRequest {
        command_id: "round21-register-bundle".into(),
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
        command_id: "round21-pin-consumer".into(),
        binding: binding.clone(),
        pin_id: "cross-packet-consumer".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, "1", 11),
    };
    let pin_receipt = custody::acquire_pin(&mut db, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut db, pin)?, pin_receipt);

    // Another packet's root cannot claim or unpin this recorded asset.
    let mut foreign = binding.clone();
    foreign.root_id = RootId::new("round21-foreign-root")?;
    assert!(matches!(
        custody::release_pin(
            &mut db,
            ReleasePinRequest {
                command_id: "round21-foreign-release".into(),
                binding: foreign.clone(),
                pin_id: "cross-packet-consumer".into(),
                expected_revision: "2".into(),
                observation: observation(&foreign, true, "2", 12),
            },
        ),
        Err(CustodyError::ForeignAsset)
    ));
    assert!(matches!(
        custody::claim_retirement(
            &mut db,
            ClaimRetirementRequest {
                command_id: "round21-retirement-before-release".into(),
                binding: binding.clone(),
                claim_id: "round21-item6-retirement".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round21-release-consumer".into(),
        binding: binding.clone(),
        pin_id: "cross-packet-consumer".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, "2", 12),
    };
    let release_receipt = custody::release_pin(&mut db, release.clone())?;
    assert_eq!(custody::release_pin(&mut db, release)?, release_receipt);

    let retirement = ClaimRetirementRequest {
        command_id: "round21-retire-item6-bundle".into(),
        binding: binding.clone(),
        claim_id: "round21-item6-retirement".into(),
        expected_revision: "3".into(),
        observation: observation(&binding, true, "3", 13),
    };
    let retirement_receipt = custody::claim_retirement(&mut db, retirement.clone())?;
    let archival_receipt = serde_json::to_value(&retirement_receipt)?;
    assert!(archival_receipt["recorded_only"].as_bool().unwrap_or(false));
    assert!(
        !archival_receipt["destructive_permission"]
            .as_bool()
            .unwrap_or(true)
    );
    assert_eq!(
        custody::claim_retirement(&mut db, retirement.clone())?,
        retirement_receipt
    );
    assert!(database_path.starts_with(&fixture_root));
    assert!(object_root.starts_with(&fixture_root));
    drop(db);

    let mut reopened = Connection::open(&database_path)?;
    custody::initialize(&reopened)?;
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        retirement_receipt
    );
    let reopened_store = ArtifactStore::open(&object_root, 2048)?;
    assert_eq!(reopened_store.read_verified(&artifact)?, bundle);
    Ok(())
}
