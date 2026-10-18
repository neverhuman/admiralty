use admiralty_core::{DomainEvent, SourceIdentity, TaskContract};
use admiralty_store::{
    Store, StoreError,
    outbox::{OutboxError, OutboxIntent},
};

fn fixture() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("owned dispatcher fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private fixture");
    }
    let store = Store::open(directory.path().join("dispatch.sqlite")).expect("Store writer");
    let contract: TaskContract = serde_json::from_value(serde_json::json!({
        "schema_version":1,"project_id":"dispatch","root_id":"root","task_id":"one","revision":1,
        "title":"Dispatch","objective":"Fence an actual durable intent","non_goals":[],
        "owned_paths":["src"],"forbidden_paths":[],"read_first":[],"dependencies":[],
        "criteria":[{"id":"one","text":"fenced","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"true","args":[],"cwd":".","timeout_secs":1}],
        "authority_ref":"fixture:dispatch","max_attempts":1,"simulated":true
    }))
    .expect("valid fixture contract");
    store
        .append_with_intent(
            SourceIdentity {
                producer_id: "controller".into(),
                incarnation: "one".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-18T00:00:00Z".into(),
            OutboxIntent {
                idempotency_key: "launch-one".into(),
                payload: serde_json::json!({"kind":"fixture_launch"}),
                not_before: 10,
            },
        )
        .expect("atomic durable intent");
    (directory, store)
}

#[test]
fn writer_serializes_claims_and_exact_completion_without_advancing_domain_cursor() {
    let (_directory, store) = fixture();
    let before = store.snapshot().expect("before");
    let changes = store.subscribe_changes();
    assert!(matches!(
        store.claim_outbox_due(None, "executor".into(), 10),
        Err(StoreError::Outbox(OutboxError::UnknownClock))
    ));
    let clone = store.clone();
    let joins: Vec<_> = [store.clone(), clone]
        .into_iter()
        .enumerate()
        .map(|(n, store)| {
            std::thread::spawn(move || {
                store.claim_outbox_due(Some(10), format!("executor-{n}"), 10)
            })
        })
        .collect();
    let claims: Vec<_> = joins
        .into_iter()
        .filter_map(|thread| thread.join().expect("worker joins").expect("claim result"))
        .collect();
    assert_eq!(claims.len(), 1);
    let token = claims[0].token.clone();
    let mut forged = token.clone();
    forged.nonce.push('x');
    assert!(
        store
            .complete_outbox(forged, "receipt".into(), Some(11))
            .is_err()
    );
    store
        .complete_outbox(token.clone(), "receipt".into(), Some(11))
        .expect("exact completion");
    store
        .complete_outbox(token, "receipt".into(), Some(12))
        .expect("idempotent receipt");
    assert!(
        store
            .claim_outbox_due(Some(12), "executor-next".into(), 10)
            .expect("no redispatch")
            .is_none()
    );
    assert_eq!(store.snapshot().expect("after"), before);
    assert!(!changes.has_changed().expect("domain hint unchanged"));
}

#[test]
fn restart_retains_ambiguous_claims_and_never_automatically_releases_them() {
    let (directory, store) = fixture();
    let claim = store
        .claim_outbox_due(Some(10), "executor".into(), 2)
        .expect("claim")
        .expect("due effect");
    drop(store);
    let reopened = Store::open(directory.path().join("dispatch.sqlite")).expect("reopen");
    assert!(
        reopened
            .claim_outbox_due(Some(13), "new-executor".into(), 2)
            .expect("expired hold")
            .is_none()
    );
    assert!(matches!(
        reopened.complete_outbox(claim.token, "late-receipt".into(), Some(13)),
        Err(StoreError::Outbox(OutboxError::ReconciliationRequired))
    ));
    assert!(
        reopened
            .claim_outbox_due(Some(14), "new-executor".into(), 2)
            .expect("still held")
            .is_none()
    );
}

#[test]
fn explicit_unknown_is_a_durable_hold_and_invalid_envelopes_do_not_claim() {
    let (_directory, store) = fixture();
    assert!(
        store
            .claim_outbox_due(Some(10), "x".repeat(129), 10)
            .is_err()
    );
    let claim = store
        .claim_outbox_due(Some(10), "executor".into(), 10)
        .expect("claim")
        .expect("due effect");
    store
        .fail_outbox_unknown(claim.token.clone(), "disconnected after launch".into())
        .expect("unknown hold");
    assert!(matches!(
        store.complete_outbox(claim.token, "receipt".into(), Some(11)),
        Err(StoreError::Outbox(OutboxError::ReconciliationRequired))
    ));
    assert!(
        store
            .claim_outbox_due(Some(12), "other".into(), 10)
            .expect("no retry")
            .is_none()
    );
}
