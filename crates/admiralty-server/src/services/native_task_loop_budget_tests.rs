//! Component tests for the task loop's protected budget admission path.
//! ResourceGate stays unconfigured; these tests qualify Store transactions and
//! retained loop state only, never Linux enforcement or a real provider.

use super::{
    controller::{self, CurrentAdmission},
    tests::{fixture_with_authority, TestFixture},
    AdmissionStatus, DispatchAdmission, DispatchClock, DispatchIntent, HoldReason,
    NativeBudgetAdmission, TaskDispatchStatus,
};
use admiralty_core::TaskView;
use admiralty_runtime::reservations::{
    MinorUnits, ReservationEvent, ReservationLedger, ScopeLimits, StopReason,
};
use admiralty_store::{
    admission_reservations::{
        ledger_digest, ReservationAuthority, ReservationBinding, ReservationCommand,
        ReservationProof,
    },
    outbox,
};
use std::{
    any::Any,
    fs,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

struct BudgetProof;
struct BudgetReadProof;

#[derive(Default)]
struct BudgetPolicy {
    revoked: AtomicBool,
    reads: AtomicUsize,
    writes: AtomicUsize,
}

impl ReservationAuthority for BudgetPolicy {
    fn authorize_write(&self, _: &ReservationBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        self.writes.fetch_add(1, Ordering::SeqCst);
        proof.is::<BudgetProof>() && !self.revoked.load(Ordering::SeqCst)
    }

    fn authorize_read(&self, _: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        self.reads.fetch_add(1, Ordering::SeqCst);
        proof.is::<BudgetReadProof>() && !self.revoked.load(Ordering::SeqCst)
    }
}

struct Permit;

impl CurrentAdmission for Permit {
    fn authorize_current(&self, _: &TaskView, _: &super::LaunchRequest) -> Result<(), String> {
        Ok(())
    }
}

impl DispatchAdmission for Permit {
    fn authorize_dispatch(&self, _: &TaskView, _: &DispatchIntent) -> Result<(), String> {
        Ok(())
    }
}

struct DenyDispatch;

impl DispatchAdmission for DenyDispatch {
    fn authorize_dispatch(&self, _: &TaskView, _: &DispatchIntent) -> Result<(), String> {
        Err("dispatch policy revoked".into())
    }
}

struct FixedClock;

impl DispatchClock for FixedClock {
    fn now_unix_seconds(&self) -> Option<u64> {
        Some(10)
    }
}

struct StopOnSecondClock<'a> {
    calls: AtomicUsize,
    store: &'a admiralty_store::Store,
}

impl StopOnSecondClock<'_> {
    fn stop_reservation(&self) {
        let ledger = self
            .store
            .reservation_ledger(read_proof())
            .expect("read reservation at second clock sample");
        let reservation = ledger.reservations["native-reservation"].clone();
        let command_id = "native-budget-stop-at-clock";
        self.store
            .record_reservation(
                ReservationCommand {
                    schema_version: 1,
                    command_id: command_id.into(),
                    source: admiralty_core::SourceIdentity {
                        producer_id: "budget-test-clock-stop".into(),
                        incarnation: reservation.scope.incarnation.clone(),
                        sequence: "1".into(),
                    },
                    expected_ledger_revision: ledger.revision,
                    expected_ledger_digest: ledger_digest(&ledger)
                        .expect("ledger digest at second clock sample"),
                    launch_intent_digest: None,
                    event: ReservationEvent::RequestStop {
                        event_id: command_id.into(),
                        reservation_id: reservation.id.clone(),
                        binding: reservation.binding(),
                        reason: StopReason::Cancellation,
                    },
                },
                proof(),
            )
            .expect("second clock sample records an actual stop command");
    }
}

impl DispatchClock for StopOnSecondClock<'_> {
    fn now_unix_seconds(&self) -> Option<u64> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            // The first exact budget replay has passed; mutate the actual ledger
            // at the controller's second clock seam before runtime invocation.
            self.stop_reservation();
        }
        Some(10)
    }
}

fn proof() -> ReservationProof {
    Arc::new(BudgetProof)
}

fn read_proof() -> ReservationProof {
    Arc::new(BudgetReadProof)
}

