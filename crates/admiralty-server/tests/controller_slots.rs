//! Controller admission through the Store's atomic budget and inference-slot writer.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, PrincipalId, ProjectId, RootId,
    SchedulingState, SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationScope, ScopeLimits,
};
use admiralty_server::services::controller::{
    self, CurrentAdmission, LaunchRequest, admit_one, admit_one_reserved, admit_one_slotted,
};
use admiralty_store::{
    Store, StoreAuthorities,
    admission_reservations::{self, ReservationAuthority, ReservationBinding, ReservationCommand},
    inference_slots::{
        self, InferenceSlotAuthority, SlotBinding, SlotClaim, SlotCommand, SlotEvent,
    },
    outbox,
};
use std::{any::Any, fs, path::Path, sync::Arc};
use tempfile::{Builder, TempDir};

struct Authority;
impl ReservationAuthority for Authority {
    fn authorize_write(&self, _: &ReservationBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Authority>()
    }
    fn authorize_read(
        &self,
        _: &admiralty_runtime::reservations::ReservationLedger,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        proof.is::<Authority>()
    }
}
impl InferenceSlotAuthority for Authority {
    fn authorize_write(&self, _: &SlotBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Authority>()
    }
    fn authorize_read(
        &self,
        _: &inference_slots::SlotLedger,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        proof.is::<Authority>()
    }
}
fn proof() -> Arc<Authority> {
    Arc::new(Authority)
}

struct Permit;
impl CurrentAdmission for Permit {
    fn authorize_current(
        &self,
        task: &admiralty_core::TaskView,
        request: &LaunchRequest,
    ) -> Result<(), String> {
        if task.contract.task_id == request.task_id {
            Ok(())
        } else {
            Err("scope mismatch".into())
        }
    }
}
struct Deny;
impl CurrentAdmission for Deny {
    fn authorize_current(
        &self,
        _: &admiralty_core::TaskView,
        _: &LaunchRequest,
    ) -> Result<(), String> {
        Err("revoked current admission".into())
    }
}

