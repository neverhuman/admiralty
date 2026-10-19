//! Actual sole-writer admission transactions; no provider is configured.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;
use admiralty_core::{DomainEvent, SourceIdentity, TaskContract, content_digest};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
    ScopeLimits,
};
use admiralty_store::{
    Store, StoreAuthorities,
    admission_reservations::*,
    outbox::{self, OutboxIntent},
};
use serde_json::json;
use std::{
    any::Any,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use support::private_tempdir;

struct Credential;
#[derive(Default)]
struct Policy {
    revoked: AtomicBool,
    calls: AtomicUsize,
    deny_at: AtomicUsize,
}
impl ReservationAuthority for Policy {
    fn authorize_write(&self, _: &ReservationBinding<'_>, proof: &(dyn Any + Send + Sync)) -> bool {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        proof.is::<Credential>()
            && !self.revoked.load(Ordering::SeqCst)
            && (self.deny_at.load(Ordering::SeqCst) == 0
                || call < self.deny_at.load(Ordering::SeqCst))
    }
    fn authorize_read(&self, _: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Credential>() && !self.revoked.load(Ordering::SeqCst)
    }
}
fn proof() -> ReservationProof {
    Arc::new(Credential)
}
fn source(producer: &str, seq: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: producer.into(),
        incarnation: "boot-1".into(),
        sequence: seq.to_string(),
    }
}
fn open(path: &Path, policy: Arc<Policy>) -> Store {
    Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            reservations: Some(policy),
            ..Default::default()
        },
    )
    .unwrap()
}
fn contract(id: &str) -> TaskContract {
    serde_json::from_value(json!({"schema_version":1,"project_id":"project-a","root_id":"root-a","task_id":id,
        "revision":1,"title":"Budget transaction fixture","objective":"Atomically persist a bounded claim",
        "non_goals":[],"owned_paths":["src/lib.rs"],"forbidden_paths":[],"read_first":[],"dependencies":[],
        "criteria":[{"id":"C1","text":"Commit atomically","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":10}],
        "authority_ref":"fixture:bounded","max_attempts":1,"simulated":true})).unwrap()
}
fn command(
    ledger: &ReservationLedger,
    id: &str,
    src: SourceIdentity,
    event: ReservationEvent,
) -> ReservationCommand {
    ReservationCommand {
        schema_version: 1,
        command_id: id.into(),
        source: src,
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: ledger_digest(ledger).unwrap(),
        launch_intent_digest: None,
        event,
    }
}
fn configure(store: &Store) {
    let ledger = store.reservation_ledger(proof()).unwrap();
    let event = ReservationEvent::SetLimits {
        event_id: "limits".into(),
        account_id: AccountId::new("account").unwrap(),
        limits: ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).unwrap(),
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    };
    store
        .record_reservation(
            command(&ledger, "limits", source("budget", 1), event),
            proof(),
        )
        .unwrap();
}
fn prepare(store: &Store, id: &str, seq: u64) -> (DomainEvent, OutboxIntent, ReservationCommand) {
    let c = contract(id);
    let attempt = admiralty_core::AttemptId::new(format!("attempt-{id}")).unwrap();
    let owner = admiralty_core::PrincipalId::new("worker").unwrap();
    let scope = ReservationScope {
        project_id: c.project_id.clone(),
        root_id: c.root_id.clone(),
        task_id: c.task_id.clone(),
        attempt_id: attempt.clone(),
        owner: owner.clone(),
        incarnation: "boot-1".into(),
        contract_revision: 1,
        contract_digest: content_digest(&c).unwrap(),
        authority_ref: "fixture:bounded".into(),
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
    let ledger = store.reservation_ledger(proof()).unwrap();
    let mut reservation = command(
        &ledger,
        &format!("reserve-{id}"),
        source("worker", seq),
        ReservationEvent::Reserve {
            event_id: format!("reserve-{id}"),
            reservation_id: format!("reservation-{id}"),
            scope,
            correlation_id: format!("launch-{id}"),
            amount: MinorUnits::new(60),
        },
    );
    reservation.launch_intent_digest = Some(outbox::intent_digest(&intent).unwrap());
    (event, intent, reservation)
}
fn submitted(store: &Store, id: &str, seq: u64) {
    store
        .append(
            source("worker", seq),
            DomainEvent::TaskSubmitted {
                contract: contract(id),
            },
            "original-time".into(),
        )
        .unwrap();
}
fn counts(path: &Path) -> (i64, i64, i64) {
    let c = rusqlite::Connection::open(path).unwrap();
    c.query_row("SELECT (SELECT COUNT(*) FROM domain_events),(SELECT COUNT(*) FROM outbox_intents),(SELECT COUNT(*) FROM admission_reservation_commands)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
}

#[test]
fn atomic_claim_exact_retry_after_later_commands_restart_and_backup() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    let first = store
        .claim_with_reservation(
            source("worker", 2),
            event.clone(),
            "first".into(),
            intent.clone(),
            res.clone(),
            proof(),
        )
        .unwrap();
    assert!(!first.claim.duplicate && !first.reservation.replayed);
    assert_eq!(counts(&path), (2, 1, 2));
    // Advancing the domain task and journal does not change the original admission receipt.
    let DomainEvent::TaskClaimed {
        task_id,
        attempt_id,
        ..
    } = event.clone()
    else {
        panic!()
    };
    store
        .append(
            source("worker", 3),
            DomainEvent::TaskStarted {
                task_id,
                attempt_id,
            },
            "later".into(),
        )
        .unwrap();
    let ledger = store.reservation_ledger(proof()).unwrap();
    let ReservationEvent::Reserve { reservation_id, .. } = &res.event else {
        panic!()
    };
    let stop = ReservationEvent::RequestStop {
        event_id: "stop".into(),
        reservation_id: reservation_id.clone(),
        binding: ledger.reservations[reservation_id].binding(),
        reason: admiralty_runtime::reservations::StopReason::LeaseExpiry,
    };
    store
        .record_reservation(command(&ledger, "stop", source("budget", 2), stop), proof())
        .unwrap();
    let retry = store
        .claim_with_reservation(
            source("worker", 2),
            event.clone(),
            "retry-time".into(),
            intent.clone(),
            res.clone(),
            proof(),
        )
        .unwrap();
    assert!(retry.claim.duplicate && retry.reservation.replayed);
    assert_eq!(retry.reservation.receipt, first.reservation.receipt);
    assert!(store.reservation_ledger(proof()).unwrap().reservations[reservation_id].outstanding);
    store.rebuild().unwrap();
    let backup = tmp.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    drop(store);
    for db in [&path, &backup] {
        let reopened = open(db, policy.clone());
        assert_eq!(
            reopened.reservation_ledger(proof()).unwrap().reservations[reservation_id].original,
            MinorUnits::new(60)
        );
        assert!(
            reopened
                .claim_with_reservation(
                    source("worker", 2),
                    event.clone(),
                    "again".into(),
                    intent.clone(),
                    res.clone(),
                    proof()
                )
                .unwrap()
                .claim
                .duplicate
        );
    }
}

#[test]
fn late_sql_failure_rolls_back_reservation_claim_and_outbox() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    let before = store.reservation_ledger(proof()).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER inject_admission_failure BEFORE INSERT ON event_reservation_associations BEGIN SELECT RAISE(ABORT,'owned test failure'); END;").unwrap();
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event.clone(),
                "first".into(),
                intent.clone(),
                res.clone(),
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (1, 0, 1));
    assert_eq!(store.reservation_ledger(proof()).unwrap(), before);
    assert_eq!(
        store.snapshot().unwrap().projection.tasks[&contract("task-a").task_id].state,
        admiralty_core::SchedulingState::Ready
    );
    c.execute_batch("DROP TRIGGER inject_admission_failure")
        .unwrap();
    store
        .claim_with_reservation(
            source("worker", 2),
            event,
            "retry".into(),
            intent,
            res,
            proof(),
        )
        .unwrap();
}

