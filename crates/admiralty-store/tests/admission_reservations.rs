//! Durable reservation journal tests through the Store's sole writer.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;
use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::reservations::{
    AccountId, CurrencyUnit, MinorUnits, ReservationEvent, ReservationLedger, ReservationScope,
    ScopeLimits, StopReason, reduce_reservation,
};
use admiralty_store::{Store, StoreAuthorities, admission_reservations::*};
use std::{
    any::Any,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use support::private_tempdir;

struct Credential;
struct Policy {
    revoked: AtomicBool,
}
impl ReservationAuthority for Policy {
    fn authorize_write(
        &self,
        binding: &ReservationBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        proof.is::<Credential>()
            && !self.revoked.load(Ordering::SeqCst)
            && binding.command.source.producer_id == "budget-agent"
    }
    fn authorize_read(&self, _ledger: &ReservationLedger, proof: &(dyn Any + Send + Sync)) -> bool {
        proof.is::<Credential>() && !self.revoked.load(Ordering::SeqCst)
    }
}
fn proof() -> ReservationProof {
    Arc::new(Credential)
}
fn unit() -> CurrencyUnit {
    CurrencyUnit::new("USD", 2).unwrap()
}
fn command(ledger: &ReservationLedger, id: &str, sequence: &str, limit: u64) -> ReservationCommand {
    ReservationCommand {
        schema_version: 1,
        command_id: id.into(),
        source: admiralty_core::SourceIdentity {
            producer_id: "budget-agent".into(),
            incarnation: "boot-1".into(),
            sequence: sequence.into(),
        },
        expected_ledger_revision: ledger.revision,
        expected_ledger_digest: ledger_digest(ledger).unwrap(),
        launch_intent_digest: None,
        event: ReservationEvent::SetLimits {
            event_id: id.into(),
            account_id: AccountId::new("account").unwrap(),
            limits: ScopeLimits {
                unit: unit(),
                account: Some(MinorUnits::new(limit)),
                project: None,
                root: None,
            },
        },
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
#[test]
fn journal_replays_exact_receipts_after_restart_and_later_commands() {
    let temp = private_tempdir();
    let path = temp.path().join("store.sqlite");
    let policy = Arc::new(Policy {
        revoked: AtomicBool::new(false),
    });
    let store = open(&path, policy.clone());
    let initial = store.reservation_ledger(proof()).unwrap();
    let first = command(&initial, "set-limits-1", "1", 500);
    let r1 = store.record_reservation(first.clone(), proof()).unwrap();
    assert!(!r1.replayed);
    assert_eq!(r1.receipt.ledger_revision, 1);
    let after_one = store.reservation_ledger(proof()).unwrap();
    let second = command(&after_one, "set-limits-2", "2", 900);
    store.record_reservation(second, proof()).unwrap();
    let replay = store.record_reservation(first, proof()).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, r1.receipt);
    assert_eq!(replay.receipt_digest, r1.receipt_digest);
    store.rebuild().unwrap();
    drop(store);
    let reopened = open(&path, policy);
    let ledger = reopened.reservation_ledger(proof()).unwrap();
    assert_eq!(ledger.revision, 2);
    assert_eq!(ledger.limits["account"].account, Some(MinorUnits::new(900)));
}
#[test]
fn source_content_reuse_and_revoked_policy_fail_closed() {
    let temp = private_tempdir();
    let path = temp.path().join("store.sqlite");
    let policy = Arc::new(Policy {
        revoked: AtomicBool::new(false),
    });
    let store = open(&path, policy.clone());
    let initial = store.reservation_ledger(proof()).unwrap();
    let first = command(&initial, "set-limits-1", "1", 500);
    store.record_reservation(first.clone(), proof()).unwrap();
    let mut collision = command(&initial, "different-command", "1", 400);
    collision.expected_ledger_revision = 1;
    collision.expected_ledger_digest =
        ledger_digest(&store.reservation_ledger(proof()).unwrap()).unwrap();
    assert!(store.record_reservation(collision, proof()).is_err());
    policy.revoked.store(true, Ordering::SeqCst);
    assert!(store.record_reservation(first, proof()).is_err());
    assert!(store.reservation_ledger(proof()).is_err());
}

#[test]
fn reducer_keeps_stop_requested_liability_and_fails_closed_on_unknown_or_wrong_units() {
    let scope = ReservationScope {
        project_id: ProjectId::new("project").unwrap(),
        root_id: RootId::new("root").unwrap(),
        task_id: TaskId::new("task").unwrap(),
        attempt_id: AttemptId::new("attempt").unwrap(),
        owner: PrincipalId::new("owner").unwrap(),
        incarnation: "boot-1".into(),
        contract_revision: 1,
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "grant:budget".into(),
        resource_claims_digest: format!("sha256:{}", "b".repeat(64)),
        account_id: AccountId::new("account").unwrap(),
        unit: unit(),
    };
    let mut ledger = ReservationLedger::default();
    ledger.limits.insert(
        "account".into(),
        ScopeLimits {
            unit: unit(),
            account: Some(MinorUnits::new(1000)),
            project: Some(MinorUnits::new(1000)),
            root: Some(MinorUnits::new(1000)),
        },
    );
    assert!(
        reduce_reservation(
            &ledger,
            ReservationEvent::Reserve {
                event_id: "reserve-1".into(),
                reservation_id: "reservation-1".into(),
                scope: {
                    let mut s = scope.clone();
                    s.unit = CurrencyUnit::new("CREDITS", 0).unwrap();
                    s
                },
                correlation_id: "correlation-1".into(),
                amount: MinorUnits::new(200),
            }
        )
        .is_err()
    );
    let reserved = reduce_reservation(
        &ledger,
        ReservationEvent::Reserve {
            event_id: "reserve-1".into(),
            reservation_id: "reservation-1".into(),
            scope: scope.clone(),
            correlation_id: "correlation-1".into(),
            amount: MinorUnits::new(200),
        },
    )
    .unwrap();
    let binding = reserved.reservations["reservation-1"].binding();
    let stopped = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "stop-1".into(),
            reservation_id: "reservation-1".into(),
            binding,
            reason: StopReason::Cancellation,
        },
    )
    .unwrap();
    assert!(stopped.reservations["reservation-1"].stop_requested);
    assert!(stopped.reservations["reservation-1"].outstanding);
    assert_eq!(stopped.unresolved_liabilities[&scope], MinorUnits::new(200));
    let unknown = ReservationScope {
        account_id: AccountId::new("unknown").unwrap(),
        ..scope
    };
    assert!(
        reduce_reservation(
            &ledger,
            ReservationEvent::Reserve {
                event_id: "reserve-unknown".into(),
                reservation_id: "reservation-unknown".into(),
                scope: unknown,
                correlation_id: "correlation-2".into(),
                amount: MinorUnits::new(1),
            }
        )
        .is_err()
    );
}

