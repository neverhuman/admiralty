//! Store-backed tests for the paired admission commit fence.
//!
//! These tests are a child of `admission_fence`, so they can seed the private
//! installed test catalog. They still exercise the real Store writer and the
//! real paired budget, slot, and fence adapters.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::{
    Store, StoreAuthorities, admission_reservations,
    admission_reservations::{ReservationAuthority, ReservationBinding, ReservationCommand},
    inference_slots,
    inference_slots::{InferenceSlotAuthority, SlotBinding, SlotClaim, SlotCommand, SlotEvent},
    outbox::{self, OutboxIntent},
};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, DomainEvent, PrincipalId, ProjectId, RootId,
    SourceIdentity, TaskContract, TaskId, content_digest,
};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationScope, ScopeLimits,
};
use serde_json::json;
use std::{
    any::Any,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};

const BARRIER_TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    event: DomainEvent,
    intent: OutboxIntent,
    budget: ReservationCommand,
    slot: SlotCommand,
    source: SourceIdentity,
    observed_utc: String,
}

fn source(producer: &str, sequence: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: "fence-test-boot".into(),
        sequence: sequence.to_string(),
    }
}

fn contract() -> TaskContract {
    contract_for("task-fence")
}

fn contract_for(task: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("project-fence").unwrap(),
        root_id: RootId::new("root-fence").unwrap(),
        task_id: TaskId::new(task).unwrap(),
        revision: 1,
        title: "Admission fence fixture".into(),
        objective: "Exercise paired policy through the Store writer".into(),
        non_goals: vec![],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "Commit paired records atomically".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "true".into(),
            args: vec![],
            cwd: ".".into(),
            timeout_secs: 1,
        }],
        authority_ref: "fixture:admission-fence".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn fixture(store: &Store) -> Fixture {
    let contract = contract();
    let attempt = AttemptId::new("attempt-fence").unwrap();
    let owner = PrincipalId::new("fence-worker").unwrap();
    let source = source("fence-worker", 2);
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: attempt.clone(),
        owner: owner.clone(),
        incarnation: source.incarnation.clone(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).unwrap(),
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: format!("sha256:{}", "a".repeat(64)),
        account_id: AccountId::new("fence-account").unwrap(),
        unit: CurrencyUnit::new("USD", 2).unwrap(),
    };
    let event = DomainEvent::TaskClaimed {
        task_id: contract.task_id,
        revision: 1,
        attempt_id: attempt,
        owner,
    };
    let intent = OutboxIntent {
        idempotency_key: "launch/task-fence".into(),
        payload: json!({"task":"task-fence","provider":"fixture"}),
        not_before: 0,
    };
    let intent_digest = outbox::intent_digest(&intent).unwrap();
    let reservations = store.reservation_ledger(Arc::new(ReadCredential)).unwrap();
    let mut budget = ReservationCommand {
        schema_version: 1,
        command_id: "reserve-task-fence".into(),
        source: source.clone(),
        expected_ledger_revision: reservations.revision,
        expected_ledger_digest: admission_reservations::ledger_digest(&reservations).unwrap(),
        launch_intent_digest: Some(intent_digest.clone()),
        event: ReservationEvent::Reserve {
            event_id: "reserve-task-fence".into(),
            reservation_id: "reservation-task-fence".into(),
            scope: scope.clone(),
            correlation_id: intent.idempotency_key.clone(),
            amount: MinorUnits::new(1),
        },
    };
    let budget_digest = admission_reservations::validate_command(&budget).unwrap();
    let slots = store
        .inference_slot_ledger(Arc::new(ReadCredential))
        .unwrap();
    let slot = SlotCommand {
        schema_version: 1,
        command_id: "hold-task-fence".into(),
        source: source.clone(),
        expected_ledger_revision: slots.revision,
        expected_ledger_digest: inference_slots::ledger_digest(&slots).unwrap(),
        event: SlotEvent::Hold {
            hold_id: "hold-task-fence".into(),
            claim: SlotClaim {
                scope,
                pool_id: "fence-pool".into(),
                generation: "generation-1".into(),
                model_id: "model-fence".into(),
                model_config_digest: format!("sha256:{}", "b".repeat(64)),
                inference_claims_digest: format!("sha256:{}", "c".repeat(64)),
                kernel_claims_digest: format!("sha256:{}", "a".repeat(64)),
                budget_reservation_id: "reservation-task-fence".into(),
                budget_command_digest: budget_digest,
                launch_intent_digest: intent_digest,
                slots: 1,
            },
        },
    };
    // Keep this mutable binding point so negative tests can vary one field
    // before asking the private catalog to install exact rules.
    budget.source = source.clone();
    Fixture {
        event,
        intent,
        budget,
        slot,
        source,
        observed_utc: "fence-test-time".into(),
    }
}

