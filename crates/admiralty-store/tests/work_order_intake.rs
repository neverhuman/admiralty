//! Real private Store/writer tests; trusted fixture authority is not a service issuer.
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, CommandId, DomainEvent, PrincipalId, ProjectId, RootId,
    SourceIdentity, TaskContract, TaskId, content_digest,
    intake::*,
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{
    Store, StoreError,
    intake::{AuthorizedIntakeContext, IntakeAuthority, IntakeStoreError},
    workplans::RegisterWorkplanRequest,
};
use rusqlite::Connection;
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
fn scope() -> AuthorizedIntakeScope {
    AuthorizedIntakeScope {
        principal: PrincipalId::new("operator").expect("principal"),
        project_id: Some(ProjectId::new("p").expect("project")),
        root_id: Some(RootId::new("r").expect("root")),
        authority_ref: "scope:intake".into(),
        authority_generation: 7,
    }
}
struct Authority {
    generation: AtomicU64,
    denied: AtomicBool,
    cap: AtomicUsize,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(7),
            denied: AtomicBool::new(false),
            cap: AtomicUsize::new(MAX_SOURCE_BYTES),
        }
    }
}
impl IntakeAuthority for Authority {
    fn authorize(
        &self,
        _: &RecordIntakeRequest,
    ) -> Result<AuthorizedIntakeContext, IntakeStoreError> {
        if self.denied.load(Ordering::SeqCst) {
            return Err(IntakeStoreError::Denied);
        }
        let mut scope = scope();
        scope.authority_generation = self.generation.load(Ordering::SeqCst);
        Ok(AuthorizedIntakeContext {
            scope,
            policy: IntakePolicy {
                max_source_bytes: self.cap.load(Ordering::SeqCst),
                ..IntakePolicy::default()
            },
        })
    }
}
fn private() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().expect("private scratch");
    std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private mode");
    d
}
fn open(path: &Path, authority: Arc<Authority>) -> Store {
    Store::open_with_authorities(path, None, Some(authority)).expect("authorized fixture Store")
}
fn task(id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("p").expect("project"),
        root_id: RootId::new("r").expect("root"),
        task_id: TaskId::new(id).expect("task"),
        revision: 1,
        title: "Explicit task".into(),
        objective: "Do not remove guard; limit 5 ms".into(),
        non_goals: vec!["Do not publish".into()],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec!["credentials".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "criterion".into(),
            text: "Named check passes within 5 ms".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "check-tool".into(),
            args: vec!["--limit-ms=5".into()],
            cwd: ".".into(),
            timeout_secs: 10,
        }],
        authority_ref: "task-policy".into(),
        max_attempts: 1,
        simulated: false,
    }
}
fn request(id: &str, source_id: &str, task_id: &str, expected_plan: u32) -> RecordIntakeRequest {
    let task = task(task_id);
    let delivery = DeliveryRequirement {
        schema_version: 1,
        task_id: task.task_id.clone(),
        contract_revision: 1,
        contract_digest: content_digest(&task).expect("digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repository".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Verified,
            },
        }],
    };
    let mut input = json!({"schema_version":1,"items":[{"text":"original explicit requirement; do not remove; 5 ms","outcomes":[{"id":"outcome","description":"Explicit passing outcome","source_spans":[{"start":0,"end":1}],"constraints":["Do not publish"],"task":task,"delivery":delivery}]}]});
    let mut raw = Vec::new();
    for _ in 0..8 {
        raw = serde_json::to_vec(&input).expect("source");
        let text = "original explicit requirement; do not remove; 5 ms";
        let start = std::str::from_utf8(&raw)
            .expect("UTF8")
            .find(text)
            .expect("text") as u32;
        let span = json!([{"start":start,"end":start+text.len() as u32}]);
        if input["items"][0]["outcomes"][0]["source_spans"] == span {
            break;
        }
        input["items"][0]["outcomes"][0]["source_spans"] = span;
    }
    let provenance = SourceProvenance {
        schema_version: 1,
        source_id: source_id.into(),
        revision: "1".into(),
        captured_at: "2026-10-19T00:00:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    };
    let policy = IntakePolicy::default();
    let preview = preview_intake(
        &raw,
        IntakeFormat::JsonBatch,
        &provenance,
        &scope(),
        &policy,
    )
    .expect("explicit preview");
    let outcome = &preview.requests[0].outcomes[0];
    let binding = WorkOrderTaskBinding {
        work_order_id: id.into(),
        work_order_revision: 1,
        request_index: 0,
        outcome_id: outcome.outcome.id.clone(),
        outcome_digest: outcome.outcome_digest.clone(),
        source_spans: outcome.outcome.source_spans.clone(),
        task_id: task.task_id.clone(),
        contract_revision: task.revision,
        contract_digest: content_digest(&task).expect("contract digest"),
    };
    RecordIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new(format!("command-{id}")).expect("command"),
        expected_work_order_revision: 0,
        plan_id: "plan".into(),
        expected_plan_revision: expected_plan,
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
        authority_generation: 7,
        work_order: WorkOrderRevision {
            schema_version: 1,
            id: id.into(),
            revision: 1,
            scope: preview.authorized_scope,
            provenance,
            title: None,
            outcome: Some("Explicit passing outcome".into()),
            rationale: None,
            constraints: vec![],
            requested_timing: None,
            raw_digest: preview.raw_digest,
            canonical_digest: preview.canonical_digest,
            parser_revision: preview.parser_revision,
            format: IntakeFormat::JsonBatch,
            source_payload: SourcePayload::InlineUtf8 {
                text: String::from_utf8(raw).expect("source UTF8"),
            },
        },
        selected_tasks: vec![SelectedTask {
            binding,
            task,
            delivery,
        }],
    }
}
fn count(path: &Path, table: &str) -> i64 {
    assert!(
        [
            "work_order_intake_orders",
            "work_order_intake_revisions",
            "work_order_intake_sources",
            "work_order_intake_bindings",
            "workplan_registry_plans",
            "workplan_registry_tasks",
            "domain_events",
            "outbox_intents"
        ]
        .contains(&table)
    );
    Connection::open(path)
        .expect("inspect fixture")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
}
fn query(r: &RecordIntakeRequest, limit: u16) -> DuplicateQuery {
    DuplicateQuery {
        schema_version: 1,
        project_id: ProjectId::new("p").expect("project"),
        raw_digest: r.work_order.raw_digest.clone(),
        canonical_digest: r.work_order.canonical_digest.clone(),
        origin: None,
        command_id: None,
        cursor: None,
        limit,
    }
}
#[test]
fn startup_authority_is_required_and_record_replay_preserves_exact_receipt() {
    let d = private();
    let path = d.path().join("store.db");
    let r = request("wo", "source", "task", 0);
    let denied = Store::open(&path).expect("default");
    assert!(matches!(
        denied.record_intake(r.clone()),
        Err(StoreError::Intake(IntakeStoreError::Denied))
    ));
    assert_eq!(count(&path, "work_order_intake_orders"), 0);
    drop(denied);
    let store = open(&path, Arc::new(Authority::default()));
    let result = store.record_intake(r.clone()).expect("record");
    let replay = store.record_intake(r.clone()).expect("lost ACK replay");
    assert!(!result.replayed && replay.replayed);
    assert_eq!(result.receipt, replay.receipt);
    assert_eq!(result.receipt_digest, replay.receipt_digest);
    assert_eq!(
        serde_json::to_vec(&result.receipt).expect("receipt"),
        serde_json::to_vec(&replay.receipt).expect("receipt")
    );
    assert_eq!(count(&path, "work_order_intake_orders"), 1);
    assert_eq!(count(&path, "domain_events"), 0);
    assert_eq!(count(&path, "outbox_intents"), 0);
    let got = store
        .get_work_order(scope(), "wo".into(), None)
        .expect("read")
        .expect("present");
    assert_eq!(got.work_order, r.work_order);
    assert_eq!(got.receipt, result.receipt);
}
#[test]
fn late_fault_rolls_back_plan_tasks_source_order_bindings_and_receipt() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("wo", "source", "task", 0);
    let injection = Connection::open(&path).expect("fault fixture");
    injection.execute_batch("CREATE TRIGGER wo2_late_fault BEFORE INSERT ON work_order_intake_bindings BEGIN SELECT RAISE(ABORT,'late binding fault'); END;").expect("trigger");
    assert!(store.record_intake(r.clone()).is_err());
    for table in [
        "work_order_intake_orders",
        "work_order_intake_revisions",
        "work_order_intake_sources",
        "work_order_intake_bindings",
        "workplan_registry_plans",
        "workplan_registry_tasks",
    ] {
        assert_eq!(count(&path, table), 0, "{table}");
    }
    injection
        .execute_batch("DROP TRIGGER wo2_late_fault")
        .expect("remove fault");
    store
        .record_intake(r)
        .expect("same unaccepted command retry");
    assert_eq!(count(&path, "work_order_intake_bindings"), 1);
}
#[test]
fn plan_cas_preserves_prior_tasks_and_historical_reply_ignores_new_heads() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: ProjectId::new("p").expect("project"),
        tasks: vec![task("prior")],
    };
    let source = serde_json::to_vec(&plan).expect("plan");
    let p = preview_workplan(&source).expect("plan preview");
    store
        .register_workplan(RegisterWorkplanRequest {
            command_id: "standalone-plan".into(),
            project_id: plan.project_id.clone(),
            plan_id: plan.plan_id.clone(),
            expected_current_revision: "0".into(),
            source,
            source_digest: p.source_digest,
            canonical_digest: content_digest(&plan).expect("digest"),
        })
        .expect("prior plan");
    let first = request("one", "source-one", "new-one", 1);
    let receipt = store.record_intake(first.clone()).expect("append one");
    let second = request("two", "source-two", "new-two", 2);
    store.record_intake(second).expect("append two");
    let got = store
        .workplan(ProjectId::new("p").expect("project"), "plan".into(), None)
        .expect("plan")
        .expect("present");
    assert_eq!(got.plan.tasks.len(), 3);
    assert!(got.plan.tasks.iter().any(|t| t.task_id.as_str() == "prior"));
    assert_eq!(
        store
            .record_intake(first)
            .expect("historical replay")
            .receipt,
        receipt.receipt
    );
    let stale = request("stale", "source-stale", "new-stale", 1);
    assert!(matches!(
        store.record_intake(stale),
        Err(StoreError::Intake(IntakeStoreError::StaleCurrent))
    ));
    assert_eq!(count(&path, "work_order_intake_orders"), 2);
}
#[test]
fn command_source_collisions_are_distinct_from_intentionally_identical_content() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let first = request("one", "stable-source", "task", 0);
    store.record_intake(first.clone()).expect("first");
    let mut collision = request("different", "different-source", "other-task", 1);
    collision.command_id = first.command_id.clone();
    assert!(matches!(
        store.record_intake(collision),
        Err(StoreError::Intake(IntakeStoreError::Collision))
    ));
    let same_source = request("different", "stable-source", "other-task", 1);
    assert!(matches!(
        store.record_intake(same_source),
        Err(StoreError::Intake(IntakeStoreError::Collision))
    ));
    let intentional = request("intentional", "new-source", "task", 1);
    assert_eq!(
        first.work_order.raw_digest,
        intentional.work_order.raw_digest
    );
    store
        .record_intake(intentional)
        .expect("explicit new source ID is intentional");
    assert_eq!(count(&path, "work_order_intake_orders"), 2);
}
#[test]
fn current_authority_policy_and_read_scope_are_rechecked_without_public_snapshot_grants() {
    let d = private();
    let path = d.path().join("store.db");
    let authority = Arc::new(Authority::default());
    let store = open(&path, authority.clone());
    let r = request("wo", "source", "task", 0);
    store.record_intake(r.clone()).expect("record");
    authority.generation.store(8, Ordering::SeqCst);
    assert!(matches!(
        store.record_intake(r.clone()),
        Err(StoreError::Intake(IntakeStoreError::Denied))
    ));
    authority.generation.store(7, Ordering::SeqCst);
    authority.cap.store(MAX_SOURCE_BYTES - 1, Ordering::SeqCst);
    assert!(store.record_intake(r.clone()).is_err());
    authority.cap.store(MAX_SOURCE_BYTES, Ordering::SeqCst);
    authority.denied.store(true, Ordering::SeqCst);
    assert!(matches!(
        store.record_intake(r.clone()),
        Err(StoreError::Intake(IntakeStoreError::Denied))
    ));
    for n in 0..4 {
        let mut foreign = scope();
        match n {
            0 => foreign.project_id = Some(ProjectId::new("foreign").expect("project")),
            1 => foreign.root_id = Some(RootId::new("foreign").expect("root")),
            2 => foreign.principal = PrincipalId::new("foreign").expect("principal"),
            _ => foreign.authority_generation = 8,
        };
        assert!(
            store
                .get_work_order(foreign.clone(), "wo".into(), None)
                .expect("scoped read")
                .is_none()
        );
        let q = query(&r, 10);
        let found = store.find_duplicate_candidates(foreign, q);
        if n == 0 {
            assert!(found.is_err());
        } else {
            assert!(found.expect("scoped duplicate read").candidates.is_empty());
        }
    }
    assert_eq!(count(&path, "work_order_intake_revisions"), 1);
}
#[test]
fn artifact_claim_plaintext_and_changed_selection_fail_without_partial_records() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("wo", "source", "task", 0);
    let mut artifact = r.clone();
    artifact.work_order.source_payload = SourcePayload::ArtifactClaim {
        raw_digest: artifact.work_order.raw_digest.clone(),
        size_bytes: 100,
        artifact_ref: "artifact:object".into(),
        pin_receipt_ref: "pin:claim".into(),
    };
    assert!(matches!(
        store.record_intake(artifact),
        Err(StoreError::Intake(IntakeStoreError::UnsupportedSource))
    ));
    let mut changed = r.clone();
    changed.selected_tasks[0].binding.outcome_digest =
        content_digest(&"different outcome").expect("digest");
    assert!(matches!(
        store.record_intake(changed),
        Err(StoreError::Intake(IntakeStoreError::Selection))
    ));
    let mut plain = r.clone();
    let p = preview_intake(
        b"Do not infer criteria",
        IntakeFormat::PlainText,
        &plain.work_order.provenance,
        &scope(),
        &IntakePolicy::default(),
    )
    .expect("plain preview");
    plain.work_order.format = IntakeFormat::PlainText;
    plain.work_order.source_payload = SourcePayload::InlineUtf8 {
        text: "Do not infer criteria".into(),
    };
    plain.work_order.raw_digest = p.raw_digest;
    plain.work_order.canonical_digest = p.canonical_digest;
    plain.preview_manifest_digest = p.manifest_digest;
    plain.selected_tasks[0].binding.source_spans = vec![SourceSpan { start: 0, end: 20 }];
    assert!(store.record_intake(plain).is_err());
    assert_eq!(count(&path, "work_order_intake_orders"), 0);
}
#[test]
fn bounded_duplicate_history_keeps_pagination_and_actual_lifecycle_separate() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let first = request("a", "source-a", "task", 0);
    let second = request("b", "source-b", "task", 1);
    store.record_intake(first.clone()).expect("first");
    store.record_intake(second).expect("second");
    store
        .append(
            SourceIdentity {
                producer_id: "fixture".into(),
                incarnation: "test".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: task("task"),
            },
            "2026-10-19T00:00:00Z".into(),
        )
        .expect("actual domain submission");
    let mut q = query(&first, 1);
    let page = store
        .find_duplicate_candidates(scope(), q.clone())
        .expect("first page");
    assert_eq!(page.candidates.len(), 1);
    assert_eq!(page.candidates[0].lifecycle, "ready");
    assert_eq!(
        page.candidates[0].reason,
        DuplicateReason::AdvisoryRawContent
    );
    q.cursor = match page.coverage {
        DuplicateCoverage::Partial { continuation } => Some(continuation),
        _ => panic!("bounded partial required"),
    };
    let next = store.find_duplicate_candidates(scope(), q).expect("next");
    assert_eq!(next.candidates.len(), 1);
    assert!(matches!(
        next.coverage,
        DuplicateCoverage::Incomplete { .. }
    ));
    let mut bad = query(&first, 1);
    bad.cursor = Some("../../foreign".into());
    assert!(store.find_duplicate_candidates(scope(), bad).is_err());
}
#[test]
fn reopen_rebuild_backup_preserve_history_and_never_install_an_issuer() {
    let d = private();
    let path = d.path().join("store.db");
    let authority = Arc::new(Authority::default());
    let store = open(&path, authority.clone());
    let r = request("wo", "source", "task", 0);
    let receipt = store.record_intake(r.clone()).expect("record");
    store.rebuild().expect("rebuild");
    let backup = d.path().join("backup.db");
    store.backup(&backup).expect("backup");
    drop(store);
    let reopened = Store::open(&path).expect("reopen consistency without issuer");
    assert!(matches!(
        reopened.record_intake(r.clone()),
        Err(StoreError::Intake(IntakeStoreError::Denied))
    ));
    assert_eq!(
        reopened
            .get_work_order(scope(), "wo".into(), None)
            .expect("read")
            .expect("present")
            .receipt,
        receipt.receipt
    );
    drop(reopened);
    let original = open(&path, authority);
    assert_eq!(
        original.record_intake(r).expect("replayed").receipt,
        receipt.receipt
    );
    let restored = Store::open(&backup).expect("backup open");
    assert_eq!(
        restored
            .get_work_order(scope(), "wo".into(), None)
            .expect("read backup")
            .expect("present")
            .receipt,
        receipt.receipt
    );
}
#[test]
fn corrupted_lookup_metadata_and_binding_fail_rebuild_backup_and_open() {
    for corruption in ["command", "binding", "scope"] {
        let d = private();
        let path = d.path().join("store.db");
        let store = open(&path, Arc::new(Authority::default()));
        store
            .record_intake(request("wo", "source", "task", 0))
            .expect("record");
        let injection = Connection::open(&path).expect("fixture fault");
        let sql = match corruption {
            "command" => {
                "DROP TRIGGER work_order_intake_revision_update;UPDATE work_order_intake_revisions SET command_id='forged-command';"
            }
            "scope" => {
                "DROP TRIGGER work_order_intake_revision_update;UPDATE work_order_intake_revisions SET principal_id='forged-principal';"
            }
            _ => {
                "DROP TRIGGER work_order_intake_binding_update;UPDATE work_order_intake_bindings SET binding_json=json_set(binding_json,'$.outcome_id','forged-outcome');"
            }
        };
        injection.execute_batch(sql).expect("corrupt fixture");
        assert!(store.rebuild().is_err());
        let backup = d.path().join("not-published.db");
        assert!(store.backup(&backup).is_err());
        assert!(!backup.exists());
        drop(store);
        drop(injection);
        assert!(Store::open(&path).is_err());
    }
}