fn budget_fixture() -> (TestFixture, Arc<BudgetPolicy>) {
    let policy = Arc::new(BudgetPolicy::default());
    let mut fixture = fixture_with_authority(Some(policy.clone() as Arc<dyn ReservationAuthority>));
    // The prepared plan's reservation scope was issued for boot-1. The claim
    // source must carry that same incarnation for Store's task binding.
    fixture.request.source.incarnation = fixture.plan.reservation.scope.incarnation.clone();

    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("read initial reservation ledger");
    let limits = ReservationCommand {
        schema_version: 1,
        command_id: "budget-test-limits".into(),
        source: admiralty_core::SourceIdentity {
            producer_id: "budget-test-policy".into(),
            incarnation: "limits-boot".into(),
            sequence: "1".into(),
        },
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: ledger_digest(&ledger).expect("initial ledger digest"),
        launch_intent_digest: None,
        event: ReservationEvent::SetLimits {
            event_id: "budget-test-limits".into(),
            account_id: fixture.plan.reservation.scope.account_id.clone(),
            limits: ScopeLimits {
                unit: fixture.plan.reservation.scope.unit.clone(),
                account: Some(MinorUnits::new(1000)),
                project: Some(MinorUnits::new(1000)),
                root: Some(MinorUnits::new(1000)),
            },
        },
    };
    fixture
        .store
        .record_reservation(limits, proof())
        .expect("seed actual account/project/root budget limits");
    (fixture, policy)
}

fn budget_for(fixture: &TestFixture) -> NativeBudgetAdmission {
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("read budget ledger");
    let task = fixture
        .store
        .snapshot()
        .expect("read task for launch intent")
        .projection
        .tasks[&fixture.request.task_id]
        .clone();
    let intent = controller::planned_launch_intent(&task, &fixture.request)
        .expect("construct exact launch intent");
    let reservation = &fixture.plan.reservation;
    let command_id = "native-budget-reserve";
    NativeBudgetAdmission {
        command: ReservationCommand {
            schema_version: 1,
            command_id: command_id.into(),
            source: fixture.request.source.clone(),
            expected_ledger_revision: ledger.revision,
            expected_ledger_digest: ledger_digest(&ledger).expect("budget ledger digest"),
            launch_intent_digest: Some(
                outbox::intent_digest(&intent).expect("launch intent digest"),
            ),
            event: ReservationEvent::Reserve {
                event_id: command_id.into(),
                reservation_id: reservation.id.clone(),
                scope: reservation.scope.clone(),
                correlation_id: reservation.correlation_id.clone(),
                amount: reservation.original.clone(),
            },
        },
        proof: proof(),
        read_proof: read_proof(),
    }
}

fn cleanup_parts(
    root: std::path::PathBuf,
    task_loop: super::NativeTaskLoop,
    store: admiralty_store::Store,
) {
    drop(task_loop);
    drop(store);
    fs::remove_dir_all(root).expect("remove owned budget fixture");
}

