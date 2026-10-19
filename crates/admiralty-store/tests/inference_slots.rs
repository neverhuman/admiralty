//! Durable inference-slot admission through the Store's sole writer.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, PrincipalId, ProjectId, RootId,
    SchedulingState, SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
    ScopeLimits, StopReason,
};
use admiralty_store::{
    Store, StoreAuthorities,
    admission_reservations::{
        self, ReservationAuthority, ReservationBinding, ReservationCommand, ReservationProof,
    },
    inference_slots::{
        self, InferenceSlotAuthority, SlotBinding, SlotClaim, SlotCommand, SlotEvent, SlotLedger,
        SlotProof,
    },
    outbox::{self, OutboxIntent},
};
use serde_json::json;
use std::{
    any::Any,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::{Builder, TempDir};

struct Credential;
#[derive(Default)]
struct Policy {
    revoked: AtomicBool,
    deny_staged_claim: AtomicBool,
    staged_claim_seen: AtomicBool,
}
impl Policy {
    fn permits(&self, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Credential>() && !self.revoked.load(Ordering::SeqCst)
    }
}
impl ReservationAuthority for Policy {
    fn authorize_write(&self, _: &ReservationBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        self.permits(proof)
    }
    fn authorize_read(&self, _: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Credential>() && !self.revoked.load(Ordering::SeqCst)
    }
}
impl InferenceSlotAuthority for Policy {
    fn authorize_write(&self, binding: &SlotBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        let staged_claim = binding.task.is_some_and(|task| {
            task.state == SchedulingState::Claimed
                && matches!(
                    &binding.command.event,
                    SlotEvent::Hold { claim, .. }
                        if task.contract.task_id == claim.scope.task_id
                            && task.attempt_id.as_ref() == Some(&claim.scope.attempt_id)
                            && task.owner.as_ref() == Some(&claim.scope.owner)
                )
        });
        if staged_claim && self.deny_staged_claim.load(Ordering::SeqCst) {
            self.staged_claim_seen.store(true, Ordering::SeqCst);
            return false;
        }
        self.permits(proof)
    }
    fn authorize_read(&self, _: &SlotLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Credential>() && !self.revoked.load(Ordering::SeqCst)
    }
}
fn proof() -> ReservationProof {
    Arc::new(Credential)
}
fn slot_proof() -> SlotProof {
    Arc::new(Credential)
}
fn private_dir() -> TempDir {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::PermissionsExt;
        let mut b = Builder::new();
        b.prefix("inference-slots-")
            .permissions(fs::Permissions::from_mode(0o700));
        b
    };
    #[cfg(not(unix))]
    let builder = {
        let mut b = Builder::new();
        b.prefix("inference-slots-");
        b
    };
    let dir = builder.tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            dir.path().metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    dir
}
fn clone_admission(
    a: &admiralty_store::BudgetAndSlotAdmission,
) -> admiralty_store::BudgetAndSlotAdmission {
    admiralty_store::BudgetAndSlotAdmission {
        budget_command: a.budget_command.clone(),
        budget_proof: a.budget_proof.clone(),
        slot_command: a.slot_command.clone(),
        slot_proof: a.slot_proof.clone(),
    }
}
fn source(producer: &str, seq: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: "boot-1".into(),
        sequence: seq.to_string(),
    }
}
fn open(path: &Path, policy: Arc<Policy>) -> Store {
    try_open(path, policy).unwrap()
}
fn try_open(path: &Path, policy: Arc<Policy>) -> admiralty_store::Result<Store> {
    Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            reservations: Some(policy.clone()),
            inference_slots: Some(policy),
            ..Default::default()
        },
    )
}
fn contract(project: &str, id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new(project).unwrap(),
        root_id: RootId::new("root").unwrap(),
        task_id: TaskId::new(id).unwrap(),
        revision: 1,
        title: "Slot fixture".into(),
        objective: "Atomically claim bounded inference capacity".into(),
        non_goals: vec![],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "Commit atomically".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "true".into(),
            args: vec![],
            cwd: ".".into(),
            timeout_secs: 1,
        }],
        authority_ref: "fixture:slots".into(),
        max_attempts: 1,
        simulated: true,
    }
}
fn configure_budget(store: &Store) {
    let l = store.reservation_ledger(proof()).unwrap();
    let c = ReservationCommand {
        schema_version: 1,
        command_id: "limits".into(),
        source: source("budget", 1),
        expected_ledger_revision: l.revision,
        expected_ledger_digest: admission_reservations::ledger_digest(&l).unwrap(),
        launch_intent_digest: None,
        event: ReservationEvent::SetLimits {
            event_id: "limits".into(),
            account_id: AccountId::new("account").unwrap(),
            limits: ScopeLimits {
                unit: CurrencyUnit::new("USD", 2).unwrap(),
                account: Some(MinorUnits::new(10_000)),
                project: Some(MinorUnits::new(10_000)),
                root: Some(MinorUnits::new(10_000)),
            },
        },
    };
    store.record_reservation(c, proof()).unwrap();
}
fn set_pool(
    store: &Store,
    pool: &str,
    key: &str,
    generation: &str,
    capacity: u16,
    seq: u64,
) -> inference_slots::SlotResult {
    let ledger = store.inference_slot_ledger(slot_proof()).unwrap();
    store
        .record_inference_slot(
            SlotCommand {
                schema_version: 1,
                command_id: format!("set-{pool}-{generation}"),
                source: source("scheduler", seq),
                expected_ledger_revision: ledger.revision,
                expected_ledger_digest: inference_slots::ledger_digest(&ledger).unwrap(),
                event: SlotEvent::SetPool {
                    pool_id: pool.into(),
                    capacity_key: key.into(),
                    generation: generation.into(),
                    managed_capacity: capacity,
                },
            },
            slot_proof(),
        )
        .unwrap()
}
struct Admission {
    event: DomainEvent,
    intent: OutboxIntent,
    admission: admiralty_store::BudgetAndSlotAdmission,
}
fn prepare(
    store: &Store,
    project: &str,
    id: &str,
    seq: u64,
    pool: &str,
    generation: &str,
    slots: u16,
) -> Admission {
    let c = contract(project, id);
    let attempt = AttemptId::new(format!("attempt-{id}")).unwrap();
    let owner = PrincipalId::new("worker").unwrap();
    let scope = ReservationScope {
        project_id: c.project_id.clone(),
        root_id: c.root_id.clone(),
        task_id: c.task_id.clone(),
        attempt_id: attempt.clone(),
        owner: owner.clone(),
        incarnation: "boot-1".into(),
        contract_revision: 1,
        contract_digest: content_digest(&c).unwrap(),
        authority_ref: "fixture:slots".into(),
        resource_claims_digest: format!("sha256:{}", "a".repeat(64)),
        account_id: AccountId::new("account").unwrap(),
        unit: CurrencyUnit::new("USD", 2).unwrap(),
    };
    let event = DomainEvent::TaskClaimed {
        task_id: c.task_id,
        revision: 1,
        attempt_id: attempt,
        owner,
    };
    let intent = OutboxIntent {
        idempotency_key: format!("launch/{id}"),
        payload: json!({"task":id,"provider":"fixture"}),
        not_before: 0,
    };
    let intent_digest = outbox::intent_digest(&intent).unwrap();
    let reservations = store.reservation_ledger(proof()).unwrap();
    let budget_command = ReservationCommand {
        schema_version: 1,
        command_id: format!("reserve-{id}"),
        source: source("worker", seq),
        expected_ledger_revision: reservations.revision,
        expected_ledger_digest: admission_reservations::ledger_digest(&reservations).unwrap(),
        launch_intent_digest: Some(intent_digest.clone()),
        event: ReservationEvent::Reserve {
            event_id: format!("reserve-{id}"),
            reservation_id: format!("reservation-{id}"),
            scope: scope.clone(),
            correlation_id: format!("launch/{id}"),
            amount: MinorUnits::new(1),
        },
    };
    // Keep the exact digest that the slot authority binds to the submitted budget command.
    let budget_digest = admission_reservations::validate_command(&budget_command).unwrap();
    let slots_ledger = store.inference_slot_ledger(slot_proof()).unwrap();
    let slot_command = SlotCommand {
        schema_version: 1,
        command_id: format!("hold-{id}"),
        source: source("worker", seq),
        expected_ledger_revision: slots_ledger.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&slots_ledger).unwrap(),
        event: SlotEvent::Hold {
            hold_id: format!("hold-{id}"),
            claim: SlotClaim {
                scope,
                pool_id: pool.into(),
                generation: generation.into(),
                model_id: "model-a".into(),
                model_config_digest: format!("sha256:{}", "b".repeat(64)),
                inference_claims_digest: format!("sha256:{}", "c".repeat(64)),
                kernel_claims_digest: format!("sha256:{}", "a".repeat(64)),
                budget_reservation_id: format!("reservation-{id}"),
                budget_command_digest: budget_digest,
                launch_intent_digest: intent_digest,
                slots,
            },
        },
    };
    // Ensure the value remains owned by the atomic admission and is not mutated after digesting.
    Admission {
        event,
        intent,
        admission: admiralty_store::BudgetAndSlotAdmission {
            budget_command,
            budget_proof: proof(),
            slot_command,
            slot_proof: slot_proof(),
        },
    }
}
fn submit(store: &Store, project: &str, id: &str, seq: u64) {
    store
        .append(
            source("worker", seq),
            DomainEvent::TaskSubmitted {
                contract: contract(project, id),
            },
            "fixture-time".into(),
        )
        .unwrap();
}
fn counts(path: &Path) -> (i64, i64, i64, i64, i64, i64, i64) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.query_row("SELECT (SELECT COUNT(*) FROM domain_events),(SELECT COUNT(*) FROM outbox_intents),(SELECT COUNT(*) FROM admission_reservation_commands),(SELECT COUNT(*) FROM inference_slot_commands),(SELECT COUNT(*) FROM event_reservation_associations),(SELECT COUNT(*) FROM event_inference_slot_associations),(SELECT COUNT(*) FROM outbox_integrity_incidents)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap()
}
fn drop_slot_association_triggers(connection: &rusqlite::Connection) {
    let mut stmt=connection.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='event_inference_slot_associations'").unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(stmt);
    for name in names {
        connection
            .execute(
                &format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")),
                [],
            )
            .unwrap();
    }
}

