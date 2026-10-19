use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, PrincipalId, ProjectId, RootId,
    SchedulingState, SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
    ScopeLimits,
};
use admiralty_store::{
    Store, StoreAuthorities, StoreError,
    admission_reservations::{
        ReservationAuthority, ReservationBinding, ReservationCommand, ReservationProof,
        ledger_digest,
    },
    backup::{
        ActiveBundleRef, ArtifactClosureEntry, create_online_backup_with_closure,
        verify_backup_closure,
    },
    outbox::{self, OutboxIntent},
    projections::{
        TaskProjectionQuery, query_tasks_bounded, rebuild_projections, snapshot_projection_digest,
    },
    writer::{
        WriterHealthMetrics, commit_domain_event, ingest_observation,
        inspect_quarantined_collisions, inspect_sqlite_engine, inspect_wal_health,
        reserve_and_enqueue,
    },
};
use rusqlite::Connection;
use serde_json::json;
use std::{any::Any, sync::Arc};

struct TestBudgetCredential;

struct TestReservationPolicy;

impl ReservationAuthority for TestReservationPolicy {
    fn authorize_write(&self, _: &ReservationBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<TestBudgetCredential>()
    }

    fn authorize_read(&self, _: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<TestBudgetCredential>()
    }
}

fn budget_proof() -> ReservationProof {
    Arc::new(TestBudgetCredential)
}

fn private_tempdir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("adm-p03-test-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private tempdir")
}

fn source(producer: &str, seq: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: "boot-p03".into(),
        sequence: seq.to_string(),
    }
}

fn task_contract(project: &str, task_id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new(project).expect("project"),
        root_id: RootId::new("root-p03").expect("root"),
        task_id: TaskId::new(task_id).expect("task"),
        revision: 1,
        title: format!("Task {task_id}"),
        objective: "Verify P03 writer, projections, and backup closure".into(),
        non_goals: vec!["No external network calls".into()],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "Pass store checks".into(),
            check_ids: vec!["T1".into()],
        }],
        checks: vec![CheckSpec {
            id: "T1".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }],
        authority_ref: "policy:p03".into(),
        max_attempts: 2,
        simulated: true,
    }
}

#[test]
fn writer_diagnostics_wal_health_and_collision_quarantine_preserve_immutable_facts() {
    let dir = private_tempdir();
    let db_path = dir.path().join("state.sqlite");
    let store = Store::open(&db_path).expect("open store");

    let c1 = task_contract("proj-a", "task-1");
    let c2 = task_contract("proj-a", "task-2");
    let r1 = ingest_observation(
        &store,
        source("prod-1", 1),
        DomainEvent::TaskSubmitted {
            contract: c1.clone(),
        },
        "2026-10-19T14:00:00Z".into(),
    )
    .expect("ingest seq 1");
    assert_eq!(r1.event_seq, "1");
    assert_eq!(r1.source_contiguous_seq, "1");

    // Reusing producer sequence 1 with different content must quarantine collision
    // without replacing the original committed fact.
    let err = ingest_observation(
        &store,
        source("prod-1", 1),
        DomainEvent::TaskSubmitted {
            contract: c2.clone(),
        },
        "2026-10-19T14:01:00Z".into(),
    )
    .expect_err("content collision must be rejected");
    assert!(matches!(err, StoreError::SourceCollision { .. }));

    // Commit seq 2 with an atomic outbox intent via commit_domain_event.
    let r2 = commit_domain_event(
        &store,
        source("prod-1", 2),
        DomainEvent::TaskSubmitted { contract: c2 },
        "2026-10-19T14:02:00Z".into(),
        Some(OutboxIntent {
            idempotency_key: "notify:task-2".into(),
            payload: json!({"task":"task-2"}),
            not_before: 10,
        }),
    )
    .expect("commit seq 2 with intent");
    assert_eq!(r2.event_seq, "2");
    assert_eq!(r2.source_contiguous_seq, "2");

    drop(store);

    let conn = Connection::open(&db_path).expect("inspect sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable fk on inspection conn");
    let engine = inspect_sqlite_engine(&conn).expect("engine diagnostics");
    assert!(engine.wal_reset_fix_qualified);
    assert_eq!(engine.journal_mode, "wal");
    assert!(engine.foreign_keys_enabled);

    let wal = inspect_wal_health(&conn).expect("wal health");
    assert!(!wal.reader_blocked);
    assert!(wal.log_frames >= wal.checkpointed_frames);

    let quarantined = inspect_quarantined_collisions(&conn, 16).expect("quarantined collisions");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].producer_id, "prod-1");
    assert_eq!(quarantined[0].producer_seq, 1);
    assert_ne!(
        quarantined[0].original_digest,
        quarantined[0].conflicting_digest
    );

    let mut metrics = WriterHealthMetrics::new(64);
    metrics.record_turn(4, 180).expect("turn 1");
    metrics.record_turn(12, 420).expect("turn 2");
    metrics.record_turn(2, 95).expect("turn 3");
    assert_eq!(metrics.completed_turns, 3);
    assert_eq!(metrics.current_queue_depth, 2);
    assert_eq!(metrics.max_observed_queue_depth, 12);
    assert_eq!(metrics.last_turn_latency_micros, 95);
    assert_eq!(metrics.max_turn_latency_micros, 420);
    assert!(metrics.record_turn(65, 100).is_err());
}