#[test]
fn protected_admission_commits_one_debit_claim_and_exact_outbox_effect() {
    let (mut fixture, policy) = budget_fixture();
    let request = fixture.request.clone();
    let key = format!("launch/{}/{}", request.task_id, request.attempt_id);
    let scope = fixture.plan.reservation.scope.clone();
    let budget = budget_for(&fixture);
    let first = fixture
        .task_loop
        .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
        .expect("registered budget admission remains durable");
    let AdmissionStatus::Queued(first) = first else {
        panic!("expected one durable budget claim")
    };
    assert!(!first.duplicate);

    policy.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("queued retry rechecks current writer authority"),
        AdmissionStatus::Held(HoldReason::AdmissionUncertain)
    ));
    policy.revoked.store(false, Ordering::SeqCst);
    let replay = fixture
        .task_loop
        .retry_admission(&request.run_id, &Permit)
        .expect("restored authority replays exact reservation and claim");
    assert!(matches!(
        replay,
        AdmissionStatus::Queued(ref intent) if intent.duplicate
    ));

    let snapshot = fixture.store.snapshot().expect("one durable claim");
    assert_eq!(snapshot.cursor.event_seq, "2");
    assert_eq!(
        snapshot.projection.tasks[&request.task_id].state,
        admiralty_core::SchedulingState::Claimed
    );
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("one authorized debit");
    assert_eq!(ledger.revision, 2, "limits plus one reserve command");
    assert_eq!(ledger.reservations.len(), 1);
    let reserved = &ledger.reservations["native-reservation"];
    assert_eq!(reserved.scope, scope);
    assert_eq!(reserved.original, MinorUnits::new(1));
    assert_eq!(reserved.settled, MinorUnits::new(0));
    assert!(reserved.outstanding);
    assert!(!reserved.stop_requested);
    let account_id = reserved.scope.account_id.clone();
    let unit = reserved.scope.unit.clone();
    drop(ledger);

    // A later unrelated ledger command changes the global CAS revision while
    // leaving this reservation identical and therefore still admissible.
    let before_limits = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("read before unrelated limits update");
    let limits_id = "budget-test-unrelated-limits";
    fixture
        .store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: limits_id.into(),
                source: admiralty_core::SourceIdentity {
                    producer_id: "budget-test-unrelated".into(),
                    incarnation: "limits-boot".into(),
                    sequence: "1".into(),
                },
                expected_ledger_revision: before_limits.revision,
                expected_ledger_digest: ledger_digest(&before_limits)
                    .expect("ledger digest before unrelated update"),
                launch_intent_digest: None,
                event: ReservationEvent::SetLimits {
                    event_id: limits_id.into(),
                    account_id,
                    limits: ScopeLimits {
                        unit,
                        account: Some(MinorUnits::new(1000)),
                        project: Some(MinorUnits::new(1000)),
                        root: Some(MinorUnits::new(1000)),
                    },
                },
            },
            proof(),
        )
        .expect("unrelated limits update advances only ledger revision");
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("exact reservation survives unrelated ledger updates"),
        AdmissionStatus::Queued(ref intent) if intent.duplicate
    ));
    let after_limits = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("unrelated update retains budget entry");
    assert_eq!(after_limits.revision, 3);
    assert_eq!(after_limits.reservations["native-reservation"].revision, 2);

    let claim = fixture
        .store
        .claim_outbox_key(Some(10), "budget-test-dispatcher".into(), 30, key.clone())
        .expect("inspect exact durable effect")
        .expect("one atomic outbox intent");
    assert_eq!(claim.idempotency_key, key);
    assert_eq!(claim.payload["run_id"], request.run_id);
    assert!(fixture
        .store
        .claim_outbox_key(
            Some(11),
            "budget-test-second".into(),
            30,
            claim.idempotency_key
        )
        .expect("same effect is not duplicated")
        .is_none());
    fixture
        .store
        .fail_outbox_unknown(claim.token, "test leaves effect unresolved".into())
        .expect("unknown effect remains fenced");
    assert!(fixture
        .store
        .claim_outbox_key(
            Some(50),
            "budget-test-no-retry".into(),
            30,
            first.outbox_key
        )
        .expect("unknown effect is not made runnable")
        .is_none());
    let after_unknown = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("unknown effect retains liability");
    assert!(after_unknown.reservations["native-reservation"].outstanding);
    assert!(after_unknown
        .unresolved_liabilities
        .values()
        .any(|amount| { amount == &MinorUnits::new(1) }));
    assert!(policy.reads.load(Ordering::SeqCst) >= 3);
    assert!(policy.writes.load(Ordering::SeqCst) >= 2);
    cleanup_parts(fixture.temp, fixture.task_loop, fixture.store);
}

#[test]
fn failed_writer_admission_retains_registered_plan_for_authorized_retry() {
    let (mut fixture, policy) = budget_fixture();
    let request = fixture.request.clone();
    let temp = fixture.temp.clone();
    let budget = budget_for(&fixture);
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
            .expect("post-registration writer denial is retained"),
        AdmissionStatus::Held(HoldReason::AdmissionUncertain)
    ));
    assert_eq!(fixture.task_loop.runs.len(), 1);
    assert_eq!(
        fixture.store.snapshot().expect("no claim").cursor.event_seq,
        "1"
    );
    assert!(fixture.store.reservation_ledger(read_proof()).is_err());

    policy.revoked.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("retained actual plan can be retried"),
        AdmissionStatus::Queued(ref intent) if !intent.duplicate
    ));
    assert_eq!(
        fixture
            .store
            .snapshot()
            .expect("one claim")
            .cursor
            .event_seq,
        "2"
    );
    cleanup_parts(temp, fixture.task_loop, fixture.store);
}