#[test]
fn cross_project_race_cannot_take_the_last_slot_twice() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy);
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        1,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    submit(&store, "project-b", "task-b", 2);
    let a = prepare(&store, "project-a", "task-a", 3, "pool", "g1", 1);
    let b = prepare(&store, "project-b", "task-b", 4, "pool", "g1", 1);
    let (x, y) = (store.clone(), store.clone());
    let t1 = std::thread::spawn(move || {
        x.claim_with_budget_and_slot(
            source("worker", 3),
            a.event,
            "a".into(),
            a.intent,
            a.admission,
        )
    });
    let t2 = std::thread::spawn(move || {
        y.claim_with_budget_and_slot(
            source("worker", 4),
            b.event,
            "b".into(),
            b.intent,
            b.admission,
        )
    });
    let a_ok = t1.join().unwrap().is_ok();
    let b_ok = t2.join().unwrap().is_ok();
    assert_eq!([a_ok, b_ok].into_iter().filter(|v| *v).count(), 1);
    assert_eq!(
        store
            .inference_slot_ledger(slot_proof())
            .unwrap()
            .holds
            .len(),
        1
    );
    // Rebuild the losing request from both current journals. This proves the loser
    // is rejected by slot capacity, rather than only by the stale budget CAS.
    let (project, id, seq) = if a_ok {
        ("project-b", "task-b", 4)
    } else {
        ("project-a", "task-a", 3)
    };
    let retry = prepare(&store, project, id, seq, "pool", "g1", 1);
    let before_counts = counts(&path);
    let before_reservations = store.reservation_ledger(proof()).unwrap();
    let before_slots = store.inference_slot_ledger(slot_proof()).unwrap();
    let error = store
        .claim_with_budget_and_slot(
            source("worker", seq),
            retry.event,
            "capacity-retry".into(),
            retry.intent,
            retry.admission,
        )
        .expect_err("fresh retry must reach the slot capacity check");
    assert!(
        error.to_string().to_ascii_lowercase().contains("capacity"),
        "losing request must fail at slot capacity: {error}"
    );
    assert_eq!(
        counts(&path),
        before_counts,
        "capacity rejection cannot append, debit or queue"
    );
    assert_eq!(
        store.reservation_ledger(proof()).unwrap(),
        before_reservations
    );
    assert_eq!(
        store.inference_slot_ledger(slot_proof()).unwrap(),
        before_slots
    );
}

