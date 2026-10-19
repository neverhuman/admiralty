//! Actual sole-writer journal tests; fixture policy grants only local recording.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{
    DomainEvent, LifecycleAction, LifecycleCommand, PrincipalId, SourceIdentity, TaskContract,
    TaskView, content_digest,
    economics::{DecimalU64, DecimalU128, ObservedAmount},
};
use admiralty_store::{
    LifecycleAuthority, LifecycleEffect, Store, StoreAuthorities, StoreError, usage_observations::*,
};
use rusqlite::Connection;
use std::{
    any::Any,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
struct Credential;
struct Policy {
    revoked: AtomicBool,
    calls: AtomicUsize,
    revoke_at: AtomicUsize,
}
impl Policy {
    fn new() -> Self {
        Self {
            revoked: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            revoke_at: AtomicUsize::new(usize::MAX),
        }
    }
}
impl UsageAuthority for Policy {
    fn authorize_write(&self, b: &UsageWriteBinding<'_>, p: &(dyn Any + Send + Sync)) -> bool {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.revoke_at.load(Ordering::SeqCst) {
            self.revoked.store(true, Ordering::SeqCst);
        }
        p.is::<Credential>()
            && !self.revoked.load(Ordering::SeqCst)
            && b.command.actor == "agent"
            && b.command.source.producer_id == "usage-only"
            && b.command.source.incarnation == "boot"
            && b.command.policy_digest == content_digest(&"usage-policy").unwrap()
            && b.current_task.contract.task_id == b.original_contract.task_id
            && b.original_contract.revision == b.command.scope.contract_revision
    }
    fn authorize_read(&self, b: &UsageReadBinding<'_>, p: &(dyn Any + Send + Sync)) -> bool {
        p.is::<Credential>()
            && !self.revoked.load(Ordering::SeqCst)
            && b.request.actor == "agent"
            && b.request.scope.project_id.as_str() == "project"
            && b.current_task.contract.task_id == b.request.scope.task_id
    }
}
fn proof() -> UsageProof {
    Arc::new(Credential)
}
fn contract() -> TaskContract {
    serde_json::from_value(serde_json::json!({
 "schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,
 "title":"Keep native usage","objective":"Record unknown counters without grants","non_goals":["No settlement"],"owned_paths":["src/lib.rs"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],
 "criteria":[{"id":"c","text":"Usage survives restart","check_ids":["unit"]}],"checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":1}],"authority_ref":"grant:task","max_attempts":1,"simulated":true})).unwrap()
}
fn unknown() -> ReportedFact<String> {
    ReportedFact::Unknown {
        reason: "provider omitted".into(),
    }
}
fn command() -> UsageCommand {
    let c = contract();
    UsageCommand {
        schema_version: 1,
        scope: UsageScope {
            project_id: c.project_id.clone(),
            root_id: c.root_id.clone(),
            task_id: c.task_id.clone(),
            contract_revision: c.revision,
            contract_digest: content_digest(&c).unwrap(),
        },
        command_id: "command1".into(),
        source: SourceIdentity {
            producer_id: "usage-only".into(),
            incarnation: "boot".into(),
            sequence: "1".into(),
        },
        actor: "agent".into(),
        policy_digest: content_digest(&"usage-policy").unwrap(),
        capture_ref: "artifact:capture1".into(),
        capture_digest: content_digest(&"capture1").unwrap(),
        observation_id: "observation1".into(),
        revision: DecimalU64(1),
        parent: None,
        correction_reason: None,
        observation: NativeUsageObservation {
            provider: ReportedFact::Reported {
                value: "codex".into(),
            },
            model: unknown(),
            effort: unknown(),
            session: ReportedFact::Reported {
                value: "session1".into(),
            },
            request: unknown(),
            turn: unknown(),
            event: unknown(),
            attempt: unknown(),
            segment: ReportedFact::Reported {
                value: "segment1".into(),
            },
            role: ReportedFact::Reported {
                value: "worker".into(),
            },
            purpose: unknown(),
            time: unknown(),
            inclusion: ReportedFact::Unknown {
                reason: "overlap not established".into(),
            },
            counters: vec![NativeCounter {
                category: "input".into(),
                unit: unknown(),
                semantics: CounterSemantics::Cumulative {
                    opening: ObservedAmount::Unknown {
                        reason: "left censored".into(),
                    },
                },
                amount: ObservedAmount::Unknown {
                    reason: "not reported".into(),
                },
            }],
        },
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    policy: Arc<Policy>,
    command: UsageCommand,
}
fn open(path: &Path, policy: Arc<Policy>) -> Store {
    Store::open_with_startup_authorities(
        path,
        StoreAuthorities {
            usage: Some(policy),
            ..Default::default()
        },
    )
    .unwrap()
}
fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let policy = Arc::new(Policy::new());
    let store = open(&dir.path().join("store.sqlite"), policy.clone());
    store
        .append(
            SourceIdentity {
                producer_id: "operator".into(),
                incarnation: "boot".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: contract(),
            },
            "2026-10-19T07:00:00Z".into(),
        )
        .unwrap();
    Fixture {
        dir,
        store,
        policy,
        command: command(),
    }
}
fn read(c: &UsageCommand, limit: u32) -> UsageRead {
    UsageRead {
        scope: c.scope.clone(),
        actor: "agent".into(),
        limit,
        cursor: None,
    }
}
fn correction(c: &UsageCommand, r: &UsageResult, n: u64) -> UsageCommand {
    let mut c = c.clone();
    c.command_id = format!("command{n}");
    c.source.sequence = n.to_string();
    c.revision = DecimalU64(n);
    c.parent = Some(RevisionRef {
        revision: r.receipt.revision.clone(),
        receipt_digest: r.receipt_digest.clone(),
    });
    c.correction_reason = Some("late provider amount".into());
    c.observation.counters[0].amount = ObservedAmount::Known {
        amount: DecimalU128(n as u128 * 100),
    };
    c
}
fn is_usage(e: StoreError, f: impl FnOnce(&UsageError) -> bool) -> bool {
    matches!(e,StoreError::Usage(ref e) if f(e))
}
fn count(path: &Path, table: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
#[test]
fn default_deny_opaque_proof_and_writer_time_revocation() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    assert!(is_usage(
        f.store
            .record_usage_observation(f.command.clone(), Arc::new("credential label"))
            .unwrap_err(),
        |e| matches!(e, UsageError::Denied)
    ));
    f.policy.revoke_at.store(2, Ordering::SeqCst);
    f.policy.calls.store(0, Ordering::SeqCst);
    assert!(is_usage(
        f.store
            .record_usage_observation(f.command.clone(), proof())
            .unwrap_err(),
        |e| matches!(e, UsageError::Denied)
    ));
    assert_eq!(count(&path, "usage_observation_origins"), 0);
    assert_eq!(count(&path, "usage_observation_revisions"), 0);
    f.policy.revoked.store(false, Ordering::SeqCst);
    f.policy.revoke_at.store(usize::MAX, Ordering::SeqCst);
    f.store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    f.policy.revoked.store(true, Ordering::SeqCst);
    assert!(
        f.store
            .record_usage_observation(f.command.clone(), proof())
            .is_err()
    );
    assert!(
        f.store
            .read_usage_observations(read(&f.command, 1), proof())
            .is_err()
    );
    drop(f.store);
    let s = Store::open(&path).unwrap();
    assert!(is_usage(
        s.record_usage_observation(f.command, proof()).unwrap_err(),
        |e| matches!(e, UsageError::Denied)
    ));
}
#[test]
fn retry_returns_original_after_corrections_reopen_rebuild_backup_without_effects() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let before = f.store.snapshot().unwrap();
    let first = f
        .store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let c2 = correction(&f.command, &first, 2);
    f.store.record_usage_observation(c2, proof()).unwrap();
    let retry = f
        .store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(
        serde_json::to_vec(&retry.receipt).unwrap(),
        serde_json::to_vec(&first.receipt).unwrap()
    );
    assert_eq!(retry.receipt_digest, first.receipt_digest);
    assert!(
        !first.receipt.execution_granted
            && !first.receipt.model_qualified
            && !first.receipt.settled
    );
    assert_eq!(f.store.snapshot().unwrap(), before);
    let rebuilt = f.store.rebuild().unwrap();
    assert_eq!(rebuilt.projection, before.projection);
    assert_eq!(rebuilt.cursor.event_seq, before.cursor.event_seq);
    assert_ne!(
        rebuilt.cursor.recovery_generation,
        before.cursor.recovery_generation
    );
    let backup = f.dir.path().join("backup.sqlite");
    f.store.backup(&backup).unwrap();
    let backup_store = open(&backup, f.policy.clone());
    assert_eq!(
        backup_store.snapshot().unwrap().projection,
        before.projection
    );
    assert_eq!(
        backup_store
            .read_usage_observations(read(&f.command, 32), proof())
            .unwrap()
            .records
            .len(),
        2
    );
    drop(backup_store);
    drop(f.store);
    let store = open(&path, f.policy);
    assert_eq!(
        store.record_usage_observation(f.command, proof()).unwrap(),
        retry
    );
}
#[test]
fn command_source_revision_and_identity_collisions_do_not_mutate_history() {
    let f = fixture();
    let first = f
        .store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let mut variants = Vec::new();
    let mut a = f.command.clone();
    a.observation.counters[0].amount = ObservedAmount::Known {
        amount: DecimalU128(4),
    };
    variants.push(a);
    let mut a = f.command.clone();
    a.command_id = "newcommand".into();
    a.observation_id = "newobservation".into();
    variants.push(a);
    let mut a = f.command.clone();
    a.command_id = "newcommand".into();
    a.source.sequence = "9".into();
    variants.push(a);
    let mut a = correction(&f.command, &first, 2);
    a.observation.model = ReportedFact::Reported {
        value: "requested-is-not-observed".into(),
    };
    variants.push(a);
    let mut a = correction(&f.command, &first, 2);
    a.observation.counters[0].semantics = CounterSemantics::Cumulative {
        opening: ObservedAmount::Known {
            amount: DecimalU128(0),
        },
    };
    variants.push(a);
    for c in variants {
        assert!(is_usage(
            f.store.record_usage_observation(c, proof()).unwrap_err(),
            |e| matches!(e, UsageError::Collision)
        ));
    }
    assert_eq!(
        f.store
            .read_usage_observations(read(&f.command, 32), proof())
            .unwrap()
            .records
            .len(),
        1
    );
}
struct RevisionAuthority {
    command: LifecycleCommand,
}
impl LifecycleAuthority for RevisionAuthority {
    fn authorize(
        &self,
        s: &SourceIdentity,
        c: &LifecycleCommand,
        _: &TaskView,
        e: Option<&LifecycleEffect<'_>>,
    ) -> bool {
        s.producer_id == "operator" && s.sequence == "2" && c == &self.command && e.is_none()
    }
}
#[test]
fn correction_parent_cas_fork_and_original_contract_survive_task_advance() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let first = f
        .store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let c2 = correction(&f.command, &first, 2);
    let second = f
        .store
        .record_usage_observation(c2.clone(), proof())
        .unwrap();
    let mut fork = c2;
    fork.command_id = "fork".into();
    fork.source.sequence = "9".into();
    assert!(f.store.record_usage_observation(fork, proof()).is_err());
    let task = f
        .store
        .snapshot()
        .unwrap()
        .projection
        .tasks
        .get(&f.command.scope.task_id)
        .unwrap()
        .clone();
    let mut next = task.contract.clone();
    next.revision = 2;
    next.title = "Next contract".into();
    let lc = LifecycleCommand {
        protocol_version: 2,
        task_id: next.task_id.clone(),
        expected_task_digest: content_digest(&task).unwrap(),
        actor: PrincipalId::new("operator").unwrap(),
        action: LifecycleAction::ReviseContract { contract: next },
    };
    drop(f.store);
    let s = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            lifecycle: Some(Arc::new(RevisionAuthority {
                command: lc.clone(),
            })),
            usage: Some(f.policy.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    s.execute_lifecycle(
        SourceIdentity {
            producer_id: "operator".into(),
            incarnation: "boot".into(),
            sequence: "2".into(),
        },
        lc,
        "2026-10-19T07:01:00Z".into(),
        None,
    )
    .unwrap();
    let c3 = correction(&f.command, &second, 3);
    s.record_usage_observation(c3, proof()).unwrap();
    assert!(
        s.record_usage_observation(f.command.clone(), proof())
            .unwrap()
            .replayed
    );
    s.rebuild().unwrap();
    drop(s);
    Store::open(&path).unwrap();
}
#[test]
fn unknown_values_opening_and_inclusion_remain_unknown_not_zero() {
    let f = fixture();
    f.store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let p = f
        .store
        .read_usage_observations(read(&f.command, 1), proof())
        .unwrap();
    assert_eq!(p.records[0].command.observation, f.command.observation);
    let json = serde_json::to_string(&p).unwrap();
    assert!(json.contains("left censored"));
    assert!(!json.contains("requested_model"));
    assert!(!json.contains("\"amount\":\"0\""));
    let mut c = f.command.clone();
    c.observation.counters[0].unit = ReportedFact::Reported {
        value: "credits".into(),
    };
    assert!(f.store.record_usage_observation(c, proof()).is_err());
}
#[test]
fn cursor_is_scope_only_and_snapshot_does_not_include_later_revision() {
    let f = fixture();
    let r1 = f
        .store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let r2 = f
        .store
        .record_usage_observation(correction(&f.command, &r1, 2), proof())
        .unwrap();
    let p = f
        .store
        .read_usage_observations(read(&f.command, 1), proof())
        .unwrap();
    assert_eq!(p.head_sequence.0, 2);
    f.store
        .record_usage_observation(correction(&f.command, &r2, 3), proof())
        .unwrap();
    let mut q = read(&f.command, 1);
    q.cursor = p.next;
    let p2 = f.store.read_usage_observations(q.clone(), proof()).unwrap();
    assert_eq!(p2.records[0].command.revision.0, 2);
    assert!(p2.next.is_none());
    assert_eq!(p2.head_sequence.0, 2);
    q.scope.contract_digest = content_digest(&"foreign scope").unwrap();
    assert!(f.store.read_usage_observations(q, proof()).is_err());
    let mut q = read(&f.command, 1);
    q.actor = "stranger".into();
    assert!(f.store.read_usage_observations(q, proof()).is_err());
}
#[test]
fn sqlite_fault_rolls_back_origin_and_revision_then_same_command_succeeds() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER usage_fixture_fault BEFORE INSERT ON usage_observation_revisions BEGIN SELECT RAISE(ABORT,'fixture fault'); END;").unwrap();
    assert!(is_usage(
        f.store
            .record_usage_observation(f.command.clone(), proof())
            .unwrap_err(),
        |e| matches!(e, UsageError::Sqlite(_))
    ));
    assert_eq!(count(&path, "usage_observation_origins"), 0);
    assert_eq!(count(&path, "usage_observation_revisions"), 0);
    db.execute_batch("DROP TRIGGER usage_fixture_fault;")
        .unwrap();
    f.store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    drop(db);
    drop(f.store);
    Store::open(&path).unwrap();
}
#[test]
fn corrupted_receipt_rejected_by_read_rebuild_backup_and_reopen() {
    let f = fixture();
    f.store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let path = f.dir.path().join("store.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TRIGGER usage_observation_revisions_no_update;")
        .unwrap();
    let raw: String = db
        .query_row(
            "SELECT receipt_json FROM usage_observation_revisions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut v: UsageReceipt = serde_json::from_str(&raw).unwrap();
    v.model_qualified = true;
    db.execute(
        "UPDATE usage_observation_revisions SET receipt_json=?1",
        [serde_json::to_string(&v).unwrap()],
    )
    .unwrap();
    assert!(
        f.store
            .read_usage_observations(read(&f.command, 1), proof())
            .is_err()
    );
    assert!(f.store.rebuild().is_err());
    let out = f.dir.path().join("badbackup.sqlite");
    assert!(f.store.backup(&out).is_err());
    assert!(!out.exists());
    drop(db);
    drop(f.store);
    assert!(Store::open(&path).is_err());
    assert_eq!(count(&path, "usage_observation_revisions"), 1);
}
#[test]
fn input_bounds_immutable_rows_and_oversize_stored_bytes_are_enforced() {
    let f = fixture();
    let mut c = f.command.clone();
    c.capture_ref = "x".repeat(MAX_COMMAND_BYTES + 1);
    assert!(matches!(validate_command(&c), Err(UsageError::Invalid)));
    let mut c = f.command.clone();
    c.observation.counters = vec![c.observation.counters[0].clone(); 9];
    assert!(validate_command(&c).is_err());
    let mut c = f.command.clone();
    c.source.sequence = "01".into();
    assert!(validate_command(&c).is_err());
    let mut q = read(&f.command, 33);
    assert!(validate_read(&q).is_err());
    q.limit = 1;
    q.actor = "x".repeat(4097);
    assert!(validate_read(&q).is_err());
    f.store
        .record_usage_observation(f.command.clone(), proof())
        .unwrap();
    let path = f.dir.path().join("store.sqlite");
    let db = Connection::open(&path).unwrap();
    assert!(
        db.execute("DELETE FROM usage_observation_revisions", [])
            .is_err()
    );
    db.execute_batch(
        "DROP TRIGGER usage_observation_revisions_no_update; PRAGMA ignore_check_constraints=ON;",
    )
    .unwrap();
    db.execute(
        "UPDATE usage_observation_revisions SET source_sequence=?1",
        ["1".repeat(4096)],
    )
    .unwrap();
    assert!(
        f.store
            .read_usage_observations(read(&f.command, 1), proof())
            .is_err()
    );
    drop(db);
    drop(f.store);
    assert!(Store::open(&path).is_err());
}

#[test]
fn bounded_origin_capacity_refuses_before_decoding_and_never_evicts() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let db = Connection::open(&path).unwrap();
    // Deliberately corrupt fixture rows exercise aggregate bound BEFORE JSON
    // decoding, not an alternate journal producer or a valid receipt history.
    db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1025) INSERT INTO usage_observation_origins SELECT 'sha256:'||lower(hex(zeroblob(32))),'corrupt'||x,'{}','{}','sha256:'||lower(hex(zeroblob(32))) FROM n;").unwrap();
    assert!(is_usage(
        f.store
            .read_usage_observations(read(&f.command, 1), proof())
            .unwrap_err(),
        |e| matches!(e, UsageError::Capacity)
    ));
    assert!(is_usage(
        f.store
            .record_usage_observation(f.command.clone(), proof())
            .unwrap_err(),
        |e| matches!(e, UsageError::Capacity)
    ));
    assert_eq!(count(&path, "usage_observation_origins"), 1025);
    assert!(f.store.rebuild().is_err());
    let b = f.dir.path().join("no.sqlite");
    assert!(f.store.backup(&b).is_err());
    assert!(!b.exists());
    drop(db);
    drop(f.store);
    assert!(Store::open(&path).is_err());
}
