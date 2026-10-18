use admiralty_core::{
    AcceptanceCriterion, AttemptId, CONTRACT_VERSION, CheckSpec, CoordinationMessage, DomainEvent,
    MessageId, MessagePayload, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract,
    TaskId, content_digest,
};
use admiralty_store::{Store, StoreError};
use std::{fs, path::Path};
use tempfile::{Builder, TempDir};

const OBSERVED_UTC: &str = "2026-10-18T00:00:00Z";
const FIXTURE_PARENT: &str = "/home/ubuntu/admiralty-v2-build/targets/adm_pool_adm_p17a";

fn private_tempdir() -> TempDir {
    let directory = Builder::new()
        .prefix("backup-reopen-")
        .tempdir_in(FIXTURE_PARENT)
        .expect("private test-owned fixture directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("make test-owned fixture directory private");
        assert_eq!(
            directory
                .path()
                .metadata()
                .expect("fixture directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "fixture directory must be private: {}",
            directory.path().display()
        );
    }
    directory
}

fn contract() -> TaskContract {
    TaskContract {
        schema_version: CONTRACT_VERSION,
        project_id: ProjectId::new("backup-project").expect("fixture project ID"),
        root_id: RootId::new("backup-root").expect("fixture root ID"),
        task_id: TaskId::new("BACKUP-TASK").expect("fixture task ID"),
        revision: 1,
        title: "Backup fixture".into(),
        objective: "Exercise durable task replay".into(),
        non_goals: vec![],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "criterion".into(),
            text: "The event is replayable".into(),
            check_ids: vec!["fixture-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "fixture-check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 1,
        }],
        authority_ref: "fixture-authority".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn append(store: &Store, sequence: u64, event: DomainEvent) -> String {
    store
        .append(
            SourceIdentity {
                producer_id: "backup-test".into(),
                incarnation: "fixture-run-1".into(),
                sequence: sequence.to_string(),
            },
            event,
            OBSERVED_UTC.into(),
        )
        .expect("fixture append")
        .event_seq
}

fn private_regular_file(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
        .open(path)
        .expect("create fixture file")
        .write_all(bytes)
        .expect("write fixture file");
}

#[test]
fn backup_reopens_at_exact_watermark_and_rebuilds_its_projection() {
    let directory = private_tempdir();
    let source_path = directory.path().join("source.sqlite");
    let backup_path = directory.path().join("snapshot.sqlite");
    let store = Store::open(&source_path).expect("open fixture source");
    let task = contract();
    let task_id = task.task_id.clone();
    let project_id = task.project_id.clone();
    let contract_digest = content_digest(&task).expect("fixture contract digest");

    let task_seq = append(&store, 1, DomainEvent::TaskSubmitted { contract: task });
    let claim_seq = append(
        &store,
        2,
        DomainEvent::TaskClaimed {
            task_id: task_id.clone(),
            revision: 1,
            attempt_id: AttemptId::new("backup-attempt").expect("fixture attempt ID"),
            owner: PrincipalId::new("backup-worker").expect("fixture owner ID"),
        },
    );
    let message_seq = append(
        &store,
        3,
        DomainEvent::MessageQueued {
            message: CoordinationMessage {
                schema_version: 1,
                message_id: MessageId::new("backup-message").expect("fixture message ID"),
                project_id,
                sender: PrincipalId::new("backup-captain").expect("fixture sender ID"),
                recipient: PrincipalId::new("backup-worker").expect("fixture recipient ID"),
                causation_id: None,
                context_epoch: "fixture-epoch-1".into(),
                authority_ref: "fixture-authority".into(),
                payload: MessagePayload::TaskAssigned {
                    task_id: task_id.clone(),
                    revision: 1,
                    contract_digest,
                },
            },
        },
    );
    assert!(task_seq.parse::<u64>().expect("task event sequence") > 0);
    assert!(claim_seq.parse::<u64>().expect("claim event sequence") > 0);
    let before_backup = store.snapshot().expect("source snapshot before backup");
    assert_eq!(before_backup.cursor.event_seq, message_seq);
    assert_eq!(before_backup.projection.messages.len(), 1);
    assert_eq!(before_backup.projection.tasks.len(), 1);

    let returned = store.backup(&backup_path).expect("SQLite online backup");
    assert_eq!(
        returned, before_backup,
        "backup receipt must report its exact serialized source watermark"
    );
    let backup_metadata = fs::symlink_metadata(&backup_path).expect("backup artifact metadata");
    assert!(
        backup_metadata.is_file(),
        "backup artifact is a regular file"
    );
    assert!(
        !backup_metadata.file_type().is_symlink(),
        "backup artifact is not a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        assert_eq!(backup_metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(backup_metadata.nlink(), 1);
    }
    assert_eq!(
        store
            .snapshot()
            .expect("source remains unchanged by backup"),
        before_backup
    );

    let second_writer = match Store::open(&source_path) {
        Err(error) => error,
        Ok(second) => {
            drop(second);
            panic!("existing source owner must block a second writer");
        }
    };
    assert!(matches!(second_writer, StoreError::AlreadyOwned));
    assert_eq!(
        store
            .snapshot()
            .expect("source after rejected second writer"),
        before_backup
    );

    let post_backup_seq = append(
        &store,
        4,
        DomainEvent::TaskStarted {
            task_id: task_id.clone(),
            attempt_id: AttemptId::new("backup-attempt").expect("fixture attempt ID"),
        },
    );
    let current_source = store.snapshot().expect("source after later append");
    assert_eq!(current_source.cursor.event_seq, post_backup_seq);
    assert_ne!(current_source.cursor.event_seq, returned.cursor.event_seq);

    let reopened = Store::open(&backup_path).expect("open the offline SQLite snapshot once");
    let reopened_snapshot = reopened.snapshot().expect("reopened snapshot");
    assert_eq!(
        reopened_snapshot, returned,
        "backup contents must match returned snapshot and watermark"
    );
    assert_eq!(
        reopened_snapshot.projection.tasks[&task_id]
            .attempt_id
            .as_ref()
            .expect("retained claim"),
        &AttemptId::new("backup-attempt").expect("fixture attempt ID")
    );
    assert_eq!(
        reopened_snapshot.projection.tasks[&task_id].state,
        admiralty_core::SchedulingState::Claimed
    );

    let rebuilt = reopened
        .rebuild()
        .expect("rebuild projection from backup events");
    assert_eq!(rebuilt.cursor.event_seq, returned.cursor.event_seq);
    assert_eq!(rebuilt.projection, returned.projection);
    assert_ne!(
        rebuilt.cursor.recovery_generation,
        returned.cursor.recovery_generation
    );
    assert!(
        !rebuilt.cursor.recovery_generation.is_empty(),
        "rebuild generation is observed, never fabricated as zero"
    );
    assert_eq!(
        reopened
            .snapshot()
            .expect("snapshot after rebuild")
            .projection,
        returned.projection
    );

    drop(reopened);
    drop(store);
}

#[test]
fn failed_backup_does_not_overwrite_or_leave_a_stage_file() {
    let directory = private_tempdir();
    let source_path = directory.path().join("source.sqlite");
    let store = Store::open(&source_path).expect("open fixture source");
    let existing = directory.path().join("existing.sqlite");
    private_regular_file(&existing, b"preserve this existing destination");
    let before = fs::read(&existing).expect("read existing destination");
    let mut names_before: Vec<_> = fs::read_dir(directory.path())
        .expect("list fixture directory")
        .map(|entry| entry.expect("fixture entry").file_name())
        .collect();
    names_before.sort();

    let failure = store
        .backup(&existing)
        .expect_err("existing destinations are never overwritten");
    assert!(matches!(failure, StoreError::Invalid(_)));
    assert_eq!(
        fs::read(&existing).expect("existing destination remains"),
        before
    );
    let mut names_after: Vec<_> = fs::read_dir(directory.path())
        .expect("list fixture directory after failed backup")
        .map(|entry| entry.expect("fixture entry").file_name())
        .collect();
    names_after.sort();
    assert_eq!(
        names_after, names_before,
        "failed backup must clean its private staging file"
    );
    drop(store);
}

#[cfg(unix)]
#[test]
fn symlink_destinations_and_unsafe_database_parents_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = private_tempdir();
    let source_path = directory.path().join("source.sqlite");
    let store = Store::open(&source_path).expect("open fixture source");
    let target = directory.path().join("target.txt");
    private_regular_file(&target, b"symlink target remains unchanged");
    let link = directory.path().join("linked-backup.sqlite");
    symlink(&target, &link).expect("create fixture destination symlink");
    assert!(matches!(store.backup(&link), Err(StoreError::Invalid(_))));
    assert_eq!(
        fs::read(&target).expect("symlink target remains"),
        b"symlink target remains unchanged"
    );
    drop(store);

    let source_target = directory.path().join("source-target.sqlite");
    private_regular_file(&source_target, b"not a sqlite source");
    let source_link = directory.path().join("source-link.sqlite");
    symlink(&source_target, &source_link).expect("create fixture source symlink");
    assert!(matches!(
        Store::open(&source_link),
        Err(StoreError::Invalid(_))
    ));

    let unsafe_parent = directory.path().join("world-writable");
    fs::create_dir(&unsafe_parent).expect("create unsafe-parent fixture");
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777))
        .expect("set fixture parent mode");
    let unsafe_db = unsafe_parent.join("unsafe.sqlite");
    assert!(matches!(
        Store::open(&unsafe_db),
        Err(StoreError::Invalid(_))
    ));
    assert!(
        !unsafe_db.exists(),
        "unsafe database path is rejected before SQLite creates it"
    );
}