#[test]
fn exact_retry_keeps_original_receipts_after_later_commands_reopen_backup_and_rebuild() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        2,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    let prepared = prepare(&store, "project-a", "task-a", 2, "pool", "g1", 1);
    let retry_admission = clone_admission(&prepared.admission);
    let reopen_event = prepared.event.clone();
    let reopen_intent = prepared.intent.clone();
    let reopen_admission = clone_admission(&prepared.admission);
    let first = store
        .claim_with_budget_and_slot(
            source("worker", 2),
            prepared.event.clone(),
            "first".into(),
            prepared.intent.clone(),
            prepared.admission,
        )
        .unwrap();
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g2",
        2,
        2,
    );
    let retry = store
        .claim_with_budget_and_slot(
            source("worker", 2),
            prepared.event,
            "retry".into(),
            prepared.intent,
            retry_admission,
        )
        .unwrap();
    assert!(retry.claim.duplicate && retry.slot.replayed && retry.reservation.replayed);
    assert_eq!(retry.slot.receipt, first.slot.receipt);
    assert_eq!(retry.slot.receipt_digest, first.slot.receipt_digest);
    let after_retry = store.inference_slot_ledger(slot_proof()).unwrap();
    assert_eq!(after_retry.pools["pool"].generation, "g2");
    assert_eq!(
        after_retry.holds.len(),
        1,
        "generation rotation retains old occupancy"
    );
    let too_small = SlotCommand {
        schema_version: 1,
        command_id: "reduce-below-held".into(),
        source: source("scheduler", 3),
        expected_ledger_revision: after_retry.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&after_retry).unwrap(),
        event: SlotEvent::SetPool {
            pool_id: "pool".into(),
            capacity_key: format!("sha256:{}", "d".repeat(64)),
            generation: "g3".into(),
            managed_capacity: 0,
        },
    };
    assert!(
        store
            .record_inference_slot(too_small, slot_proof())
            .is_err()
    );
    store.rebuild().unwrap();
    store.backup(&backup).unwrap();
    drop(store);
    for db in [&path, &backup] {
        let reopened = open(db, policy.clone());
        assert_eq!(
            reopened
                .inference_slot_ledger(slot_proof())
                .unwrap()
                .holds
                .len(),
            1
        );
        let replay = reopened
            .claim_with_budget_and_slot(
                source("worker", 2),
                reopen_event.clone(),
                "reopen-retry".into(),
                reopen_intent.clone(),
                clone_admission(&reopen_admission),
            )
            .unwrap();
        assert!(replay.claim.duplicate && replay.slot.replayed && replay.reservation.replayed);
        assert_eq!(replay.slot.receipt, first.slot.receipt);
        assert_eq!(replay.slot.receipt_digest, first.slot.receipt_digest);
    }
}