#[test]
fn reserve_and_enqueue_rebuild_projections_and_bounded_queries_preserve_hash_without_effects() {
    let dir = private_tempdir();
    let db_path = dir.path().join("state.sqlite");
    let store = Store::open_with_startup_authorities(
        &db_path,
        StoreAuthorities {
            reservations: Some(Arc::new(TestReservationPolicy)),
            ..Default::default()
        },
    )
    .expect("open store with reservation authority");

    // Configure account budget limits first.
    let initial_ledger = store
        .reservation_ledger(budget_proof())
        .expect("initial ledger");
    let set_limits_cmd = ReservationCommand {
        schema_version: 1,
        command_id: "cmd-limits-1".into(),
        source: source("budget-admin", 1),
        expected_ledger_revision: initial_ledger.revision,
        expected_ledger_digest: ledger_digest(&initial_ledger).expect("ledger digest"),
        launch_intent_digest: None,
        event: ReservationEvent::SetLimits {
            event_id: "cmd-limits-1".into(),
            account_id: AccountId::new("account-main").expect("account"),
            limits: ScopeLimits {
                unit: CurrencyUnit::new("USD", 2).expect("currency"),
                account: Some(MinorUnits::new(1_000)),
                project: Some(MinorUnits::new(1_000)),
                root: Some(MinorUnits::new(1_000)),
            },
        },
    };
    store
        .record_reservation(set_limits_cmd, budget_proof())
        .expect("record budget limits");

    let contract = task_contract("proj-a", "task-alpha");
    let contract_digest = content_digest(&contract).expect("contract digest");
    ingest_observation(
        &store,
        source("prod-1", 1),
        DomainEvent::TaskSubmitted {
            contract: contract.clone(),
        },
        "2026-10-19T14:05:00Z".into(),
    )
    .expect("submit task-alpha");

    let c_beta = task_contract("proj-b", "task-beta");
    ingest_observation(
        &store,
        source("prod-1", 2),
        DomainEvent::TaskSubmitted { contract: c_beta },
        "2026-10-19T14:05:01Z".into(),
    )
    .expect("submit task-beta");

    let intent = OutboxIntent {
        idempotency_key: "launch:attempt-alpha-1".into(),
        payload: json!({"launch":"attempt-alpha-1"}),
        not_before: 0,
    };
    let launch_intent_digest = outbox::intent_digest(&intent).expect("intent digest");

    let owner = PrincipalId::new("worker-alpha").expect("owner");
    let attempt_id = AttemptId::new("attempt-alpha-1").expect("attempt");
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: attempt_id.clone(),
        owner: owner.clone(),
        incarnation: "boot-p03".into(),
        contract_revision: 1,
        contract_digest,
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: format!("sha256:{}", "1".repeat(64)),
        account_id: AccountId::new("account-main").expect("account"),
        unit: CurrencyUnit::new("USD", 2).expect("currency"),
    };
    let ledger = store
        .reservation_ledger(budget_proof())
        .expect("ledger before reserve");
    let reservation_cmd = ReservationCommand {
        schema_version: 1,
        command_id: "cmd-res-alpha-1".into(),
        source: source("prod-1", 3),
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: ledger_digest(&ledger).expect("ledger digest"),
        launch_intent_digest: Some(launch_intent_digest),
        event: ReservationEvent::Reserve {
            event_id: "cmd-res-alpha-1".into(),
            reservation_id: "res-alpha-1".into(),
            scope,
            correlation_id: "corr-alpha-1".into(),
            amount: MinorUnits::new(100),
        },
    };

    let claim_receipt = reserve_and_enqueue(
        &store,
        source("prod-1", 3),
        DomainEvent::TaskClaimed {
            task_id: contract.task_id.clone(),
            revision: 1,
            attempt_id,
            owner: owner.clone(),
        },
        "2026-10-19T14:05:02Z".into(),
        intent,
        reservation_cmd,
        budget_proof(),
    )
    .expect("atomic reserve_and_enqueue");
    assert_eq!(claim_receipt.claim.event_seq, "3");
    assert!(!claim_receipt.reservation.replayed);

    let before_snap = store.snapshot().expect("snapshot before rebuild");
    let before_digest = snapshot_projection_digest(&before_snap).expect("digest before");
    let rebuilt = rebuild_projections(&store).expect("rebuild_projections");
    assert!(!rebuilt.effects_executed);
    assert_eq!(rebuilt.projection, before_snap.projection);
    assert_eq!(rebuilt.projection_digest, before_digest);
    assert_ne!(
        rebuilt.cursor.recovery_generation,
        before_snap.cursor.recovery_generation
    );

    let page1 = query_tasks_bounded(
        &before_snap,
        &TaskProjectionQuery {
            project_id: None,
            state: None,
            owner: None,
            after_task_id: None,
            limit: 1,
        },
    )
    .expect("page 1");
    assert_eq!(page1.tasks.len(), 1);
    assert_eq!(page1.tasks[0].contract.task_id.as_str(), "task-alpha");
    assert!(page1.next_after_task_id.is_some());

    let page2 = query_tasks_bounded(
        &before_snap,
        &TaskProjectionQuery {
            project_id: None,
            state: None,
            owner: None,
            after_task_id: page1.next_after_task_id,
            limit: 1,
        },
    )
    .expect("page 2");
    assert_eq!(page2.tasks.len(), 1);
    assert_eq!(page2.tasks[0].contract.task_id.as_str(), "task-beta");
    assert!(page2.next_after_task_id.is_none());

    let claimed_only = query_tasks_bounded(
        &before_snap,
        &TaskProjectionQuery {
            project_id: Some(ProjectId::new("proj-a").expect("proj-a")),
            state: Some(SchedulingState::Claimed),
            owner: Some(owner),
            after_task_id: None,
            limit: 10,
        },
    )
    .expect("filtered query");
    assert_eq!(claimed_only.tasks.len(), 1);
    assert_eq!(
        claimed_only.tasks[0].contract.task_id.as_str(),
        "task-alpha"
    );
}