#[test]
fn final_authority_revocation_and_revoked_retry_have_no_effects() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    policy.calls.store(0, Ordering::SeqCst);
    policy.deny_at.store(6, Ordering::SeqCst);
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event.clone(),
                "first".into(),
                intent.clone(),
                res.clone(),
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (1, 0, 1));
    policy.deny_at.store(0, Ordering::SeqCst);
    store
        .claim_with_reservation(
            source("worker", 2),
            event.clone(),
            "retry".into(),
            intent.clone(),
            res.clone(),
            proof(),
        )
        .unwrap();
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event,
                "later".into(),
                intent,
                res,
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (2, 1, 2));
}

#[test]
fn ordinary_claim_cannot_be_upgraded_or_bypass_protected_retry() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    store
        .append_with_intent(
            source("worker", 2),
            event.clone(),
            "ordinary".into(),
            intent.clone(),
        )
        .unwrap();
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event,
                "upgrade".into(),
                intent,
                res,
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (2, 1, 1));
    submitted(&store, "task-b", 3);
    let (event, intent, res) = prepare(&store, "task-b", 4);
    store
        .claim_with_reservation(
            source("worker", 4),
            event.clone(),
            "protected".into(),
            intent.clone(),
            res,
            proof(),
        )
        .unwrap();
    assert!(
        store
            .append_with_intent(source("worker", 4), event, "without-proof".into(), intent)
            .is_err()
    );
    assert_eq!(counts(&path), (4, 2, 2));
}