#[test]
fn default_foreign_and_revoked_authorities_fail_closed() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let bare = Store::open(&path).unwrap();
    assert!(bare.inference_slot_ledger(slot_proof()).is_err());
    drop(bare);
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    let ledger = store.inference_slot_ledger(slot_proof()).unwrap();
    let cmd = SlotCommand {
        schema_version: 1,
        command_id: "pool".into(),
        source: source("scheduler", 1),
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&ledger).unwrap(),
        event: SlotEvent::SetPool {
            pool_id: "pool".into(),
            capacity_key: format!("sha256:{}", "d".repeat(64)),
            generation: "g1".into(),
            managed_capacity: 1,
        },
    };
    assert!(
        store
            .record_inference_slot(cmd.clone(), Arc::new(()))
            .is_err()
    );
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(store.record_inference_slot(cmd, slot_proof()).is_err());
    assert!(store.inference_slot_ledger(slot_proof()).is_err());
}

#[test]
fn ordinary_and_budget_only_claims_cannot_upgrade_or_downgrade_a_slot_claim() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        2,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    let p = prepare(&store, "project-a", "task-a", 2, "pool", "g1", 1);
    store
        .append_with_intent(
            source("worker", 2),
            p.event.clone(),
            "ordinary".into(),
            p.intent.clone(),
        )
        .unwrap();
    assert!(
        store
            .claim_with_budget_and_slot(
                source("worker", 2),
                p.event.clone(),
                "upgrade".into(),
                p.intent.clone(),
                p.admission
            )
            .is_err()
    );
    submit(&store, "project-a", "task-b", 3);
    let q = prepare(&store, "project-a", "task-b", 4, "pool", "g1", 1);
    let q_admission = clone_admission(&q.admission);
    store
        .claim_with_reservation(
            source("worker", 4),
            q.event.clone(),
            "budget-only".into(),
            q.intent.clone(),
            q.admission.budget_command.clone(),
            proof(),
        )
        .unwrap();
    assert!(
        store
            .claim_with_budget_and_slot(
                source("worker", 4),
                q.event,
                "budget-downgrade-attempt".into(),
                q.intent,
                q_admission
            )
            .is_err()
    );
    submit(&store, "project-a", "task-c", 5);
    let r = prepare(&store, "project-a", "task-c", 6, "pool", "g1", 1);
    let event = r.event.clone();
    let intent = r.intent.clone();
    let budget = r.admission.budget_command.clone();
    store
        .claim_with_budget_and_slot(
            source("worker", 6),
            r.event,
            "slotted".into(),
            r.intent,
            r.admission,
        )
        .unwrap();
    assert!(
        store
            .claim_with_reservation(
                source("worker", 6),
                event.clone(),
                "budget-downgrade".into(),
                intent.clone(),
                budget,
                proof()
            )
            .is_err()
    );
    assert!(
        store
            .append_with_intent(
                source("worker", 6),
                event,
                "ordinary-downgrade".into(),
                intent
            )
            .is_err()
    );
}