#[test]
fn online_backup_with_artifact_closure_verifies_and_keeps_p25_restore_authority_false() {
    let dir = private_tempdir();
    let db_path = dir.path().join("live.sqlite");
    let backup_path = dir.path().join("backup.sqlite");
    let manifest_path = dir.path().join("backup.closure.json");

    let store = Store::open(&db_path).expect("open store");
    ingest_observation(
        &store,
        source("prod-1", 1),
        DomainEvent::TaskSubmitted {
            contract: task_contract("proj-backup", "task-b1"),
        },
        "2026-10-19T14:10:00Z".into(),
    )
    .expect("append task-b1");

    let artifacts = vec![ArtifactClosureEntry {
        artifact_id: "art-receipt-1".into(),
        content_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .into(),
        size_bytes: 4096,
        pin_id: "pin-backup-1".into(),
        verified_available: true,
    }];
    let bundles = vec![ActiveBundleRef {
        target_id: "target-primary".into(),
        bundle_id: "bundle-c58a67c0".into(),
        version: "0.2.0".into(),
        artifact_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .into(),
    }];

    // Unverified artifact availability must be rejected before creating backup.
    let mut unverified = artifacts.clone();
    unverified[0].verified_available = false;
    assert!(
        create_online_backup_with_closure(
            &store,
            &backup_path,
            &manifest_path,
            unverified,
            bundles.clone()
        )
        .is_err()
    );

    let manifest =
        create_online_backup_with_closure(&store, &backup_path, &manifest_path, artifacts, bundles)
            .expect("create online backup with closure");
    assert!(!manifest.full_restore_authority_granted);
    assert_eq!(manifest.snapshot_cursor.event_seq, "1");
    assert!(manifest.backup_file_sha256.starts_with("sha256:"));
    assert!(manifest.backup_file_size_bytes > 0);

    let verified =
        verify_backup_closure(&backup_path, &manifest_path).expect("verify backup closure");
    assert_eq!(verified, manifest);
}