fn private_dir() -> TempDir {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::PermissionsExt;
        let mut builder = Builder::new();
        builder
            .prefix("controller-slots-")
            .permissions(fs::Permissions::from_mode(0o700));
        builder
    };
    #[cfg(not(unix))]
    let builder = {
        let mut builder = Builder::new();
        builder.prefix("controller-slots-");
        builder
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
fn contract(id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("controller-project").unwrap(),
        root_id: RootId::new("controller-root").unwrap(),
        task_id: TaskId::new(id).unwrap(),
        revision: 1,
        title: "Slotted controller fixture".into(),
        objective: "Atomically claim, debit and hold managed inference capacity".into(),
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
        authority_ref: "grant:controller-slots".into(),
        max_attempts: 1,
        simulated: true,
    }
}
fn source(producer: &str, sequence: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: "controller-test-incarnation".into(),
        sequence: sequence.to_string(),
    }
}
fn request(id: &str, sequence: u64) -> LaunchRequest {
    let c = contract(id);
    LaunchRequest {
        source: source("controller-test", sequence),
        task_id: c.task_id.clone(),
        expected_contract_digest: content_digest(&c).unwrap(),
        attempt_id: AttemptId::new(format!("attempt-{id}")).unwrap(),
        owner: PrincipalId::new("controller-owner").unwrap(),
        run_id: format!("run-{id}"),
        context_digest: format!("sha256:{}", "a".repeat(64)),
        policy_digest: format!("sha256:{}", "b".repeat(64)),
        provider: "codex_cli".into(),
        model: "model-a".into(),
        observed_utc: "2026-10-19T10:00:00Z".into(),
        not_before: 10,
    }
}
fn open(path: &Path) -> Store {
    let auth = Arc::new(Authority);
    Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            reservations: Some(auth.clone()),
            inference_slots: Some(auth),
            ..Default::default()
        },
    )
    .unwrap()
}
fn submit(store: &Store, id: &str, seq: u64) {
    store
        .append(
            source("controller-test", seq),
            DomainEvent::TaskSubmitted {
                contract: contract(id),
            },
            "fixture-time".into(),
        )
        .unwrap();
}
fn configure(store: &Store, capacity: u16) {
    let p = proof();
    let budget = store.reservation_ledger(p.clone()).unwrap();
    store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: "limits".into(),
                source: source("budget", 1),
                expected_ledger_revision: budget.revision,
                expected_ledger_digest: admission_reservations::ledger_digest(&budget).unwrap(),
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
            },
            p,
        )
        .unwrap();
    let slots = store.inference_slot_ledger(proof()).unwrap();
    store
        .record_inference_slot(
            SlotCommand {
                schema_version: 1,
                command_id: "pool-g1".into(),
                source: source("scheduler", 1),
                expected_ledger_revision: slots.revision,
                expected_ledger_digest: inference_slots::ledger_digest(&slots).unwrap(),
                event: SlotEvent::SetPool {
                    pool_id: "pool".into(),
                    capacity_key: format!("sha256:{}", "d".repeat(64)),
                    generation: "g1".into(),
                    managed_capacity: capacity,
                },
            },
            proof(),
        )
        .unwrap();
}
fn admission(
    store: &Store,
    req: &LaunchRequest,
    pool: &str,
    sequence: u64,
) -> admiralty_store::BudgetAndSlotAdmission {
    let task = store.snapshot().unwrap().projection.tasks[&req.task_id].clone();
    let intent = controller::planned_launch_intent(&task, req).unwrap();
    let intent_digest = outbox::intent_digest(&intent).unwrap();
    let budget = store.reservation_ledger(proof()).unwrap();
    let scope = ReservationScope {
        project_id: task.contract.project_id.clone(),
        root_id: task.contract.root_id.clone(),
        task_id: req.task_id.clone(),
        attempt_id: req.attempt_id.clone(),
        owner: req.owner.clone(),
        incarnation: req.source.incarnation.clone(),
        contract_revision: task.contract.revision,
        contract_digest: req.expected_contract_digest.clone(),
        authority_ref: task.contract.authority_ref.clone(),
        resource_claims_digest: req.policy_digest.clone(),
        account_id: AccountId::new("account").unwrap(),
        unit: CurrencyUnit::new("USD", 2).unwrap(),
    };
    let budget_command = ReservationCommand {
        schema_version: 1,
        command_id: format!("reserve-{}", req.task_id),
        source: req.source.clone(),
        expected_ledger_revision: budget.revision,
        expected_ledger_digest: admission_reservations::ledger_digest(&budget).unwrap(),
        launch_intent_digest: Some(intent_digest.clone()),
        event: ReservationEvent::Reserve {
            event_id: format!("reserve-{}", req.task_id),
            reservation_id: format!("reservation-{}", req.task_id),
            scope: scope.clone(),
            correlation_id: req.run_id.clone(),
            amount: MinorUnits::new(1),
        },
    };
    let budget_digest = admission_reservations::validate_command(&budget_command).unwrap();
    let slots = store.inference_slot_ledger(proof()).unwrap();
    let slot_command = SlotCommand {
        schema_version: 1,
        command_id: format!("hold-{}", req.task_id),
        source: req.source.clone(),
        expected_ledger_revision: slots.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&slots).unwrap(),
        event: SlotEvent::Hold {
            hold_id: format!("hold-{}", req.task_id),
            claim: SlotClaim {
                scope,
                pool_id: pool.into(),
                generation: "g1".into(),
                model_id: req.model.clone(),
                model_config_digest: format!("sha256:{}", "e".repeat(64)),
                inference_claims_digest: format!("sha256:{}", "f".repeat(64)),
                kernel_claims_digest: req.policy_digest.clone(),
                budget_reservation_id: format!("reservation-{}", req.task_id),
                budget_command_digest: budget_digest,
                launch_intent_digest: intent_digest,
                slots: 1,
            },
        },
    };
    let _ = sequence;
    admiralty_store::BudgetAndSlotAdmission {
        budget_command,
        budget_proof: proof(),
        slot_command,
        slot_proof: proof(),
    }
}
fn clone_admission(
    a: &admiralty_store::BudgetAndSlotAdmission,
) -> admiralty_store::BudgetAndSlotAdmission {
    admiralty_store::BudgetAndSlotAdmission {
        budget_command: a.budget_command.clone(),
        budget_proof: proof(),
        slot_command: a.slot_command.clone(),
        slot_proof: proof(),
    }
}
fn durable_counts(store: &Store) -> (usize, u64, usize, u64, usize) {
    let events = store.events_after(0, 8192).unwrap();
    let budget = store.reservation_ledger(proof()).unwrap();
    let slots = store.inference_slot_ledger(proof()).unwrap();
    let claimed = store
        .snapshot()
        .unwrap()
        .projection
        .tasks
        .values()
        .filter(|task| task.state == SchedulingState::Claimed)
        .count();
    (
        events.len(),
        budget.revision,
        budget.reservations.len(),
        slots.revision,
        slots.holds.len() + claimed,
    )
}