struct ReadCredential;
impl ReservationAuthority for ReadCredential {
    fn authorize_write(&self, _: &ReservationBinding<'_>, _: &(dyn Any + Send + Sync)) -> bool {
        true
    }
    fn authorize_read(
        &self,
        _: &admiralty_runtime::reservations::ReservationLedger,
        _: &(dyn Any + Send + Sync),
    ) -> bool {
        true
    }
}
impl InferenceSlotAuthority for ReadCredential {
    fn authorize_write(&self, _: &SlotBinding<'_>, _: &(dyn Any + Send + Sync)) -> bool {
        true
    }
    fn authorize_read(&self, _: &inference_slots::SlotLedger, _: &(dyn Any + Send + Sync)) -> bool {
        true
    }
}

fn seed(path: &Path) -> Fixture {
    let seed_policy = Arc::new(ReadCredential);
    let store = Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            reservations: Some(seed_policy.clone()),
            inference_slots: Some(seed_policy),
            ..Default::default()
        },
    )
    .unwrap();
    let reservations = store.reservation_ledger(Arc::new(ReadCredential)).unwrap();
    store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: "fence-test-limits".into(),
                source: source("budget-seed", 1),
                expected_ledger_revision: reservations.revision,
                expected_ledger_digest: admission_reservations::ledger_digest(&reservations)
                    .unwrap(),
                launch_intent_digest: None,
                event: ReservationEvent::SetLimits {
                    event_id: "fence-test-limits".into(),
                    account_id: AccountId::new("fence-account").unwrap(),
                    limits: ScopeLimits {
                        unit: CurrencyUnit::new("USD", 2).unwrap(),
                        account: Some(MinorUnits::new(100)),
                        project: Some(MinorUnits::new(100)),
                        root: Some(MinorUnits::new(100)),
                    },
                },
            },
            Arc::new(ReadCredential),
        )
        .unwrap();
    let slots = store
        .inference_slot_ledger(Arc::new(ReadCredential))
        .unwrap();
    store
        .record_inference_slot(
            SlotCommand {
                schema_version: 1,
                command_id: "fence-test-pool".into(),
                source: source("scheduler-seed", 1),
                expected_ledger_revision: slots.revision,
                expected_ledger_digest: inference_slots::ledger_digest(&slots).unwrap(),
                event: SlotEvent::SetPool {
                    pool_id: "fence-pool".into(),
                    capacity_key: format!("sha256:{}", "d".repeat(64)),
                    generation: "generation-1".into(),
                    managed_capacity: 2,
                },
            },
            Arc::new(ReadCredential),
        )
        .unwrap();
    store
        .append(
            source("fence-worker", 1),
            DomainEvent::TaskSubmitted {
                contract: contract(),
            },
            "fence-seed-time".into(),
        )
        .unwrap();
    let f = fixture(&store);
    drop(store);
    f
}

fn admission(
    fixture: &Fixture,
    proofs: &(
        admission_reservations::ReservationProof,
        inference_slots::SlotProof,
    ),
) -> crate::BudgetAndSlotAdmission {
    crate::BudgetAndSlotAdmission {
        budget_command: fixture.budget.clone(),
        budget_proof: proofs.0.clone(),
        slot_command: fixture.slot.clone(),
        slot_proof: proofs.1.clone(),
    }
}