#[test]
fn capacity_key_generation_and_all_historical_holds_define_occupancy() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        2,
        1,
    );
    let l = store.inference_slot_ledger(slot_proof()).unwrap();
    let change_key = SlotCommand {
        schema_version: 1,
        command_id: "key-change".into(),
        source: source("scheduler", 2),
        expected_ledger_revision: l.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&l).unwrap(),
        event: SlotEvent::SetPool {
            pool_id: "pool".into(),
            capacity_key: format!("sha256:{}", "e".repeat(64)),
            generation: "g2".into(),
            managed_capacity: 2,
        },
    };
    assert!(
        store
            .record_inference_slot(change_key, slot_proof())
            .is_err()
    );
    let alias = SlotCommand {
        schema_version: 1,
        command_id: "alias".into(),
        source: source("scheduler", 2),
        expected_ledger_revision: l.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&l).unwrap(),
        event: SlotEvent::SetPool {
            pool_id: "alias".into(),
            capacity_key: format!("sha256:{}", "d".repeat(64)),
            generation: "g1".into(),
            managed_capacity: 2,
        },
    };
    assert!(store.record_inference_slot(alias, slot_proof()).is_err());
}

#[test]
fn malformed_scope_and_missing_or_stale_pool_are_denied() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure_budget(&store);
    submit(&store, "project-a", "task-a", 1);
    let p = prepare(&store, "project-a", "task-a", 2, "unknown", "g1", 1);
    assert!(
        store
            .claim_with_budget_and_slot(
                source("worker", 2),
                p.event,
                "unknown-pool".into(),
                p.intent,
                p.admission
            )
            .is_err()
    );
}