#[test]
fn wrong_budget_read_proof_rejects_preflight_and_returns_original_plan_and_custody() {
    let (mut fixture, _) = budget_fixture();
    let request = fixture.request.clone();
    let expected_head = fixture.plan.prepared_workspace.head_oid().to_owned();
    let mut budget = budget_for(&fixture);
    budget.read_proof = Arc::new(String::from("foreign-read-capability"));
    let original_command = budget.command.clone();
    let original_write = Arc::clone(&budget.proof);
    let original_read = Arc::clone(&budget.read_proof);

    let rejected = fixture
        .task_loop
        .admit_reserved(request, fixture.plan, &Permit, budget)
        .expect_err("foreign read capability is checked before plan registration");
    assert_eq!(rejected.reason, super::LoopError::StoreUnavailable);
    assert_eq!(rejected.plan.prepared_workspace.head_oid(), expected_head);
    let returned = rejected
        .budget
        .expect("original budget custody is returned");
    assert_eq!(returned.command, original_command);
    assert!(Arc::ptr_eq(&returned.proof, &original_write));
    assert!(Arc::ptr_eq(&returned.read_proof, &original_read));
    assert!(fixture.task_loop.runs.is_empty());
    assert_eq!(
        fixture
            .store
            .snapshot()
            .expect("no durable claim")
            .cursor
            .event_seq,
        "1"
    );
    assert!(fixture
        .store
        .reservation_ledger(read_proof())
        .expect("authorized read can inspect unchanged ledger")
        .reservations
        .is_empty());
    cleanup_parts(fixture.temp, fixture.task_loop, fixture.store);
}

#[test]
fn mismatched_reservation_and_command_return_actual_preparation_without_effects() {
    for mismatch in [
        "amount",
        "scope",
        "correlation",
        "source",
        "incarnation",
        "effect",
        "command",
    ] {
        let (mut fixture, _) = budget_fixture();
        let mut request = fixture.request.clone();
        let original_head = fixture.plan.prepared_workspace.head_oid().to_owned();
        let mut budget = budget_for(&fixture);
        match (&mut budget.command.event, mismatch) {
            (ReservationEvent::Reserve { amount, .. }, "amount") => *amount = MinorUnits::new(2),
            (ReservationEvent::Reserve { scope, .. }, "scope") => {
                scope.root_id = admiralty_core::RootId::new("foreign-root").expect("root ID")
            }
            (ReservationEvent::Reserve { correlation_id, .. }, "correlation") => {
                *correlation_id = "foreign-run".into()
            }
            (_, "source") => budget.command.source.sequence = "2".into(),
            (_, "incarnation") => {
                // Keep the request and command equal while changing both away
                // from the actual resource reservation's boot incarnation.
                request.source.incarnation = "foreign-boot".into();
                budget.command.source = request.source.clone();
            }
            (_, "effect") => {
                budget.command.launch_intent_digest = Some(format!("sha256:{}", "f".repeat(64)))
            }
            (_, "command") => budget.command.command_id = "other-command".into(),
            _ => unreachable!("mismatch case is exhaustive"),
        }
        let rejected = fixture
            .task_loop
            .admit_reserved(request, fixture.plan, &Permit, budget)
            .expect_err("mismatched protected admission fails before registration");
        assert_eq!(
            rejected.reason,
            super::LoopError::InvalidBinding,
            "{mismatch}"
        );
        assert_eq!(
            rejected.plan.prepared_workspace.head_oid(),
            original_head,
            "original preparation survives {mismatch}"
        );
        assert!(fixture.task_loop.runs.is_empty(), "{mismatch}");
        assert_eq!(
            fixture
                .store
                .snapshot()
                .expect("unchanged Store")
                .cursor
                .event_seq,
            "1",
            "{mismatch}"
        );
        assert!(
            fixture
                .store
                .reservation_ledger(read_proof())
                .expect("limits remain readable")
                .reservations
                .is_empty(),
            "{mismatch}"
        );
        drop(rejected);
        cleanup_parts(fixture.temp, fixture.task_loop, fixture.store);
    }
}