fn binding<'a>(
    fixture: &'a Fixture,
    event_digest: &'a str,
    effect: crate::LifecycleEffect<'a>,
) -> AdmissionFenceBinding<'a> {
    AdmissionFenceBinding {
        source: &fixture.source,
        source_seq: 2,
        event: &fixture.event,
        event_digest,
        observed_utc: &fixture.observed_utc,
        effect: Some(effect),
        budget: &fixture.budget,
        slot: &fixture.slot,
    }
}

struct Pause {
    entered: SyncSender<()>,
    release: Mutex<Receiver<()>>,
    once: AtomicBool,
    deny_after_release: AtomicBool,
}

fn pause_pair() -> (Arc<Pause>, Receiver<()>, SyncSender<()>) {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    (
        Arc::new(Pause {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            once: AtomicBool::new(true),
            deny_after_release: AtomicBool::new(false),
        }),
        entered_rx,
        release_tx,
    )
}

struct PausingBudget {
    inner: Arc<dyn ReservationAuthority>,
    pause: Arc<Pause>,
}

impl ReservationAuthority for PausingBudget {
    fn authorize_write(
        &self,
        binding: &ReservationBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        if self.pause.once.swap(false, Ordering::SeqCst) {
            self.pause
                .entered
                .send(())
                .expect("test receiver remains open");
            self.pause
                .release
                .lock()
                .expect("pause mutex is not poisoned")
                .recv_timeout(BARRIER_TIMEOUT)
                .expect("bounded release arrives");
        }
        self.inner.authorize_write(binding, proof)
            && !self.pause.deny_after_release.load(Ordering::SeqCst)
    }

    fn authorize_read(
        &self,
        ledger: &admiralty_runtime::reservations::ReservationLedger,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        self.inner.authorize_read(ledger, proof)
    }
}

fn db_counts(path: &Path) -> (i64, i64, i64, i64, i64, i64, i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.query_row(
        "SELECT (SELECT COUNT(*) FROM domain_events), (SELECT COUNT(*) FROM outbox_intents), (SELECT COUNT(*) FROM admission_reservation_commands), (SELECT COUNT(*) FROM inference_slot_commands), (SELECT COUNT(*) FROM event_reservation_associations), (SELECT COUNT(*) FROM event_inference_slot_associations), (SELECT COUNT(*) FROM outbox_integrity_incidents)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
    ).unwrap()
}