#[test]
fn current_budget_and_slot_bindings_reject_scope_source_budget_digest_kernel_and_intent_mismatches()
{
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        8,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    let prepared = prepare(&store, "project-a", "task-a", 2, "pool", "g1", 1);
    let original = clone_admission(&prepared.admission);
    let mut bads = Vec::new();
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.scope.project_id = ProjectId::new("project-b").unwrap();
    }
    bads.push(("scope", bad));
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.scope.incarnation = "other-boot".into();
    }
    bads.push(("scope incarnation", bad));
    let mut bad = clone_admission(&original);
    bad.slot_command.source = source("other-worker", 2);
    bads.push(("source", bad));
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.budget_reservation_id = "different-reservation".into();
    }
    bads.push(("budget reservation id", bad));
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.budget_command_digest = format!("sha256:{}", "e".repeat(64));
    }
    bads.push(("budget command digest", bad));
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.kernel_claims_digest = format!("sha256:{}", "f".repeat(64));
    }
    bads.push(("kernel resource digest", bad));
    let mut bad = clone_admission(&original);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.launch_intent_digest = format!("sha256:{}", "e".repeat(64));
    }
    bads.push(("launch intent digest", bad));
    let before = counts(&path);
    for (label, admission) in bads {
        assert!(
            store
                .claim_with_budget_and_slot(
                    source("worker", 2),
                    prepared.event.clone(),
                    format!("bad-{label}"),
                    prepared.intent.clone(),
                    admission
                )
                .is_err(),
            "accepted mismatched {label}"
        );
        assert_eq!(counts(&path), before, "mismatched {label} partially wrote");
    }
}

#[test]
fn final_authority_denial_and_late_sql_failure_leave_no_partial_admission() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        3,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    submit(&store, "project-a", "task-b", 2);
    let a = prepare(&store, "project-a", "task-a", 3, "pool", "g1", 1);
    store
        .claim_with_budget_and_slot(
            source("worker", 3),
            a.event,
            "first-authorized".into(),
            a.intent,
            a.admission,
        )
        .unwrap();
    let b = prepare(&store, "project-a", "task-b", 4, "pool", "g1", 1);
    let before_counts = counts(&path);
    let before_budget = store.reservation_ledger(proof()).unwrap();
    let before_slots = store.inference_slot_ledger(slot_proof()).unwrap();
    policy.staged_claim_seen.store(false, Ordering::SeqCst);
    policy.deny_staged_claim.store(true, Ordering::SeqCst);
    let denied = store.claim_with_budget_and_slot(
        source("worker", 4),
        b.event.clone(),
        "final-denial".into(),
        b.intent.clone(),
        clone_admission(&b.admission),
    );
    assert!(
        denied.is_err(),
        "denial on the staged final authority callback must reject"
    );
    assert!(
        policy.staged_claim_seen.load(Ordering::SeqCst),
        "policy must observe and deny the actual staged Claimed task at final reauthorization"
    );
    assert_eq!(counts(&path), before_counts);
    assert_eq!(store.reservation_ledger(proof()).unwrap(), before_budget);
    assert_eq!(
        store.inference_slot_ledger(slot_proof()).unwrap(),
        before_slots
    );
    policy.deny_staged_claim.store(false, Ordering::SeqCst);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER inject_slot_association_failure BEFORE INSERT ON event_inference_slot_associations BEGIN SELECT RAISE(ABORT,'owned slot fixture failure'); END;").unwrap();
    let failed = store.claim_with_budget_and_slot(
        source("worker", 4),
        b.event.clone(),
        "sql-failure".into(),
        b.intent.clone(),
        clone_admission(&b.admission),
    );
    assert!(failed.is_err());
    assert_eq!(counts(&path), before_counts);
    assert_eq!(store.reservation_ledger(proof()).unwrap(), before_budget);
    assert_eq!(
        store.inference_slot_ledger(slot_proof()).unwrap(),
        before_slots
    );
    c.execute_batch("DROP TRIGGER inject_slot_association_failure")
        .unwrap();
    store
        .claim_with_budget_and_slot(
            source("worker", 4),
            b.event,
            "sql-retry".into(),
            b.intent,
            b.admission,
        )
        .unwrap();
}

