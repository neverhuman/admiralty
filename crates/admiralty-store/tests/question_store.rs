//! Actual sole-writer question recording; no model, delivery or decision authority.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{
    DomainEvent, LifecycleAction, LifecycleCommand, PrincipalId, SourceIdentity, TaskContract,
    TaskView, content_digest,
    coordination::questions::{QuestionEvent, QuestionScope, QuestionStatus},
};
use admiralty_store::{
    LifecycleAuthority, LifecycleEffect, Store, StoreError,
    questions::{
        JournalScope, QuestionAuthority, QuestionCommand, QuestionCommandBinding,
        QuestionProvenance, QuestionRead, QuestionStoreError,
    },
};
use rusqlite::Connection;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Policy {
    scope: JournalScope,
    allowed: Mutex<BTreeSet<String>>,
    revoked: AtomicBool,
    calls: AtomicUsize,
}
impl Policy {
    fn allow(&self, command: &QuestionCommand) {
        self.allowed
            .lock()
            .unwrap()
            .insert(content_digest(command).unwrap());
    }
}
impl QuestionAuthority for Policy {
    fn authorize_command(&self, b: &QuestionCommandBinding<'_>) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        !self.revoked.load(Ordering::SeqCst)
            && b.command.scope == self.scope
            && b.current_task.contract.task_id == self.scope.question.task_id
            && b.command.binding.source.producer_id == "question-producer"
            && b.command.binding.source.incarnation == "boot-a"
            && self.allowed.lock().unwrap().contains(b.command_digest)
    }
    fn authorize_read(&self, r: &QuestionRead, t: &TaskView) -> bool {
        !self.revoked.load(Ordering::SeqCst)
            && r.scope == self.scope
            && ["worker", "commodore"].contains(&r.actor.as_str())
            && t.contract.project_id == self.scope.question.project_id
            && t.contract.root_id == self.scope.root_id
    }
}
fn contract() -> TaskContract {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,
        "title":"Retain a scoped question","objective":"Record uncertain input without authority",
        "non_goals":["No automatic answer or provider input"],"owned_paths":["src/lib.rs"],
        "forbidden_paths":["private"],"read_first":[],"dependencies":[],
        "criteria":[{"id":"c","text":"Question survives restart","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":1}],
        "authority_ref":"grant:task","max_attempts":1,"simulated":true
    }))
    .unwrap()
}
fn source(n: u64) -> SourceIdentity {
    SourceIdentity {
        producer_id: "question-producer".into(),
        incarnation: "boot-a".into(),
        sequence: n.to_string(),
    }
}
fn open_command(contract: &TaskContract) -> QuestionCommand {
    let scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: QuestionScope {
            question_id: admiralty_core::MessageId::new("question").unwrap(),
            project_id: contract.project_id.clone(),
            task_id: contract.task_id.clone(),
        },
        authority_ref: contract.authority_ref.clone(),
    };
    QuestionCommand {
        scope: scope.clone(),
        command_id: admiralty_core::MessageId::new("open-command").unwrap(),
        event: QuestionEvent::Open {
            event_id: admiralty_core::MessageId::new("open-event").unwrap(),
            scope: scope.question.clone(),
            authority_ref: scope.authority_ref.clone(),
            asker: PrincipalId::new("worker").unwrap(),
            recipient: PrincipalId::new("commodore").unwrap(),
            allowed_recipients: vec![PrincipalId::new("commodore").unwrap()],
            requirement: "Choose the exact interface revision".into(),
            question: "Which version is valid? Outcome remains unknown.".into(),
            options: vec!["Version A".into(), "Need more evidence".into()],
            deadline_utc: Some(100),
        },
        binding: QuestionProvenance {
            actor: PrincipalId::new("worker").unwrap(),
            source: source(1),
            expected_contract_revision: contract.revision,
            expected_contract_digest: content_digest(contract).unwrap(),
            policy_digest: content_digest(&"protected-policy-v1").unwrap(),
            evidence_refs: vec!["artifact:question-source".into()],
        },
    }
}
fn answer(open: &QuestionCommand) -> QuestionCommand {
    QuestionCommand {
        scope: open.scope.clone(),
        command_id: admiralty_core::MessageId::new("answer-command").unwrap(),
        event: QuestionEvent::Answer {
            event_id: admiralty_core::MessageId::new("answer-event").unwrap(),
            scope: open.scope.question.clone(),
            expected_revision: 1,
            authority_ref: open.scope.authority_ref.clone(),
            actor: PrincipalId::new("commodore").unwrap(),
            answer: "Ambiguous: additional evidence is required; no authorization given.".into(),
        },
        binding: QuestionProvenance {
            actor: PrincipalId::new("commodore").unwrap(),
            source: source(2),
            ..open.binding.clone()
        },
    }
}
fn read(open: &QuestionCommand) -> QuestionRead {
    QuestionRead {
        scope: open.scope.clone(),
        actor: PrincipalId::new("worker").unwrap(),
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    policy: Arc<Policy>,
    open: QuestionCommand,
}
fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let contract = contract();
    let open = open_command(&contract);
    let policy = Arc::new(Policy {
        scope: open.scope.clone(),
        allowed: Mutex::new(BTreeSet::new()),
        revoked: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    policy.allow(&open);
    let store =
        Store::open_with_question_authority(dir.path().join("store.sqlite"), policy.clone())
            .unwrap();
    store
        .append(
            SourceIdentity {
                producer_id: "operator".into(),
                incarnation: "boot".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-19T03:00:00Z".into(),
        )
        .unwrap();
    Fixture {
        dir,
        store,
        policy,
        open,
    }
}
fn is_question(error: StoreError, predicate: impl FnOnce(&QuestionStoreError) -> bool) -> bool {
    matches!(error,StoreError::Question(ref e) if predicate(e))
}
fn count(path: &Path, table: &str) -> i64 {
    assert!(
        [
            "durable_questions",
            "durable_question_commands",
            "durable_question_events",
            "durable_question_authorizations"
        ]
        .contains(&table)
    );
    Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn lost_ack_and_historical_retry_do_not_rewind_answer_or_duplicate_effects() {
    let f = fixture();
    let first = f.store.question_command(f.open.clone()).unwrap();
    let answer = answer(&f.open);
    f.policy.allow(&answer);
    let answered = f.store.question_command(answer.clone()).unwrap();
    assert_eq!(answered.revision, "2");
    assert_eq!(f.store.question_command(answer).unwrap(), answered);
    assert_eq!(f.store.question_command(f.open.clone()).unwrap(), first);
    let view = f.store.question(read(&f.open)).unwrap().unwrap();
    assert_eq!(view.state.revision, 2);
    assert!(
        matches!(view.state.status,QuestionStatus::Answered{ref answer} if answer.contains("Ambiguous"))
    );
    assert_eq!(
        count(
            &f.dir.path().join("store.sqlite"),
            "durable_question_events"
        ),
        2
    );
    assert_eq!(
        count(
            &f.dir.path().join("store.sqlite"),
            "durable_question_authorizations"
        ),
        2
    );
    assert_eq!(f.policy.calls.load(Ordering::SeqCst), 4);
}

#[test]
fn current_policy_denies_default_writes_reads_and_revoked_exact_retries() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let receipt = f.store.question_command(f.open.clone()).unwrap();
    f.policy.revoked.store(true, Ordering::SeqCst);
    assert!(is_question(
        f.store.question_command(f.open.clone()).unwrap_err(),
        |e| matches!(e, QuestionStoreError::AccessDenied)
    ));
    assert!(is_question(
        f.store.question(read(&f.open)).unwrap_err(),
        |e| matches!(e, QuestionStoreError::AccessDenied)
    ));
    drop(f.store);
    let denied = Store::open(&path).unwrap();
    assert!(is_question(
        denied.question_command(f.open.clone()).unwrap_err(),
        |e| matches!(e, QuestionStoreError::AccessDenied)
    ));
    assert!(is_question(
        denied.question(read(&f.open)).unwrap_err(),
        |e| matches!(e, QuestionStoreError::AccessDenied)
    ));
    drop(denied);
    f.policy.revoked.store(false, Ordering::SeqCst);
    let reopened = Store::open_with_question_authority(path, f.policy).unwrap();
    assert_eq!(reopened.question_command(f.open).unwrap(), receipt);
}

#[test]
fn scope_source_principal_and_provenance_collisions_fail_without_rows() {
    let f = fixture();
    let mut wrong = f.open.clone();
    wrong.scope.root_id = admiralty_core::RootId::new("foreign-root").unwrap();
    f.policy.allow(&wrong);
    assert!(f.store.question_command(wrong).is_err());
    let mut wrong = f.open.clone();
    wrong.binding.actor = PrincipalId::new("impostor").unwrap();
    f.policy.allow(&wrong);
    assert!(f.store.question_command(wrong).is_err());
    let mut wrong = f.open.clone();
    wrong.binding.evidence_refs[0] = "x".repeat(513);
    f.policy.allow(&wrong);
    assert!(f.store.question_command(wrong).is_err());
    assert_eq!(
        count(&f.dir.path().join("store.sqlite"), "durable_questions"),
        0
    );
    f.store.question_command(f.open.clone()).unwrap();
    let mut changed = f.open.clone();
    changed.binding.policy_digest = content_digest(&"other-policy").unwrap();
    f.policy.allow(&changed);
    assert!(is_question(
        f.store.question_command(changed).unwrap_err(),
        |e| matches!(e, QuestionStoreError::IdentityCollision)
    ));
    let mut duplicate = f.open.clone();
    duplicate.command_id = admiralty_core::MessageId::new("other-command").unwrap();
    f.policy.allow(&duplicate);
    assert!(is_question(
        f.store.question_command(duplicate).unwrap_err(),
        |e| matches!(e, QuestionStoreError::IdentityCollision)
    ));
    let mut wrong_read = read(&f.open);
    wrong_read.actor = PrincipalId::new("impostor").unwrap();
    assert!(f.store.question(wrong_read).is_err());
    assert_eq!(
        count(
            &f.dir.path().join("store.sqlite"),
            "durable_question_commands"
        ),
        1
    );
}

#[test]
fn unknown_deadline_never_reads_a_clock_and_unanswered_survives_backup_reopen() {
    let f = fixture();
    f.store.question_command(f.open.clone()).unwrap();
    let deadline = QuestionCommand {
        command_id: admiralty_core::MessageId::new("unknown-deadline").unwrap(),
        event: QuestionEvent::EvaluateDeadline {
            event_id: admiralty_core::MessageId::new("deadline-event").unwrap(),
            scope: f.open.scope.question.clone(),
            expected_revision: 1,
            authority_ref: f.open.scope.authority_ref.clone(),
            actor: PrincipalId::new("worker").unwrap(),
            observed_utc: None,
        },
        binding: QuestionProvenance {
            source: source(2),
            ..f.open.binding.clone()
        },
        ..f.open.clone()
    };
    f.policy.allow(&deadline);
    assert!(f.store.question_command(deadline).is_err());
    f.store.rebuild().unwrap();
    let backup = f.dir.path().join("backup.sqlite");
    f.store.backup(&backup).unwrap();
    drop(f.store);
    for path in [f.dir.path().join("store.sqlite"), backup] {
        let store = Store::open_with_question_authority(path, f.policy.clone()).unwrap();
        let q = store.question(read(&f.open)).unwrap().unwrap();
        assert_eq!(q.state.status, QuestionStatus::Open);
        assert_eq!(q.state.deadline_utc, Some(100));
        assert_eq!(q.state.revision, 1);
    }
}

struct RevisionAuthority {
    source: SourceIdentity,
    command: LifecycleCommand,
}
impl LifecycleAuthority for RevisionAuthority {
    fn authorize(
        &self,
        s: &SourceIdentity,
        c: &LifecycleCommand,
        t: &TaskView,
        e: Option<&LifecycleEffect<'_>>,
    ) -> bool {
        s == &self.source
            && c == &self.command
            && content_digest(t).unwrap() == c.expected_task_digest
            && e.is_none()
    }
}
#[test]
fn legitimate_task_revision_keeps_database_open_and_original_retry_but_reports_stale_actions() {
    let f = fixture();
    let original = f.store.question_command(f.open.clone()).unwrap();
    let path = f.dir.path().join("store.sqlite");
    let task = f.store.snapshot().unwrap().projection.tasks[&f.open.scope.question.task_id].clone();
    drop(f.store);
    let mut next = task.contract.clone();
    next.revision = 2;
    next.title = "Changed contract".into();
    let command = LifecycleCommand {
        protocol_version: 2,
        task_id: next.task_id.clone(),
        expected_task_digest: content_digest(&task).unwrap(),
        actor: PrincipalId::new("operator").unwrap(),
        action: LifecycleAction::ReviseContract {
            contract: next.clone(),
        },
    };
    let authority = Arc::new(RevisionAuthority {
        source: SourceIdentity {
            producer_id: "operator".into(),
            incarnation: "boot".into(),
            sequence: "2".into(),
        },
        command: command.clone(),
    });
    let store = Store::open_with_lifecycle_authority(&path, authority.clone()).unwrap();
    store
        .execute_lifecycle(
            authority.source.clone(),
            command,
            "2026-10-19T03:01:00Z".into(),
            None,
        )
        .unwrap();
    drop(store);
    let store = Store::open_with_question_authority(&path, f.policy.clone()).unwrap();
    assert_eq!(store.question_command(f.open.clone()).unwrap(), original);
    assert!(is_question(
        store.question(read(&f.open)).unwrap_err(),
        |e| matches!(e, QuestionStoreError::StaleContract)
    ));
    let mut answer = answer(&f.open);
    answer.binding.expected_contract_revision = 2;
    answer.binding.expected_contract_digest = content_digest(&next).unwrap();
    f.policy.allow(&answer);
    assert!(is_question(
        store.question_command(answer).unwrap_err(),
        |e| matches!(e, QuestionStoreError::StaleContract)
    ));
    store.rebuild().unwrap();
    store.backup(&f.dir.path().join("history.sqlite")).unwrap();
    assert_eq!(count(&path, "durable_question_events"), 1);
}

#[test]
fn late_sql_failure_rolls_back_question_event_receipt_and_provenance() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    let injected = Connection::open(&path).unwrap();
    injected.execute_batch("CREATE TRIGGER fail_question_provenance BEFORE INSERT ON durable_question_authorizations BEGIN SELECT RAISE(ABORT,'owned injected late failure'); END;").unwrap();
    assert!(f.store.question_command(f.open.clone()).is_err());
    for table in [
        "durable_questions",
        "durable_question_events",
        "durable_question_commands",
        "durable_question_authorizations",
    ] {
        assert_eq!(count(&path, table), 0)
    }
    injected
        .execute_batch("DROP TRIGGER fail_question_provenance;")
        .unwrap();
    assert_eq!(f.store.question_command(f.open).unwrap().revision, "1");
}

#[test]
fn corrupt_provenance_fails_rebuild_backup_and_reopen_without_repairing_history() {
    let f = fixture();
    f.store.question_command(f.open.clone()).unwrap();
    let path = f.dir.path().join("store.sqlite");
    let injected = Connection::open(&path).unwrap();
    injected
        .execute(
            "UPDATE durable_question_authorizations SET original_contract_digest=?1",
            [format!("sha256:{}", "0".repeat(64))],
        )
        .unwrap();
    assert!(f.store.rebuild().is_err());
    let backup = f.dir.path().join("rejected.sqlite");
    assert!(f.store.backup(&backup).is_err());
    assert!(!backup.exists());
    drop(f.store);
    drop(injected);
    assert!(Store::open(&path).is_err());
    assert_eq!(count(&path, "durable_question_events"), 1);
}

#[test]
fn global_question_cap_fails_closed_before_reading_unbounded_histories() {
    let f = fixture();
    let path = f.dir.path().join("store.sqlite");
    drop(f.store);
    let injected = Connection::open(&path).unwrap();
    injected.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<4097) INSERT INTO durable_questions(question_id,root_id,project_id,task_id,authority_ref,event_count,head_digest,revision,projection_json,projection_digest) SELECT 'q'||x,'r'||(x/1024),'project','task','grant:task',1,'genesis','1','{}','sha256:'||lower(hex(zeroblob(32))) FROM n;").unwrap();
    drop(injected);
    assert!(is_question(
        Store::open(&path).err().expect("oversized journal denied"),
        |e| matches!(e, QuestionStoreError::CapacityFull)
    ));
    assert_eq!(count(&path, "durable_questions"), 4097);
}

#[test]
fn missing_provenance_cannot_silently_downgrade_a_protected_answer() {
    let f = fixture();
    f.store.question_command(f.open.clone()).unwrap();
    let answer = answer(&f.open);
    f.policy.allow(&answer);
    f.store.question_command(answer.clone()).unwrap();
    let path = f.dir.path().join("store.sqlite");
    let injected = Connection::open(&path).unwrap();
    injected
        .execute(
            "DELETE FROM durable_question_authorizations WHERE command_id=?1",
            [answer.command_id.as_str()],
        )
        .unwrap();
    assert!(f.store.question(read(&f.open)).is_err());
    assert!(f.store.question_command(f.open.clone()).is_err());
    assert!(f.store.rebuild().is_err());
    drop(f.store);
    drop(injected);
    assert!(Store::open(&path).is_err());
    assert_eq!(count(&path, "durable_question_events"), 2);
}

#[test]
fn oversized_stored_source_fails_before_provenance_decode_or_historical_replay() {
    let f = fixture();
    f.store.question_command(f.open.clone()).unwrap();
    let path = f.dir.path().join("store.sqlite");
    let injected = Connection::open(&path).unwrap();
    injected
        .execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    injected
        .execute(
            "UPDATE durable_question_authorizations SET source_sequence=?1",
            ["9".repeat(4096)],
        )
        .unwrap();
    assert!(f.store.question(read(&f.open)).is_err());
    assert!(f.store.question_command(f.open.clone()).is_err());
    drop(f.store);
    drop(injected);
    assert!(Store::open(&path).is_err());
}