#[test]
fn racing_identical_commands_replay_once_and_competing_plan_cas_has_one_winner() {
    let d = private();
    let path = d.path().join("same.db");
    let store = open(&path, Arc::new(Authority::default()));
    let r = request("wo", "source", "task", 0);
    let results = std::thread::scope(|threads| {
        let left = threads.spawn(|| store.record_intake(r.clone()));
        let right = threads.spawn(|| store.record_intake(r.clone()));
        [left.join().expect("left"), right.join().expect("right")]
    });
    let receipts: Vec<_> = results
        .into_iter()
        .map(|r| r.expect("record/replay"))
        .collect();
    assert_eq!(receipts[0].receipt, receipts[1].receipt);
    assert_eq!(receipts.iter().filter(|r| r.replayed).count(), 1);
    assert_eq!(count(&path, "work_order_intake_revisions"), 1);
    let path = d.path().join("different.db");
    let store = open(&path, Arc::new(Authority::default()));
    let left = request("left", "source-left", "task-left", 0);
    let right = request("right", "source-right", "task-right", 0);
    let results = std::thread::scope(|threads| {
        let left = threads.spawn(|| store.record_intake(left));
        let right = threads.spawn(|| store.record_intake(right));
        [left.join().expect("left"), right.join().expect("right")]
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Intake(IntakeStoreError::StaleCurrent))))
            .count(),
        1
    );
    assert_eq!(count(&path, "work_order_intake_revisions"), 1);
}