#[test]
fn conflicting_outbox_commits_incident_without_debit() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure(&store);
    store
        .append_with_intent(
            source("worker", 1),
            DomainEvent::TaskSubmitted {
                contract: contract("task-a"),
            },
            "original".into(),
            OutboxIntent {
                idempotency_key: "launch/task-a".into(),
                payload: json!({"different":true}),
                not_before: 0,
            },
        )
        .unwrap();
    let (event, intent, res) = prepare(&store, "task-a", 2);
    let before = store.reservation_ledger(proof()).unwrap();
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event.clone(),
                "revoked-collision".into(),
                intent.clone(),
                res.clone(),
                proof()
            )
            .is_err()
    );
    let c = rusqlite::Connection::open(&path).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM outbox_integrity_incidents", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0);
    policy.revoked.store(false, Ordering::SeqCst);
    policy.calls.store(0, Ordering::SeqCst);
    policy.deny_at.store(3, Ordering::SeqCst);
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event.clone(),
                "late-revoked-collision".into(),
                intent.clone(),
                res.clone(),
                proof()
            )
            .is_err()
    );
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM outbox_integrity_incidents", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0);
    policy.deny_at.store(0, Ordering::SeqCst);
    policy.revoked.store(false, Ordering::SeqCst);
    assert!(
        store
            .claim_with_reservation(
                source("worker", 2),
                event,
                "collision".into(),
                intent,
                res,
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (1, 1, 1));
    assert_eq!(store.reservation_ledger(proof()).unwrap(), before);
    let c = rusqlite::Connection::open(&path).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM outbox_integrity_incidents", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(n > 0);
}

#[test]
fn racing_ready_claims_cannot_overspend_one_current_ledger() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure(&store);
    submitted(&store, "task-a", 1);
    submitted(&store, "task-b", 2);
    let a = prepare(&store, "task-a", 3);
    let b = prepare(&store, "task-b", 4);
    let x = store.clone();
    let y = store.clone();
    let t1 = std::thread::spawn(move || {
        x.claim_with_reservation(source("worker", 3), a.0, "a".into(), a.1, a.2, proof())
    });
    let t2 = std::thread::spawn(move || {
        y.claim_with_reservation(source("worker", 4), b.0, "b".into(), b.1, b.2, proof())
    });
    assert_eq!(
        [t1.join().unwrap().is_ok(), t2.join().unwrap().is_ok()]
            .iter()
            .filter(|v| **v)
            .count(),
        1
    );
    assert_eq!(counts(&path), (3, 1, 2));
    let snapshot = store.snapshot().unwrap();
    let remaining = if snapshot.projection.tasks[&contract("task-a").task_id].state
        == admiralty_core::SchedulingState::Ready
    {
        "task-a"
    } else {
        "task-b"
    };
    let (event, intent, res) = prepare(&store, remaining, 5);
    // A refreshed CAS still cannot spend the account's remaining 40 as 60.
    assert!(
        store
            .claim_with_reservation(
                source("worker", 5),
                event,
                "fresh-overdraft".into(),
                intent,
                res,
                proof()
            )
            .is_err()
    );
    assert_eq!(counts(&path), (3, 1, 2));
}

#[test]
fn missing_reverse_binding_is_corruption_on_reopen() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let policy = Arc::new(Policy::default());
    let store = open(&path, policy.clone());
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    store
        .claim_with_reservation(
            source("worker", 2),
            event,
            "first".into(),
            intent,
            res,
            proof(),
        )
        .unwrap();
    drop(store);
    let c = rusqlite::Connection::open(&path).unwrap();
    assert!(
        c.execute("DELETE FROM event_reservation_associations", [])
            .is_err()
    );
    c.execute_batch("DROP TRIGGER event_reservation_associations_no_delete; DELETE FROM event_reservation_associations;").unwrap();
    drop(c);
    assert!(
        Store::open_with_startup_authorities(
            &path,
            StoreAuthorities {
                reservations: Some(policy),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn corrupted_active_binding_cannot_be_replayed_as_ordinary() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    store
        .claim_with_reservation(
            source("worker", 2),
            event.clone(),
            "protected".into(),
            intent.clone(),
            res,
            proof(),
        )
        .unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER event_reservation_associations_no_delete; DELETE FROM event_reservation_associations;").unwrap();
    assert!(
        store
            .append_with_intent(
                source("worker", 2),
                event,
                "unprotected-retry".into(),
                intent
            )
            .is_err()
    );
    assert!(store.rebuild().is_err());
    assert!(
        store
            .backup(&tmp.path().join("corrupt-backup.sqlite"))
            .is_err()
    );
}

#[test]
fn wrong_contract_scope_unit_account_and_effect_deny_without_partial_writes() {
    let tmp = private_tempdir();
    let path = tmp.path().join("db.sqlite");
    let store = open(&path, Arc::new(Policy::default()));
    configure(&store);
    submitted(&store, "task-a", 1);
    let (event, intent, res) = prepare(&store, "task-a", 2);
    for which in 0..5 {
        let mut changed = res.clone();
        if let ReservationEvent::Reserve { scope, .. } = &mut changed.event {
            match which {
                0 => scope.contract_digest = format!("sha256:{}", "b".repeat(64)),
                1 => scope.project_id = admiralty_core::ProjectId::new("wrong-project").unwrap(),
                2 => scope.unit = CurrencyUnit::new("EUR", 2).unwrap(),
                3 => scope.account_id = AccountId::new("unknown-account").unwrap(),
                _ => changed.launch_intent_digest = Some(format!("sha256:{}", "c".repeat(64))),
            }
        }
        assert!(
            store
                .claim_with_reservation(
                    source("worker", 2),
                    event.clone(),
                    "denied".into(),
                    intent.clone(),
                    changed,
                    proof()
                )
                .is_err()
        );
        assert_eq!(counts(&path), (1, 0, 1));
    }
}
