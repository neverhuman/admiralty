use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::{
    artifacts::ArtifactStore,
    custody::{
        self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyError,
        CustodyObservation, RegisterAssetRequest, ReleasePinRequest,
    },
};
use rusqlite::Connection;
use std::{error::Error, fs, os::unix::fs::PermissionsExt, path::Path};

fn observation(
    binding: &AssetBinding,
    registered: bool,
    revision: &str,
    at: u64,
) -> CustodyObservation {
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: revision.into(),
        clock_id: "round18-p20-bundle-clock".into(),
        observed_at: at.to_string(),
        decision_at: at.saturating_add(1).to_string(),
        max_age: "10".into(),
        provenance: "fixture:round18-release-bundle-schema-parity".into(),
        registered: Some(registered),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

fn schema_snapshot(
    connection: &Connection,
) -> Result<Vec<(String, String, String)>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT type, name, sql FROM main.sqlite_schema \
         WHERE name GLOB 'custody_*' AND sql IS NOT NULL ORDER BY type, name",
    )?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect()
}

fn prepare_database(path: &Path) -> Result<Connection, Box<dyn Error>> {
    let connection = Connection::open(path)?;
    custody::initialize(&connection)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS unrelated_component_state(value TEXT NOT NULL);",
    )?;
    connection.execute(
        "INSERT INTO unrelated_component_state(value) VALUES ('preserve-me')",
        [],
    )?;
    Ok(connection)
}

#[test]
fn p20_round18_release_bundle_schema_parity_qualification() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let object_root = fixture.path().join("objects");
    fs::create_dir(&object_root)?;
    fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))?;
    let release_bytes = b"round18 signed-off release bundle and archival evidence";
    let artifacts = ArtifactStore::open(&object_root, 1024)?;
    let reference = artifacts.publish_bytes(release_bytes)?;
    assert_eq!(artifacts.read_verified(&reference)?, release_bytes);
    assert_eq!(artifacts.publish_bytes(release_bytes)?, reference);

    let first_path = fixture.path().join("custody-first.sqlite");
    let second_path = fixture.path().join("custody-second.sqlite");
    let mut first = prepare_database(&first_path)?;
    let second = prepare_database(&second_path)?;
    let first_schema = schema_snapshot(&first)?;
    let second_schema = schema_snapshot(&second)?;
    assert_eq!(
        first_schema, second_schema,
        "initialized custody schema must be byte-identical across databases"
    );
    assert_eq!(
        first_schema.len(),
        4,
        "three tables and the active-pin index are component-owned"
    );
    let user_version: i64 = first.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(
        user_version, 0,
        "custody initialization must not claim global schema version"
    );
    let preserved: String =
        first.query_row("SELECT value FROM unrelated_component_state", [], |row| {
            row.get(0)
        })?;
    assert_eq!(preserved, "preserve-me");
    drop(second);

    let binding = AssetBinding {
        project_id: ProjectId::new("round18-project")?,
        root_id: RootId::new("round18-root")?,
        task_id: TaskId::new("round18-release-task")?,
        asset_id: "round18-release-bundle".into(),
        owner_id: PrincipalId::new("round18-owner")?,
        sha256: reference.sha256.clone(),
        generation: "18".into(),
    };
    let registration = RegisterAssetRequest {
        command_id: "round18-register-bundle".into(),
        binding: binding.clone(),
        expected_revision: "0".into(),
        observation: observation(&binding, false, "0", 10),
    };
    let registration_receipt = custody::register_asset(&mut first, registration.clone())?;
    assert_eq!(
        custody::register_asset(&mut first, registration)?,
        registration_receipt
    );

    let pin = AcquirePinRequest {
        command_id: "round18-pin-release-review".into(),
        binding: binding.clone(),
        pin_id: "release-review".into(),
        expected_revision: "1".into(),
        observation: observation(&binding, true, "1", 11),
    };
    let pin_receipt = custody::acquire_pin(&mut first, pin.clone())?;
    assert_eq!(custody::acquire_pin(&mut first, pin)?, pin_receipt);
    assert!(matches!(
        custody::claim_retirement(
            &mut first,
            ClaimRetirementRequest {
                command_id: "round18-blocked-retirement".into(),
                binding: binding.clone(),
                claim_id: "round18-retirement".into(),
                expected_revision: "2".into(),
                observation: observation(&binding, true, "2", 12),
            },
        ),
        Err(CustodyError::ActivePins)
    ));

    let release = ReleasePinRequest {
        command_id: "round18-release-review".into(),
        binding: binding.clone(),
        pin_id: "release-review".into(),
        expected_revision: "2".into(),
        observation: observation(&binding, true, "2", 12),
    };
    let release_receipt = custody::release_pin(&mut first, release.clone())?;
    assert_eq!(custody::release_pin(&mut first, release)?, release_receipt);
    let retirement = ClaimRetirementRequest {
        command_id: "round18-retire-bundle".into(),
        binding: binding.clone(),
        claim_id: "round18-retirement".into(),
        expected_revision: "3".into(),
        observation: observation(&binding, true, "3", 13),
    };
    let retirement_receipt = custody::claim_retirement(&mut first, retirement.clone())?;
    let serialized_receipt = serde_json::to_value(&retirement_receipt)?;
    assert_eq!(serialized_receipt["recorded_only"], true);
    assert_eq!(serialized_receipt["destructive_permission"], false);
    assert_eq!(
        custody::claim_retirement(&mut first, retirement.clone())?,
        retirement_receipt
    );
    drop(first);

    let mut reopened = Connection::open(&first_path)?;
    custody::initialize(&reopened)?;
    assert_eq!(schema_snapshot(&reopened)?, first_schema);
    assert_eq!(
        custody::claim_retirement(&mut reopened, retirement)?,
        retirement_receipt
    );
    let reopened_artifacts = ArtifactStore::open(&object_root, 1024)?;
    assert_eq!(reopened_artifacts.read_verified(&reference)?, release_bytes);
    Ok(())
}