#[test]
fn request_stop_invalidates_retry_and_dispatch_while_liability_stays_outstanding() {
    let (mut fixture, _) = budget_fixture();
    let request = fixture.request.clone();
    let temp = fixture.temp.clone();
    let budget = budget_for(&fixture);
    let initial = fixture
        .task_loop
        .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
        .expect("budget claim");
    assert!(matches!(initial, AdmissionStatus::Queued(_)));

    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("read current reservation");
    let reservation = ledger.reservations["native-reservation"].clone();
    let stop_id = "native-budget-stop";
    fixture
        .store
        .record_reservation(
            ReservationCommand {
                schema_version: 1,
                command_id: stop_id.into(),
                source: admiralty_core::SourceIdentity {
                    producer_id: "budget-test-stop".into(),
                    incarnation: request.source.incarnation.clone(),
                    sequence: "1".into(),
                },
                expected_ledger_revision: ledger.revision,
                expected_ledger_digest: ledger_digest(&ledger).expect("current ledger digest"),
                launch_intent_digest: None,
                event: ReservationEvent::RequestStop {
                    event_id: stop_id.into(),
                    reservation_id: reservation.id.clone(),
                    binding: reservation.binding(),
                    reason: StopReason::Cancellation,
                },
            },
            proof(),
        )
        .expect("actual Store CAS records cancellation");

    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("current reservation no longer matches frozen plan"),
        AdmissionStatus::Held(HoldReason::AdmissionUncertain)
    ));
    assert!(matches!(
        fixture
            .task_loop
            .dispatch(&request.run_id, &FixedClock, &Permit)
            .expect("stopped reservation blocks runtime"),
        TaskDispatchStatus::Held {
            runtime_attempted: false,
            ..
        }
    ));
    assert!(!fixture.marker.exists(), "no child started");
    let stopped = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("stopped liability remains visible");
    let stopped = &stopped.reservations["native-reservation"];
    assert!(stopped.stop_requested);
    assert!(stopped.outstanding);
    assert_eq!(stopped.original, MinorUnits::new(1));
    assert_eq!(stopped.settled, MinorUnits::new(0));
    cleanup_parts(temp, fixture.task_loop, fixture.store);
}

#[test]
fn dispatch_denial_marks_effect_unknown_without_runtime_or_admission_retry() {
    let (mut fixture, _) = budget_fixture();
    let request = fixture.request.clone();
    let temp = fixture.temp.clone();
    let budget = budget_for(&fixture);
    assert!(matches!(
        fixture
            .task_loop
            .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
            .expect("budget admission"),
        AdmissionStatus::Queued(_)
    ));

    assert!(matches!(
        fixture
            .task_loop
            .dispatch(&request.run_id, &FixedClock, &DenyDispatch)
            .expect("revoked dispatch is durably unknown"),
        TaskDispatchStatus::Held {
            runtime_attempted: false,
            ..
        }
    ));
    assert!(!fixture.marker.exists(), "dispatch denial starts no child");
    assert!(fixture
        .store
        .claim_outbox_key(
            Some(50),
            "budget-test-after-denial".into(),
            30,
            format!("launch/{}/{}", request.task_id, request.attempt_id),
        )
        .expect("unknown outbox effect remains fenced")
        .is_none());
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("unknown dispatch must not requeue"),
        AdmissionStatus::Held(_)
    ));
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("uncertain dispatch retains budget liability");
    assert!(ledger.reservations["native-reservation"].outstanding);
    assert_eq!(ledger.reservations.len(), 1);
    assert_eq!(
        fixture
            .store
            .snapshot()
            .expect("claimed task remains held")
            .projection
            .tasks[&request.task_id]
            .state,
        admiralty_core::SchedulingState::Claimed
    );
    cleanup_parts(temp, fixture.task_loop, fixture.store);
}

#[test]
fn stop_at_second_dispatch_clock_sample_fences_runtime_and_preserves_liability() {
    let (mut fixture, _) = budget_fixture();
    let request = fixture.request.clone();
    let temp = fixture.temp.clone();
    let budget = budget_for(&fixture);
    assert!(matches!(
        fixture
            .task_loop
            .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
            .expect("budget admission"),
        AdmissionStatus::Queued(_)
    ));

    let store_for_clock = fixture.store.clone();
    let clock = StopOnSecondClock {
        calls: AtomicUsize::new(0),
        store: &store_for_clock,
    };
    assert!(matches!(
        fixture
            .task_loop
            .dispatch(&request.run_id, &clock, &Permit)
            .expect("mid-dispatch reservation stop becomes unknown"),
        TaskDispatchStatus::Held {
            runtime_attempted: false,
            ..
        }
    ));
    assert_eq!(
        clock.calls.load(Ordering::SeqCst),
        3,
        "stop occurs at the second sample; the controller also samples after the refused runtime call"
    );
    assert!(!fixture.marker.exists(), "no child starts after the stop");
    assert!(fixture
        .store
        .claim_outbox_key(
            Some(50),
            "budget-test-no-runtime-retry".into(),
            30,
            format!("launch/{}/{}", request.task_id, request.attempt_id),
        )
        .expect("ambiguous outbox stays fenced")
        .is_none());
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("unknown dispatch cannot requeue"),
        AdmissionStatus::Held(_)
    ));
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("read stopped reservation liability");
    let reservation = &ledger.reservations["native-reservation"];
    assert!(reservation.stop_requested);
    assert!(reservation.outstanding);
    assert_eq!(reservation.settled, MinorUnits::new(0));
    cleanup_parts(temp, fixture.task_loop, fixture.store);
}

