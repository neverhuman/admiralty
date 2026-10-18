use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_store::{
    artifacts::ArtifactStore,
    custody::{
        self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyObservation,
        RegisterAssetRequest,
    },
};
use rusqlite::Connection;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    sync::{Arc, Barrier},
    thread,
    time::Duration,
};
use tempfile::TempDir;

fn private_fixture() -> TempDir {
    // Resolve aliases such as macOS /var before exercising strict custody paths.
    let base = fs::canonicalize(std::env::temp_dir()).expect("canonical temporary base");
    tempfile::Builder::new()
        .prefix("admiralty-artifact-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(base)
        .expect("create owner-only disposable fixture")
}

fn binding(reference_sha256: &str) -> AssetBinding {
    AssetBinding {
        project_id: ProjectId::new("project-1").expect("valid project fixture"),
        root_id: RootId::new("root-1").expect("valid root fixture"),
        task_id: TaskId::new("task-1").expect("valid task fixture"),
        asset_id: "asset-1".into(),
        owner_id: PrincipalId::new("owner-1").expect("valid owner fixture"),
        sha256: reference_sha256.into(),
        generation: "1".into(),
    }
}

fn observation(binding: &AssetBinding, expected_revision: &str) -> CustodyObservation {
    let (observed_at, decision_at, registered) = if expected_revision == "0" {
        ("10", "11", Some(false))
    } else {
        ("11", "12", Some(true))
    };
    CustodyObservation {
        binding: binding.clone(),
        asset_revision: expected_revision.into(),
        clock_id: "test-clock".into(),
        observed_at: observed_at.into(),
        decision_at: decision_at.into(),
        max_age: "10".into(),
        provenance: "fixture:artifact-custody".into(),
        registered,
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    }
}

fn connection(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open custody fixture database");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("set bounded SQLite busy timeout");
    connection
}

#[test]
fn publication_replay_and_pin_retirement_race_preserve_artifact_bytes() {
    let fixture = private_fixture();
    let artifact_root = fixture.path().join("objects");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&artifact_root)
        .expect("create owner-only artifact root");

    let bytes = b"immutable candidate evidence";
    let artifacts = ArtifactStore::open(&artifact_root, 1024).expect("open artifact store");
    let reference = artifacts
        .publish_bytes(bytes)
        .expect("publish content-addressed evidence");
    assert_eq!(reference.size_bytes, bytes.len() as u64);
    assert_eq!(
        artifacts.read_verified(&reference).expect("verify object"),
        bytes
    );
    assert_eq!(
        artifacts
            .publish_bytes(bytes)
            .expect("exact publication replay is idempotent"),
        reference
    );

    let database_path = fixture.path().join("custody.sqlite");
    let mut first_connection = connection(&database_path);
    custody::initialize(&first_connection).expect("initialize custody schema");
    let asset = binding(&reference.sha256);
    let registration = RegisterAssetRequest {
        command_id: "register-asset".into(),
        binding: asset.clone(),
        expected_revision: "0".into(),
        observation: observation(&asset, "0"),
    };
    let registered = custody::register_asset(&mut first_connection, registration.clone())
        .expect("record clean artifact custody");
    let receipt = serde_json::to_value(&registered).expect("serialize custody receipt");
    assert_eq!(receipt["recorded_only"].as_bool(), Some(true));
    assert_eq!(receipt["destructive_permission"].as_bool(), Some(false));

    drop(first_connection);
    let mut reopened = connection(&database_path);
    custody::initialize(&reopened).expect("reopen custody schema after process boundary");
    assert_eq!(
        custody::register_asset(&mut reopened, registration)
            .expect("exact command replay returns durable receipt"),
        registered
    );
    drop(reopened);

    let second_connection = connection(&database_path);
    custody::initialize(&second_connection).expect("initialize second real connection");
    let barrier = Arc::new(Barrier::new(3));

    let pin_barrier = Arc::clone(&barrier);
    let pin_asset = asset.clone();
    let mut pin_connection = connection(&database_path);
    let pin = thread::spawn(move || {
        pin_barrier.wait();
        custody::acquire_pin(
            &mut pin_connection,
            AcquirePinRequest {
                command_id: "race-pin".into(),
                binding: pin_asset.clone(),
                pin_id: "live-candidate".into(),
                expected_revision: "1".into(),
                observation: observation(&pin_asset, "1"),
            },
        )
    });

    let claim_barrier = Arc::clone(&barrier);
    let claim_asset = asset.clone();
    let mut claim_connection = second_connection;
    let claim = thread::spawn(move || {
        claim_barrier.wait();
        custody::claim_retirement(
            &mut claim_connection,
            ClaimRetirementRequest {
                command_id: "race-claim".into(),
                binding: claim_asset.clone(),
                claim_id: "retire-candidate".into(),
                expected_revision: "1".into(),
                observation: observation(&claim_asset, "1"),
            },
        )
    });

    barrier.wait();
    let pin_result = pin.join().expect("pin worker completes");
    let claim_result = claim.join().expect("retirement worker completes");
    assert_ne!(pin_result.is_ok(), claim_result.is_ok());

    let reopened_artifacts = ArtifactStore::open(&artifact_root, 1024)
        .expect("reopen artifact store after custody race");
    assert_eq!(
        reopened_artifacts
            .read_verified(&reference)
            .expect("custody metadata never removes a live object"),
        bytes
    );
    assert_eq!(
        fs::read_dir(&artifact_root)
            .expect("inspect durable object directory")
            .count(),
        1,
        "successful publication leaves one immutable object and no temporary file"
    );
}