#[test]
fn request_stop_and_expiry_do_not_release_slot_occupancy() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        2,
        1,
    );
    submit(&store, "project-a", "task-a", 1);
    let p = prepare(&store, "project-a", "task-a", 2, "pool", "g1", 1);
    store
        .claim_with_budget_and_slot(
            source("worker", 2),
            p.event,
            "held".into(),
            p.intent,
            p.admission,
        )
        .unwrap();
    let ledger = store.reservation_ledger(proof()).unwrap();
    let reservation = &ledger.reservations["reservation-task-a"];
    let stop = ReservationCommand {
        schema_version: 1,
        command_id: "expiry-stop".into(),
        source: source("budget", 2),
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: admission_reservations::ledger_digest(&ledger).unwrap(),
        launch_intent_digest: None,
        event: ReservationEvent::RequestStop {
            event_id: "expiry-stop".into(),
            reservation_id: "reservation-task-a".into(),
            binding: reservation.binding(),
            reason: StopReason::LeaseExpiry,
        },
    };
    store.record_reservation(stop, proof()).unwrap();
    assert!(
        store.reservation_ledger(proof()).unwrap().reservations["reservation-task-a"].outstanding
    );
    let first = store
        .claim_outbox_key(
            Some(100),
            "fixture-dispatcher".into(),
            5,
            "launch/task-a".into(),
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .claim_outbox_key(
                Some(105),
                "fixture-dispatcher".into(),
                5,
                "launch/task-a".into()
            )
            .unwrap()
            .is_none(),
        "expired dispatch must become ambiguous, not retryable"
    );
    assert!(
        store
            .complete_outbox(first.token, "late-receipt".into(), Some(106))
            .is_err()
    );
    assert_eq!(
        store
            .inference_slot_ledger(slot_proof())
            .unwrap()
            .holds
            .len(),
        1
    );
}

#[test]
fn outbox_collision_and_corrupt_reverse_associations_fail_closed() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure_budget(&store);
    set_pool(
        &store,
        "pool",
        &format!("sha256:{}", "d".repeat(64)),
        "g1",
        2,
        1,
    );
    let conflict = OutboxIntent {
        idempotency_key: "launch/task-a".into(),
        payload: json!({"different":true}),
        not_before: 0,
    };
    store
        .append_with_intent(
            source("worker", 1),
            DomainEvent::TaskSubmitted {
                contract: contract("project-a", "task-a"),
            },
            "conflict".into(),
            conflict,
        )
        .unwrap();
    let p = prepare(&store, "project-a", "task-a", 2, "pool", "g1", 1);
    let before = counts(&path);
    assert!(
        store
            .claim_with_budget_and_slot(
                source("worker", 2),
                p.event,
                "outbox-collision".into(),
                p.intent,
                p.admission
            )
            .is_err()
    );
    let after_collision = counts(&path);
    assert_eq!(
        (
            after_collision.0,
            after_collision.1,
            after_collision.2,
            after_collision.3,
            after_collision.4,
            after_collision.5,
        ),
        (before.0, before.1, before.2, before.3, before.4, before.5),
        "collision cannot append, queue, debit, or hold"
    );
    assert_eq!(
        after_collision.6,
        before.6 + 1,
        "preserve the collision incident"
    );
    submit(&store, "project-a", "task-b", 2);
    let q = prepare(&store, "project-a", "task-b", 3, "pool", "g1", 1);
    store
        .claim_with_budget_and_slot(
            source("worker", 3),
            q.event,
            "association".into(),
            q.intent,
            q.admission,
        )
        .unwrap();
    store.backup(&backup).unwrap();
    // Repoint an event's durable association to the pool configuration command.
    // The row remains locally well formed, but its command no longer binds this event.
    let connection = rusqlite::Connection::open(&path).unwrap();
    drop_slot_association_triggers(&connection);
    connection.execute("UPDATE event_inference_slot_associations SET command_id='set-pool-g1', command_digest=(SELECT command_digest FROM inference_slot_commands WHERE command_id='set-pool-g1'), receipt_digest=(SELECT receipt_digest FROM inference_slot_commands WHERE command_id='set-pool-g1') WHERE event_seq=(SELECT MAX(event_seq) FROM event_inference_slot_associations)",[]).unwrap();
    drop(connection);
    assert!(
        store.rebuild().is_err(),
        "rebuild accepted a cross-event slot association"
    );
    assert!(
        store
            .backup(&dir.path().join("must-not-backup-corruption.sqlite"))
            .is_err(),
        "backup accepted cross-event association corruption"
    );
    drop(store);
    assert!(
        try_open(&path, policy.clone()).is_err(),
        "open accepted cross-event slot association corruption"
    );
    assert_eq!(
        try_open(&backup, policy.clone())
            .unwrap()
            .inference_slot_ledger(slot_proof())
            .unwrap()
            .holds
            .len(),
        1,
        "the backup taken before corruption remains valid with the successful hold"
    );
}