#[test]
fn command_revision_json_is_canonical_decimal() {
    let ledger = ReservationLedger::default();
    let request = command(&ledger, "set-limits-1", "1", 500);
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"expected_ledger_revision\":\"0\""));
    let malformed = json.replace(
        "\"expected_ledger_revision\":\"0\"",
        "\"expected_ledger_revision\":\"00\"",
    );
    assert!(serde_json::from_str::<ReservationCommand>(&malformed).is_err());
}

#[test]
fn oversized_corrupt_ledger_is_rejected_before_deserialization() {
    let temp = private_tempdir();
    let path = temp.path().join("store.sqlite");
    let policy = Arc::new(Policy {
        revoked: AtomicBool::new(false),
    });
    drop(open(&path, policy));
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE admission_reservation_ledger SET ledger_json=?1 WHERE singleton=1",
            ["x".repeat(1024 * 1024 + 1)],
        )
        .unwrap();
    drop(connection);
    assert!(Store::open(&path).is_err());
}

#[test]
fn launch_intent_digest_is_only_valid_for_reserve_commands() {
    let request = command(&ReservationLedger::default(), "set-limits-1", "1", 500);
    let mut malformed = request;
    malformed.launch_intent_digest = Some(format!("sha256:{}", "c".repeat(64)));
    assert!(validate_command(&malformed).is_err());
}