#[test]
fn work_order_revisions_advance_by_cas_without_rewriting_original_source_or_receipt() {
    let d = private();
    let path = d.path().join("store.db");
    let store = open(&path, Arc::new(Authority::default()));
    let first = request("wo", "source", "task", 0);
    let original = store.record_intake(first.clone()).expect("first");
    let mut revised = first.clone();
    revised.command_id = CommandId::new("revision-two").expect("command");
    revised.expected_work_order_revision = 1;
    revised.work_order.revision = 2;
    revised.expected_plan_revision = 1;
    revised.selected_tasks[0].binding.work_order_revision = 2;
    let second = store
        .record_intake(revised)
        .expect("explicit next revision");
    assert_eq!(second.receipt.work_order_revision, 2);
    let current = store
        .get_work_order(scope(), "wo".into(), None)
        .expect("current")
        .expect("present");
    assert_eq!(current.work_order.revision, 2);
    let old = store
        .get_work_order(scope(), "wo".into(), Some(1))
        .expect("historical")
        .expect("present");
    assert_eq!(old.receipt, original.receipt);
    assert_eq!(
        store
            .record_intake(first.clone())
            .expect("historical retry")
            .receipt,
        original.receipt
    );
    let mut stale = first;
    stale.command_id = CommandId::new("stale-work-order-cas").expect("command");
    assert!(matches!(
        store.record_intake(stale),
        Err(StoreError::Intake(IntakeStoreError::StaleCurrent))
    ));
    assert_eq!(count(&path, "work_order_intake_sources"), 1);
    assert_eq!(count(&path, "work_order_intake_revisions"), 2);
}