#[test]
fn stale_revision_cannot_acquire_a_custody_pin() {
    let fixture = TempDir::new().expect("create disposable fixture root");
    let artifact_root = fixture.path().join("objects");
    fs::create_dir(&artifact_root).expect("create artifact root");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("make artifact root owner-only");
    let reference = ArtifactStore::open(&artifact_root, 1024)
        .expect("open artifact store")
        .publish_bytes(b"stale revision candidate")
        .expect("publish immutable evidence");

    let database_path = fixture.path().join("custody.sqlite");
    let mut connection = connection(&database_path);
    custody::initialize(&connection).expect("initialize custody schema");
    let asset = binding(&reference.sha256);
    custody::register_asset(
        &mut connection,
        RegisterAssetRequest {
            command_id: "register-stale-cas".into(),
            binding: asset.clone(),
            expected_revision: "0".into(),
            observation: observation(&asset, "0"),
        },
    )
    .expect("register asset at revision one");

    let result = custody::acquire_pin(
        &mut connection,
        AcquirePinRequest {
            command_id: "stale-pin".into(),
            binding: asset.clone(),
            pin_id: "stale-candidate".into(),
            expected_revision: "0".into(),
            observation: observation(&asset, "0"),
        },
    );
    assert!(matches!(result, Err(custody::CustodyError::StaleRevision)));

    let pin_count: i64 = connection
        .query_row("SELECT count(*) FROM custody_pins", [], |row| row.get(0))
        .expect("inspect pin projection after rejected compare-and-swap");
    assert_eq!(pin_count, 0, "stale request must not create a live pin");
    let stored_revision: i64 = connection
        .query_row(
            "SELECT revision FROM custody_assets WHERE asset_id=?1",
            [&asset.asset_id],
            |row| row.get(0),
        )
        .expect("inspect custody revision after rejected compare-and-swap");
    assert_eq!(
        stored_revision, 1,
        "rejected stale request must not advance the asset"
    );
}

#[test]
fn publication_rejects_object_one_byte_over_configured_limit() {
    let fixture = TempDir::new().expect("create disposable fixture root");
    let artifact_root = fixture.path().join("objects");
    fs::create_dir(&artifact_root).expect("create artifact root");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("make artifact root owner-only");
    let artifacts = ArtifactStore::open(&artifact_root, 1).expect("open one-byte artifact store");

    let at_limit = artifacts
        .publish_bytes(&[0xA5])
        .expect("one byte at configured limit is accepted");
    assert_eq!(at_limit.size_bytes, 1);
    assert!(artifacts.publish_bytes(&[0xA5, 0x5A]).is_err());
    assert_eq!(
        artifacts
            .publish_bytes(&[0xA5])
            .expect("exact replay after rejected oversize write remains valid"),
        at_limit,
        "replayed publication must preserve the original content address"
    );
    assert_eq!(
        fs::read_dir(&artifact_root)
            .expect("inspect object directory after oversize rejection")
            .count(),
        1,
        "oversize publication must not leave an object or temporary file"
    );
}

#[test]
fn verified_read_rejects_corrupted_bytes_at_the_content_address() {
    let fixture = TempDir::new().expect("create disposable fixture root");
    let artifact_root = fixture.path().join("objects");
    fs::create_dir(&artifact_root).expect("create artifact root");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("make artifact root owner-only");
    let artifacts = ArtifactStore::open(&artifact_root, 1024).expect("open artifact store");
    let reference = artifacts
        .publish_bytes(b"verified source bytes")
        .expect("publish content-addressed bytes");
    let object_path = artifact_root.join(
        reference
            .sha256
            .strip_prefix("sha256:")
            .expect("artifact digest has canonical prefix"),
    );
    fs::write(&object_path, b"cross-boundary corruption")
        .expect("simulate corrupted object at its digest path");

    assert!(artifacts.read_verified(&reference).is_err());
    assert_eq!(
        fs::read(&object_path).expect("corrupt object remains unchanged"),
        b"cross-boundary corruption",
        "verified read must reject rather than silently accept corrupted bytes"
    );
}