#[test]
fn actual_store_holds_paired_cohort_lease_through_commit_and_old_proof_retry_denies() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fenced.sqlite");
    let fixture = seed(&path);
    let event_digest = crate::digest(&serde_json::to_string(&fixture.event).unwrap());
    let payload = serde_json::to_string(&fixture.intent.payload).unwrap();
    let payload_digest = crate::digest(&payload);
    let intent_digest = outbox::intent_digest(&fixture.intent).unwrap();
    let effect = crate::LifecycleEffect {
        idempotency_key: &fixture.intent.idempotency_key,
        payload: &payload,
        payload_digest: &payload_digest,
        intent_digest: &intent_digest,
        not_before: fixture.intent.not_before.try_into().unwrap(),
    };
    let binding = binding(&fixture, &event_digest, effect);
    let catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    let cohort = PairedAdmissionCohort::from_installed(catalog).unwrap();
    let proofs = cohort.proofs_for(&binding).unwrap();
    let foreign_catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    let foreign_cohort = PairedAdmissionCohort::from_installed(foreign_catalog).unwrap();
    let foreign_proofs = foreign_cohort.proofs_for(&binding).unwrap();
    let adapters = cohort.adapters();
    let epoch = cohort.epoch().unwrap();
    let (pause, entered, release) = pause_pair();
    let authorities = StoreAuthorities {
        reservations: Some(Arc::new(PausingBudget {
            inner: adapters.budget.clone(),
            pause,
        })),
        inference_slots: Some(adapters.slots.clone()),
        admission_fence: Some(adapters.fence.clone()),
        ..Default::default()
    };
    let store = Store::open_with_startup_authorities(&path, authorities).unwrap();
    let before = db_counts(&path);

    let standalone_budget = store.record_reservation(fixture.budget.clone(), proofs.0.clone());
    assert!(matches!(
        standalone_budget,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    let standalone_slot = store.record_inference_slot(fixture.slot.clone(), proofs.1.clone());
    assert!(standalone_slot.is_err(), "standalone Hold must be rejected");
    let budget_only = store.claim_with_reservation(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
        fixture.budget.clone(),
        proofs.0.clone(),
    );
    assert!(matches!(
        budget_only,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    let ordinary = store.append_with_intent(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
    );
    assert!(matches!(
        ordinary,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    assert_eq!(db_counts(&path), before);

    let mixed_proofs = (proofs.0.clone(), foreign_proofs.1.clone());
    let mixed = store.claim_with_budget_and_slot(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
        admission(&fixture, &mixed_proofs),
    );
    assert!(matches!(
        mixed,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    let changed_binding = store.claim_with_budget_and_slot(
        fixture.source.clone(),
        fixture.event.clone(),
        "changed-observation".into(),
        fixture.intent.clone(),
        admission(&fixture, &proofs),
    );
    assert!(matches!(
        changed_binding,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    assert_eq!(db_counts(&path), before);

    let request = admission(&fixture, &proofs);
    let writer_store = store.clone();
    let writer_fixture = Fixture {
        event: fixture.event.clone(),
        intent: fixture.intent.clone(),
        budget: fixture.budget.clone(),
        slot: fixture.slot.clone(),
        source: fixture.source.clone(),
        observed_utc: fixture.observed_utc.clone(),
    };
    let writer = std::thread::spawn(move || {
        writer_store.claim_with_budget_and_slot(
            writer_fixture.source,
            writer_fixture.event,
            writer_fixture.observed_utc,
            writer_fixture.intent,
            request,
        )
    });
    entered
        .recv_timeout(BARRIER_TIMEOUT)
        .expect("writer reaches actual paired budget policy");
    assert_eq!(cohort.try_revoke(epoch), Err(FenceError::Busy));
    release.send(()).expect("writer remains paused");
    let first = writer.join().unwrap().unwrap();
    assert!(!first.claim.duplicate && !first.reservation.replayed && !first.slot.replayed);
    assert_eq!(
        db_counts(&path),
        (
            before.0 + 1,
            before.1 + 1,
            before.2 + 1,
            before.3 + 1,
            before.4 + 1,
            before.5 + 1,
            before.6
        )
    );

    assert_eq!(cohort.try_revoke(epoch).unwrap(), epoch + 1);
    let retry = store.claim_with_budget_and_slot(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
        admission(&fixture, &proofs),
    );
    assert!(matches!(
        retry,
        Err(crate::StoreError::AdmissionFence(FenceError::Denied))
    ));
    assert_eq!(
        db_counts(&path),
        (
            before.0 + 1,
            before.1 + 1,
            before.2 + 1,
            before.3 + 1,
            before.4 + 1,
            before.5 + 1,
            before.6
        )
    );

    let rotated_catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    assert_eq!(
        cohort.try_rotate(epoch + 1, rotated_catalog).unwrap(),
        epoch + 2
    );
    let fresh_proofs = cohort.proofs_for(&binding).unwrap();
    let replay = store
        .claim_with_budget_and_slot(
            fixture.source.clone(),
            fixture.event.clone(),
            fixture.observed_utc.clone(),
            fixture.intent.clone(),
            admission(&fixture, &fresh_proofs),
        )
        .unwrap();
    assert!(replay.claim.duplicate && replay.reservation.replayed && replay.slot.replayed);
    assert_eq!(replay.claim.event_seq, first.claim.event_seq);
    assert_eq!(
        replay.claim.source_contiguous_seq,
        first.claim.source_contiguous_seq
    );
    assert_eq!(replay.reservation.receipt, first.reservation.receipt);
    assert_eq!(replay.slot.receipt, first.slot.receipt);
    assert_eq!(
        db_counts(&path),
        (
            before.0 + 1,
            before.1 + 1,
            before.2 + 1,
            before.3 + 1,
            before.4 + 1,
            before.5 + 1,
            before.6
        )
    );
}

#[test]
fn actual_store_denial_after_acquire_releases_cohort_lease_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("denied.sqlite");
    let fixture = seed(&path);
    let event_digest = crate::digest(&serde_json::to_string(&fixture.event).unwrap());
    let payload = serde_json::to_string(&fixture.intent.payload).unwrap();
    let payload_digest = crate::digest(&payload);
    let intent_digest = outbox::intent_digest(&fixture.intent).unwrap();
    let effect = crate::LifecycleEffect {
        idempotency_key: &fixture.intent.idempotency_key,
        payload: &payload,
        payload_digest: &payload_digest,
        intent_digest: &intent_digest,
        not_before: fixture.intent.not_before.try_into().unwrap(),
    };
    let binding = binding(&fixture, &event_digest, effect);
    let catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    let cohort = PairedAdmissionCohort::from_installed(catalog).unwrap();
    let proofs = cohort.proofs_for(&binding).unwrap();
    let adapters = cohort.adapters();
    let epoch = cohort.epoch().unwrap();
    let (pause, entered, release) = pause_pair();
    pause.deny_after_release.store(true, Ordering::SeqCst);
    let store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            reservations: Some(Arc::new(PausingBudget {
                inner: adapters.budget.clone(),
                pause,
            })),
            inference_slots: Some(adapters.slots.clone()),
            admission_fence: Some(adapters.fence.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let before = db_counts(&path);
    let request = admission(&fixture, &proofs);
    let writer_store = store.clone();
    let writer_fixture = Fixture {
        event: fixture.event.clone(),
        intent: fixture.intent.clone(),
        budget: fixture.budget.clone(),
        slot: fixture.slot.clone(),
        source: fixture.source.clone(),
        observed_utc: fixture.observed_utc.clone(),
    };
    let writer = std::thread::spawn(move || {
        writer_store.claim_with_budget_and_slot(
            writer_fixture.source,
            writer_fixture.event,
            writer_fixture.observed_utc,
            writer_fixture.intent,
            request,
        )
    });
    entered
        .recv_timeout(BARRIER_TIMEOUT)
        .expect("writer reaches paired authority after fence acquisition");
    assert_eq!(cohort.try_revoke(epoch), Err(FenceError::Busy));
    release.send(()).expect("writer remains paused");
    assert!(matches!(
        writer.join().unwrap(),
        Err(crate::StoreError::Reservation(_))
    ));
    assert_eq!(db_counts(&path), before);
    assert_eq!(cohort.try_revoke(epoch).unwrap(), epoch + 1);
}

#[test]
fn stale_budget_cas_rolls_back_all_admission_records_and_releases_the_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stale-budget.sqlite");
    let fixture = seed(&path);
    let advance_policy = Arc::new(ReadCredential);
    let advance_store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            reservations: Some(advance_policy.clone()),
            inference_slots: Some(advance_policy),
            ..Default::default()
        },
    )
    .unwrap();
    let ledger = advance_store
        .reservation_ledger(Arc::new(ReadCredential))
        .unwrap();
    advance_store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: "advance-budget-after-proof-preparation".into(),
                source: source("budget-advance", 1),
                expected_ledger_revision: ledger.revision,
                expected_ledger_digest: admission_reservations::ledger_digest(&ledger).unwrap(),
                launch_intent_digest: None,
                event: ReservationEvent::SetLimits {
                    event_id: "advance-budget-after-proof-preparation".into(),
                    account_id: AccountId::new("fence-account").unwrap(),
                    limits: ScopeLimits {
                        unit: CurrencyUnit::new("USD", 2).unwrap(),
                        account: Some(MinorUnits::new(100)),
                        project: Some(MinorUnits::new(100)),
                        root: Some(MinorUnits::new(100)),
                    },
                },
            },
            Arc::new(ReadCredential),
        )
        .unwrap();
    drop(advance_store);

    let event_digest = crate::digest(&serde_json::to_string(&fixture.event).unwrap());
    let payload = serde_json::to_string(&fixture.intent.payload).unwrap();
    let payload_digest = crate::digest(&payload);
    let intent_digest = outbox::intent_digest(&fixture.intent).unwrap();
    let effect = crate::LifecycleEffect {
        idempotency_key: &fixture.intent.idempotency_key,
        payload: &payload,
        payload_digest: &payload_digest,
        intent_digest: &intent_digest,
        not_before: fixture.intent.not_before.try_into().unwrap(),
    };
    let binding = binding(&fixture, &event_digest, effect);
    let catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    let cohort = PairedAdmissionCohort::from_installed(catalog).unwrap();
    let proofs = cohort.proofs_for(&binding).unwrap();
    let adapters = cohort.adapters();
    let epoch = cohort.epoch().unwrap();
    let store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            reservations: Some(adapters.budget.clone()),
            inference_slots: Some(adapters.slots.clone()),
            admission_fence: Some(adapters.fence.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let before = db_counts(&path);
    let result = store.claim_with_budget_and_slot(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
        admission(&fixture, &proofs),
    );
    assert!(matches!(result, Err(crate::StoreError::Reservation(_))));
    assert_eq!(db_counts(&path), before);
    assert_eq!(cohort.try_revoke(epoch).unwrap(), epoch + 1);
}

#[test]
fn outbox_collision_incident_commit_keeps_admission_effects_absent_and_releases_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox-incident.sqlite");
    let fixture = seed(&path);
    let seed_policy = Arc::new(ReadCredential);
    let seed_store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            reservations: Some(seed_policy.clone()),
            inference_slots: Some(seed_policy),
            ..Default::default()
        },
    )
    .unwrap();
    let mut conflicting_intent = fixture.intent.clone();
    conflicting_intent.payload = json!({"task":"different-payload"});
    seed_store
        .append_with_intent(
            source("outbox-conflict-seed", 1),
            DomainEvent::TaskSubmitted {
                contract: contract_for("task-outbox-conflict"),
            },
            "outbox-conflict-seed-time".into(),
            conflicting_intent,
        )
        .unwrap();
    drop(seed_store);
    let before = db_counts(&path);

    let event_digest = crate::digest(&serde_json::to_string(&fixture.event).unwrap());
    let payload = serde_json::to_string(&fixture.intent.payload).unwrap();
    let payload_digest = crate::digest(&payload);
    let intent_digest = outbox::intent_digest(&fixture.intent).unwrap();
    let effect = crate::LifecycleEffect {
        idempotency_key: &fixture.intent.idempotency_key,
        payload: &payload,
        payload_digest: &payload_digest,
        intent_digest: &intent_digest,
        not_before: fixture.intent.not_before.try_into().unwrap(),
    };
    let binding = binding(&fixture, &event_digest, effect);
    let catalog = catalog_for_tests(std::slice::from_ref(&binding), false).unwrap();
    let cohort = PairedAdmissionCohort::from_installed(catalog).unwrap();
    let proofs = cohort.proofs_for(&binding).unwrap();
    let adapters = cohort.adapters();
    let epoch = cohort.epoch().unwrap();
    let store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            reservations: Some(adapters.budget.clone()),
            inference_slots: Some(adapters.slots.clone()),
            admission_fence: Some(adapters.fence.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let result = store.claim_with_budget_and_slot(
        fixture.source.clone(),
        fixture.event.clone(),
        fixture.observed_utc.clone(),
        fixture.intent.clone(),
        admission(&fixture, &proofs),
    );
    assert!(matches!(
        result,
        Err(crate::StoreError::Outbox(
            outbox::OutboxError::IdentityCollision
        ))
    ));
    let after = db_counts(&path);
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, before.1);
    assert_eq!(after.2, before.2);
    assert_eq!(after.3, before.3);
    assert_eq!(after.4, before.4);
    assert_eq!(after.5, before.5);
    assert_eq!(after.6, before.6 + 1);
    assert_eq!(cohort.try_revoke(epoch).unwrap(), epoch + 1);
}