#[test]
fn slotted_controller_commits_claim_debit_hold_and_outbox_once_and_replays_original_receipts() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path);
    configure(&store, 2);
    submit(&store, "task-a", 1);
    let req = request("task-a", 2);
    let admission = admission(&store, &req, "pool", 2);
    let retry_admission = clone_admission(&admission);
    let first = admit_one_slotted(&store, &Permit, &req, admission).unwrap();
    assert!(!first.intent.duplicate && !first.reservation.replayed && !first.slot.replayed);
    let before = durable_counts(&store);
    assert_eq!(before, (2, 2, 1, 2, 2));
    let budget_ledger = store.reservation_ledger(proof()).unwrap();
    assert_eq!(
        budget_ledger.reservations["reservation-task-a"].outstanding,
        true
    );
    assert_eq!(store.inference_slot_ledger(proof()).unwrap().holds.len(), 1);
    let later = store.reservation_ledger(proof()).unwrap();
    store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: "later-limits".into(),
                source: source("budget", 2),
                expected_ledger_revision: later.revision,
                expected_ledger_digest: admission_reservations::ledger_digest(&later).unwrap(),
                launch_intent_digest: None,
                event: ReservationEvent::SetLimits {
                    event_id: "later-limits".into(),
                    account_id: AccountId::new("account").unwrap(),
                    limits: ScopeLimits {
                        unit: CurrencyUnit::new("USD", 2).unwrap(),
                        account: Some(MinorUnits::new(9_000)),
                        project: Some(MinorUnits::new(9_000)),
                        root: Some(MinorUnits::new(9_000)),
                    },
                },
            },
            proof(),
        )
        .unwrap();
    let retry = admit_one_slotted(&store, &Permit, &req, retry_admission).unwrap();
    assert!(retry.intent.duplicate && retry.reservation.replayed && retry.slot.replayed);
    assert_eq!(retry.reservation.receipt, first.reservation.receipt);
    assert_eq!(retry.slot.receipt, first.slot.receipt);
    assert_eq!(durable_counts(&store), (2, 3, 1, 2, 2));
    drop(store);
    let reopened = open(&path);
    assert_eq!(
        reopened.inference_slot_ledger(proof()).unwrap().holds.len(),
        1
    );
    assert_eq!(
        reopened
            .reservation_ledger(proof())
            .unwrap()
            .reservations
            .len(),
        1
    );
}