#[test]
fn revoked_budget_authority_blocks_dispatch_replay_and_cannot_reset_unknown_effect() {
    let (mut fixture, policy) = budget_fixture();
    let request = fixture.request.clone();
    let budget = budget_for(&fixture);
    assert!(matches!(
        fixture
            .task_loop
            .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
            .expect("budget admission before revocation"),
        AdmissionStatus::Queued(_)
    ));
    let writes_before = policy.writes.load(Ordering::SeqCst);
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .dispatch(&request.run_id, &FixedClock, &Permit)
            .expect("current budget writer authority is checked during dispatch replay"),
        TaskDispatchStatus::Held {
            runtime_attempted: false,
            ..
        }
    ));
    assert!(policy.writes.load(Ordering::SeqCst) > writes_before);
    assert!(!fixture.marker.exists(), "revoked budget starts no child");

    policy.revoked.store(false, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("restoring authority cannot turn unknown dispatch back into queued"),
        AdmissionStatus::Held(HoldReason::DispatchUncertain)
    ));
    assert!(fixture
        .store
        .claim_outbox_key(
            Some(50),
            "budget-revoked-no-retry".into(),
            30,
            format!("launch/{}/{}", request.task_id, request.attempt_id),
        )
        .expect("durable unknown cannot be claimed again")
        .is_none());
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("restored read authority");
    assert_eq!(ledger.reservations.len(), 1);
    assert_eq!(ledger.revision, 2, "revocation and replay add no debit");
    assert!(ledger.reservations["native-reservation"].outstanding);
    assert_eq!(
        ledger.reservations["native-reservation"].settled,
        MinorUnits::new(0)
    );
    cleanup_parts(fixture.temp, fixture.task_loop, fixture.store);
}

#[test]
fn current_budget_dispatch_reaches_only_the_unconfigured_executor_without_a_child() {
    let (mut fixture, policy) = budget_fixture();
    let request = fixture.request.clone();
    let budget = budget_for(&fixture);
    assert!(matches!(
        fixture
            .task_loop
            .admit_reserved(request.clone(), fixture.plan, &Permit, budget)
            .expect("current budget admission"),
        AdmissionStatus::Queued(_)
    ));
    let writes_before = policy.writes.load(Ordering::SeqCst);
    assert!(matches!(
        fixture
            .task_loop
            .dispatch(&request.run_id, &FixedClock, &Permit)
            .expect("current exact budget replay permits the executor call"),
        TaskDispatchStatus::Held {
            runtime_attempted: true,
            ..
        }
    ));
    assert!(
        policy.writes.load(Ordering::SeqCst) >= writes_before + 2,
        "current budget authority is checked at dispatch and immediately before the executor"
    );
    assert!(
        !fixture.marker.exists(),
        "unconfigured ResourceGate cannot start a child"
    );
    assert!(
        fixture
            .task_loop
            .pending_diagnostics(&request.run_id)
            .is_none(),
        "no RunningAdmission or resource observation was created"
    );
    let ledger = fixture
        .store
        .reservation_ledger(read_proof())
        .expect("retained liability");
    assert_eq!(ledger.revision, 2);
    assert!(ledger.reservations["native-reservation"].outstanding);
    assert_eq!(
        ledger.reservations["native-reservation"].settled,
        MinorUnits::new(0)
    );
    assert!(matches!(
        fixture
            .task_loop
            .retry_admission(&request.run_id, &Permit)
            .expect("unknown is retained"),
        AdmissionStatus::Held(HoldReason::DispatchUncertain)
    ));
    cleanup_parts(fixture.temp, fixture.task_loop, fixture.store);
}