#[test]
fn current_admission_and_slot_capacity_denials_have_no_partial_effects() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path);
    configure(&store, 1);
    submit(&store, "task-a", 1);
    submit(&store, "task-b", 2);
    let a = request("task-a", 3);
    let b = request("task-b", 4);
    admit_one_slotted(&store, &Permit, &a, admission(&store, &a, "pool", 3)).unwrap();
    let before = durable_counts(&store);
    let budgets = store.reservation_ledger(proof()).unwrap();
    let slots = store.inference_slot_ledger(proof()).unwrap();
    assert!(admit_one_slotted(&store, &Deny, &b, admission(&store, &b, "pool", 4)).is_err());
    assert_eq!(durable_counts(&store), before);
    assert_eq!(
        admit_one_slotted(&store, &Permit, &b, admission(&store, &b, "pool", 4))
            .unwrap_err()
            .to_string()
            .contains("capacity"),
        true
    );
    assert_eq!(durable_counts(&store), before);
    assert_eq!(store.reservation_ledger(proof()).unwrap(), budgets);
    assert_eq!(store.inference_slot_ledger(proof()).unwrap(), slots);
}

#[test]
fn unknown_pool_scope_and_intent_mismatches_are_denied_without_effects() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path);
    configure(&store, 3);
    submit(&store, "task-a", 1);
    let req = request("task-a", 2);
    let mut bad = admission(&store, &req, "missing-pool", 2);
    let before = durable_counts(&store);
    assert!(admit_one_slotted(&store, &Permit, &req, bad).is_err());
    assert_eq!(durable_counts(&store), before);
    bad = admission(&store, &req, "pool", 2);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.scope.project_id = ProjectId::new("other-project").unwrap();
    }
    assert!(admit_one_slotted(&store, &Permit, &req, bad).is_err());
    assert_eq!(durable_counts(&store), before);
    bad = admission(&store, &req, "pool", 2);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.launch_intent_digest = format!("sha256:{}", "0".repeat(64));
    }
    assert!(admit_one_slotted(&store, &Permit, &req, bad).is_err());
    assert_eq!(durable_counts(&store), before);
    bad = admission(&store, &req, "pool", 2);
    if let SlotEvent::Hold { claim, .. } = &mut bad.slot_command.event {
        claim.model_id = "another-model".into();
    }
    assert!(admit_one_slotted(&store, &Permit, &req, bad).is_err());
    assert_eq!(durable_counts(&store), before);
}

#[test]
fn ordinary_and_budget_only_controller_paths_cannot_downgrade_a_slotted_claim() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path);
    configure(&store, 2);
    submit(&store, "task-a", 1);
    let req = request("task-a", 2);
    let admission = admission(&store, &req, "pool", 2);
    let reservation = admission.budget_command.clone();
    let budget_proof = proof();
    let slots = clone_admission(&admission);
    admit_one_slotted(&store, &Permit, &req, admission).unwrap();
    let before = durable_counts(&store);
    assert!(admit_one(&store, &Permit, &req).is_err());
    assert!(admit_one_reserved(&store, &Permit, &req, reservation, budget_proof).is_err());
    assert_eq!(durable_counts(&store), before);
    assert_eq!(store.inference_slot_ledger(proof()).unwrap().holds.len(), 1);
    assert_eq!(
        store
            .reservation_ledger(proof())
            .unwrap()
            .reservations
            .len(),
        1
    );
    drop(slots);
}

#[test]
fn last_slot_race_admits_only_one_new_controller_claim() {
    let dir = private_dir();
    let path = dir.path().join("store.sqlite");
    let store = open(&path);
    configure(&store, 1);
    submit(&store, "task-a", 1);
    submit(&store, "task-b", 2);
    let a = request("task-a", 3);
    let b = request("task-b", 4);
    let aa = admission(&store, &a, "pool", 3);
    let bb = admission(&store, &b, "pool", 4);
    let (sa, sb) = (store.clone(), store.clone());
    let ta = std::thread::spawn(move || admit_one_slotted(&sa, &Permit, &a, aa).is_ok());
    let tb = std::thread::spawn(move || admit_one_slotted(&sb, &Permit, &b, bb).is_ok());
    let successes = [ta.join().unwrap(), tb.join().unwrap()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    assert_eq!(store.inference_slot_ledger(proof()).unwrap().holds.len(), 1);
    assert_eq!(
        store
            .snapshot()
            .unwrap()
            .projection
            .tasks
            .values()
            .filter(|task| task.state == SchedulingState::Claimed)
            .count(),
        1
    );
}
