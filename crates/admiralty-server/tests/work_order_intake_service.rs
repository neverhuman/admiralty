#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    AcceptanceCriterion, CheckSpec, CommandId, DomainEvent, PrincipalId, ProjectId, RootId,
    SourceIdentity, TaskContract, TaskId, content_digest,
    intake::{
        DeliveryRequirement, DeliveryStage, DeliveryTarget, DeliveryTargetKind, DuplicateQuery,
        DuplicateReason, INTAKE_VERSION, IntakeBatch, IntakeFormat, IntakeItem, IntakePolicy,
        OriginReference, OutcomeInput, RecordIntakeRequest, RequesterAttribution, SelectedTask,
        SourcePayload, SourceProvenance, SourceSpan, TargetRequirement, UnresolvedFact,
        WorkOrderRevision, WorkOrderTaskBinding,
    },
};
use admiralty_server::services::work_order_intake::{
    AuthenticatedIntakeActor, AuthorizationStage, IntakeScopeGrant, MonotonicClock,
    PreviewIntakeInput, WorkOrderIntakeAction, WorkOrderIntakeAdmissionBridge,
    WorkOrderIntakeAuthRegistry, WorkOrderIntakeBinding, WorkOrderIntakeCredential,
    WorkOrderIntakeError, WorkOrderIntakePolicy, WorkOrderIntakeService,
};
use admiralty_store::{
    Store, StoreAuthorities, StoreError,
    intake::IntakeStoreError,
    intake_capture::{CaptureError, CaptureIntakeRequest},
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

const OPERATOR_TOKEN: &str = "wo-service-operator-token-0123456789abcdef";
const READER_TOKEN: &str = "wo-service-readonly-token-fedcba9876543210";
const FOREIGN_TOKEN: &str = "wo-service-foreign-token-1122334455667788";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("wo-intake-service-{}", Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("create private 0700 test directory");
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestClock {
    now: AtomicU64,
    auto_step: AtomicU64,
}

impl TestClock {
    fn new(initial: u64) -> Self {
        Self {
            now: AtomicU64::new(initial),
            auto_step: AtomicU64::new(0),
        }
    }

    fn set_auto_step(&self, step: u64) {
        self.auto_step.store(step, Ordering::SeqCst);
    }
}

impl MonotonicClock for TestClock {
    fn now_nanos(&self) -> u64 {
        let step = self.auto_step.load(Ordering::SeqCst);
        self.now.fetch_add(step, Ordering::SeqCst)
    }
}

type StageHook = Box<dyn Fn(AuthorizationStage) + Send + Sync>;

struct ProgrammablePolicy {
    allow_pre_queue: AtomicBool,
    allow_writer_turn: AtomicBool,
    pre_queue_calls: AtomicU64,
    writer_turn_calls: AtomicU64,
    on_stage: Mutex<Option<StageHook>>,
}

impl ProgrammablePolicy {
    fn new() -> Self {
        Self {
            allow_pre_queue: AtomicBool::new(true),
            allow_writer_turn: AtomicBool::new(true),
            pre_queue_calls: AtomicU64::new(0),
            writer_turn_calls: AtomicU64::new(0),
            on_stage: Mutex::new(None),
        }
    }

    fn set_stage_hook(&self, hook: impl Fn(AuthorizationStage) + Send + Sync + 'static) {
        *self.on_stage.lock().expect("stage hook lock") = Some(Box::new(hook));
    }

    fn clear_stage_hook(&self) {
        *self.on_stage.lock().expect("stage hook lock") = None;
    }
}

impl WorkOrderIntakePolicy for ProgrammablePolicy {
    fn authorize_current(&self, binding: &WorkOrderIntakeBinding<'_>) -> bool {
        if let Some(hook) = self.on_stage.lock().expect("stage hook lock").as_ref() {
            hook(binding.stage);
        }
        match binding.stage {
            AuthorizationStage::ServicePreQueue => {
                self.pre_queue_calls.fetch_add(1, Ordering::SeqCst);
                self.allow_pre_queue.load(Ordering::SeqCst)
            }
            AuthorizationStage::WriterTurn => {
                self.writer_turn_calls.fetch_add(1, Ordering::SeqCst);
                self.allow_writer_turn.load(Ordering::SeqCst)
            }
        }
    }
}

struct ServiceFixture {
    _dir: TestDir,
    store: Store,
    registry: Arc<WorkOrderIntakeAuthRegistry>,
    policy: Arc<ProgrammablePolicy>,
    clock: Arc<TestClock>,
    service: WorkOrderIntakeService,
    operator: AuthenticatedIntakeActor,
    reader: AuthenticatedIntakeActor,
    foreign: AuthenticatedIntakeActor,
    project_id: ProjectId,
    root_id: RootId,
}

fn all_actions() -> BTreeSet<WorkOrderIntakeAction> {
    BTreeSet::from([
        WorkOrderIntakeAction::PreviewRaw,
        WorkOrderIntakeAction::CaptureRaw,
        WorkOrderIntakeAction::RecordEnriched,
        WorkOrderIntakeAction::ReadCaptured,
        WorkOrderIntakeAction::ReadWorkOrder,
        WorkOrderIntakeAction::QueryDuplicates,
    ])
}

fn read_only_actions() -> BTreeSet<WorkOrderIntakeAction> {
    BTreeSet::from([
        WorkOrderIntakeAction::PreviewRaw,
        WorkOrderIntakeAction::ReadCaptured,
        WorkOrderIntakeAction::ReadWorkOrder,
        WorkOrderIntakeAction::QueryDuplicates,
    ])
}

fn build_fixture() -> ServiceFixture {
    let dir = TestDir::new();
    let project_id = ProjectId::new("project-alpha").expect("project");
    let root_id = RootId::new("root-alpha").expect("root");
    let foreign_project = ProjectId::new("project-beta").expect("foreign project");
    let foreign_root = RootId::new("root-beta").expect("foreign root");

    let operator_principal = PrincipalId::new("intake-operator").expect("operator principal");
    let reader_principal = PrincipalId::new("intake-reader").expect("reader principal");
    let foreign_principal = PrincipalId::new("foreign-operator").expect("foreign principal");

    let registry = Arc::new(
        WorkOrderIntakeAuthRegistry::new(vec![
            WorkOrderIntakeCredential::configured(
                "cred-operator".into(),
                OPERATOR_TOKEN.into(),
                operator_principal,
                vec![IntakeScopeGrant {
                    project_id: project_id.clone(),
                    root_id: root_id.clone(),
                    authority_ref: "grant:intake-operator".into(),
                    authority_generation: 1,
                    policy: IntakePolicy::default(),
                    allowed_actions: all_actions(),
                }],
            ),
            WorkOrderIntakeCredential::configured(
                "cred-reader".into(),
                READER_TOKEN.into(),
                reader_principal,
                vec![IntakeScopeGrant {
                    project_id: project_id.clone(),
                    root_id: root_id.clone(),
                    authority_ref: "grant:intake-reader".into(),
                    authority_generation: 1,
                    policy: IntakePolicy::default(),
                    allowed_actions: read_only_actions(),
                }],
            ),
            WorkOrderIntakeCredential::configured(
                "cred-foreign".into(),
                FOREIGN_TOKEN.into(),
                foreign_principal,
                vec![IntakeScopeGrant {
                    project_id: foreign_project,
                    root_id: foreign_root,
                    authority_ref: "grant:foreign-operator".into(),
                    authority_generation: 1,
                    policy: IntakePolicy::default(),
                    allowed_actions: all_actions(),
                }],
            ),
        ])
        .expect("valid intake auth registry"),
    );

    let policy = Arc::new(ProgrammablePolicy::new());
    let clock = Arc::new(TestClock::new(1_000));
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::with_clock_and_limits(
        Arc::clone(&registry),
        Some(policy.clone() as Arc<dyn WorkOrderIntakePolicy>),
        clock.clone(),
        Duration::from_secs(30),
        8,
    ));

    let store = Store::open_with_startup_authorities(
        dir.0.join("intake-service.sqlite"),
        StoreAuthorities {
            lifecycle: None,
            intake: Some(bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
            intake_capture: Some(
                bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
            ),
            questions: None,
        },
    )
    .expect("open store with startup intake authorities");

    let operator = registry
        .authenticate(OPERATOR_TOKEN)
        .expect("authenticate operator");
    let reader = registry
        .authenticate(READER_TOKEN)
        .expect("authenticate reader");
    let foreign = registry
        .authenticate(FOREIGN_TOKEN)
        .expect("authenticate foreign");

    let service = WorkOrderIntakeService::new(store.clone(), bridge);

    ServiceFixture {
        _dir: dir,
        store,
        registry,
        policy,
        clock,
        service,
        operator,
        reader,
        foreign,
        project_id,
        root_id,
    }
}

fn sample_provenance(
    source_id: &str,
    revision: &str,
    requester: RequesterAttribution,
) -> SourceProvenance {
    SourceProvenance {
        schema_version: INTAKE_VERSION,
        source_id: source_id.into(),
        revision: revision.into(),
        captured_at: "2026-10-19T04:00:00Z".into(),
        requester,
        origin: Some(OriginReference {
            system: "ticket-tracker".into(),
            record_id: format!("ticket-{source_id}"),
            revision: revision.into(),
        }),
    }
}

fn sample_task(
    project_id: &ProjectId,
    root_id: &RootId,
    task_id: &str,
    title: &str,
) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: project_id.clone(),
        root_id: root_id.clone(),
        task_id: TaskId::new(task_id).expect("task id"),
        revision: 1,
        title: title.into(),
        objective: format!("Complete {title} safely"),
        non_goals: vec!["No autonomous provider side effects".into()],
        owned_paths: vec!["crates/admiralty-server/src/services/work_order_intake.rs".into()],
        forbidden_paths: vec!["crates/admiralty-store".into()],
        read_first: vec!["docs/plans/admiralty-v2/REMAINING_20261019.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "c1".into(),
            text: "Unit check passes".into(),
            check_ids: vec!["chk1".into()],
        }],
        checks: vec![CheckSpec {
            id: "chk1".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }],
        authority_ref: "grant:intake-operator".into(),
        max_attempts: 1,
        simulated: false,
    }
}

fn sample_delivery(task: &TaskContract) -> DeliveryRequirement {
    DeliveryRequirement {
        schema_version: INTAKE_VERSION,
        task_id: task.task_id.clone(),
        contract_revision: task.revision,
        contract_digest: content_digest(task).expect("task digest"),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "admiralty-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    }
}

#[allow(clippy::too_many_arguments)]
fn build_capture_request(
    f: &ServiceFixture,
    command_id: &str,
    capture_id: &str,
    revision: u32,
    expected_revision: u32,
    parent_receipt_digest: Option<String>,
    text: &str,
    source_id: &str,
    source_rev: &str,
    requester: RequesterAttribution,
) -> CaptureIntakeRequest {
    let provenance = sample_provenance(source_id, source_rev, requester);
    let payload = SourcePayload::InlineUtf8 { text: text.into() };
    let preview = f
        .service
        .preview_raw(
            &f.operator,
            &PreviewIntakeInput {
                project_id: f.project_id.clone(),
                root_id: f.root_id.clone(),
                format: IntakeFormat::PlainText,
                provenance: provenance.clone(),
                source_payload: payload.clone(),
            },
        )
        .expect("preview raw input");
    CaptureIntakeRequest {
        schema_version: INTAKE_VERSION,
        command_id: CommandId::new(command_id).expect("command id"),
        capture_id: capture_id.into(),
        revision,
        expected_revision,
        parent_receipt_digest,
        scope: preview.authorized_scope,
        format: IntakeFormat::PlainText,
        provenance,
        source_payload: payload,
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_enriched_record_request(
    f: &ServiceFixture,
    command_id: &str,
    work_order_id: &str,
    wo_revision: u32,
    expected_wo_revision: u32,
    plan_id: &str,
    expected_plan_revision: u32,
    task_id: &str,
    source_id: &str,
    source_rev: &str,
    requester: RequesterAttribution,
) -> RecordIntakeRequest {
    let task = sample_task(
        &f.project_id,
        &f.root_id,
        task_id,
        &format!("Task {task_id}"),
    );
    let delivery = sample_delivery(&task);
    let item_text = format!("Enriched request for {task_id}");

    // First serialize with a placeholder span inside the JSON item, then compute exact raw span.
    let mut batch = IntakeBatch {
        schema_version: INTAKE_VERSION,
        items: vec![IntakeItem {
            text: item_text.clone(),
            title: Some(format!("Work order {work_order_id} r{wo_revision}")),
            rationale: Some("Operator reviewed and enriched".into()),
            requested_timing: Some("immediate".into()),
            constraints: vec!["strict-writer-policy".into()],
            claimed_project_id: Some(f.project_id.clone()),
            outcomes: vec![OutcomeInput {
                id: "outcome-1".into(),
                description: format!("Deliver {task_id}"),
                source_spans: vec![SourceSpan { start: 10, end: 20 }],
                constraints: vec![],
                task: Some(task.clone()),
                delivery: Some(delivery.clone()),
            }],
        }],
    };
    let prelim = serde_json::to_string(&batch).expect("prelim batch json");
    let text_pos = prelim
        .find(&item_text)
        .expect("locate item_text in raw json") as u32;
    batch.items[0].outcomes[0].source_spans = vec![SourceSpan {
        start: text_pos,
        end: text_pos + item_text.len() as u32,
    }];
    let raw_json = serde_json::to_string(&batch).expect("final batch json");

    let provenance = sample_provenance(source_id, source_rev, requester);
    let payload = SourcePayload::InlineUtf8 { text: raw_json };
    let preview = f
        .service
        .preview_raw(
            &f.operator,
            &PreviewIntakeInput {
                project_id: f.project_id.clone(),
                root_id: f.root_id.clone(),
                format: IntakeFormat::JsonBatch,
                provenance: provenance.clone(),
                source_payload: payload.clone(),
            },
        )
        .expect("preview enriched json batch");

    let item_preview = &preview.requests[0];
    assert!(
        item_preview
            .unresolved
            .iter()
            .all(|u| matches!(u, UnresolvedFact::AttributionUnknown)),
        "enriched item must have no structural unresolved facts: {:?}",
        item_preview.unresolved
    );
    let outcome_preview = &item_preview.outcomes[0];
    assert!(
        outcome_preview.unresolved.is_empty(),
        "enriched outcome must have no unresolved facts: {:?}",
        outcome_preview.unresolved
    );

    let contract_digest = outcome_preview
        .contract_digest
        .clone()
        .expect("contract digest");

    RecordIntakeRequest {
        schema_version: INTAKE_VERSION,
        command_id: CommandId::new(command_id).expect("command id"),
        expected_work_order_revision: expected_wo_revision,
        plan_id: plan_id.into(),
        expected_plan_revision,
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
        authority_generation: preview.authorized_scope.authority_generation,
        work_order: WorkOrderRevision {
            schema_version: INTAKE_VERSION,
            id: work_order_id.into(),
            revision: wo_revision,
            scope: preview.authorized_scope,
            provenance,
            title: item_preview.title.clone(),
            outcome: Some(outcome_preview.outcome.description.clone()),
            rationale: item_preview.rationale.clone(),
            constraints: item_preview.constraints.clone(),
            requested_timing: item_preview.requested_timing.clone(),
            raw_digest: preview.raw_digest,
            canonical_digest: preview.canonical_digest,
            parser_revision: preview.parser_revision,
            format: IntakeFormat::JsonBatch,
            source_payload: payload,
        },
        selected_tasks: vec![SelectedTask {
            binding: WorkOrderTaskBinding {
                work_order_id: work_order_id.into(),
                work_order_revision: wo_revision,
                request_index: 0,
                outcome_id: outcome_preview.outcome.id.clone(),
                outcome_digest: outcome_preview.outcome_digest.clone(),
                source_spans: outcome_preview.outcome.source_spans.clone(),
                task_id: task.task_id.clone(),
                contract_revision: task.revision,
                contract_digest,
            },
            task,
            delivery,
        }],
    }
}

#[test]
fn default_deny_and_read_only_grants_reject_capture_and_record_and_direct_store_bypass() {
    let f = build_fixture();
    let capture_req = build_capture_request(
        &f,
        "cmd-cap-1",
        "cap-1",
        1,
        0,
        None,
        "Raw operator request text",
        "src-1",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    let record_req = build_enriched_record_request(
        &f,
        "cmd-rec-1",
        "wo-1",
        1,
        0,
        "plan-1",
        0,
        "task-1",
        "src-rec-1",
        "rev-1",
        RequesterAttribution::Claimed {
            requester: "alice".into(),
        },
    );

    // 1. Default-deny service (policy = None) rejects every operation.
    let no_policy_bridge = Arc::new(WorkOrderIntakeAdmissionBridge::new(
        Arc::clone(&f.registry),
        None,
    ));
    let default_deny_service = WorkOrderIntakeService::new(f.store.clone(), no_policy_bridge);
    assert_eq!(
        default_deny_service.capture_raw(&f.operator, capture_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(
        default_deny_service.record_enriched(&f.operator, record_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(
        default_deny_service.get_captured_intake(
            &f.operator,
            &f.project_id,
            &f.root_id,
            "cap-1",
            None
        ),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(
        default_deny_service.get_work_order(&f.operator, &f.project_id, &f.root_id, "wo-1", None),
        Err(WorkOrderIntakeError::AccessDenied)
    );

    // 2. Read-only credential cannot authorize capture_raw or record_enriched.
    assert_eq!(
        f.service.capture_raw(&f.reader, capture_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(
        f.service.record_enriched(&f.reader, record_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );

    // 3. Direct Store calls bypassing WorkOrderIntakeService have no inflight ticket and fail closed at the writer.
    assert!(matches!(
        f.store.capture_intake(capture_req),
        Err(StoreError::IntakeCapture(CaptureError::Denied))
    ));
    assert!(matches!(
        f.store.record_intake(record_req),
        Err(StoreError::Intake(IntakeStoreError::Denied))
    ));
}

#[test]
fn writer_turn_revocation_generation_and_policy_rotation_deny_queued_and_retried_commands() {
    let f = build_fixture();
    let capture_req = build_capture_request(
        &f,
        "cmd-cap-writer-deny",
        "cap-writer",
        1,
        0,
        None,
        "Raw request testing writer-turn policy denial",
        "src-writer",
        "rev-1",
        RequesterAttribution::Claimed {
            requester: "bob".into(),
        },
    );

    // Case A: Policy allows ServicePreQueue, then flips to deny before WriterTurn executes.
    f.policy.allow_pre_queue.store(true, Ordering::SeqCst);
    f.policy.allow_writer_turn.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service.capture_raw(&f.operator, capture_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(f.policy.pre_queue_calls.load(Ordering::SeqCst), 2); // 1 preview + 1 capture pre-queue
    assert_eq!(f.policy.writer_turn_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.service.admission_bridge().active_inflight_count(), Ok(0));

    f.policy.allow_writer_turn.store(true, Ordering::SeqCst);
    assert_eq!(
        f.service
            .get_captured_intake(&f.operator, &f.project_id, &f.root_id, "cap-writer", None)
            .expect("read after writer denial"),
        None
    );

    // Case B: Authority generation rotates between ServicePreQueue and WriterTurn.
    let reg_for_hook = Arc::clone(&f.registry);
    let proj_for_hook = f.project_id.clone();
    let root_for_hook = f.root_id.clone();
    f.policy.set_stage_hook(move |stage| {
        if stage == AuthorizationStage::ServicePreQueue {
            let _ = reg_for_hook.update_grant(
                "cred-operator",
                &proj_for_hook,
                &root_for_hook,
                2,
                IntakePolicy::default(),
            );
        }
    });
    assert_eq!(
        f.service.capture_raw(&f.operator, capture_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    f.policy.clear_stage_hook();
    // Restore generation 1 and verify nothing was written.
    f.registry
        .update_grant(
            "cred-operator",
            &f.project_id,
            &f.root_id,
            1,
            IntakePolicy::default(),
        )
        .expect("restore generation 1");
    assert_eq!(
        f.service
            .get_captured_intake(&f.operator, &f.project_id, &f.root_id, "cap-writer", None)
            .expect("read after generation rotation denial"),
        None
    );

    // Case C: Record enriched succeeds once, then credential revocation between ServicePreQueue
    // and WriterTurn denies historical retry on the Store writer thread.
    let record_req = build_enriched_record_request(
        &f,
        "cmd-rec-writer",
        "wo-writer",
        1,
        0,
        "plan-writer",
        0,
        "task-writer",
        "src-rec-writer",
        "rev-1",
        RequesterAttribution::Claimed {
            requester: "carol".into(),
        },
    );
    let first = f
        .service
        .record_enriched(&f.operator, record_req.clone())
        .expect("initial enriched record");
    assert!(!first.replayed);

    let reg_for_revoke = Arc::clone(&f.registry);
    f.policy.set_stage_hook(move |stage| {
        if stage == AuthorizationStage::ServicePreQueue {
            let _ = reg_for_revoke.revoke("cred-operator");
        }
    });
    assert_eq!(
        f.service.record_enriched(&f.operator, record_req),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    f.policy.clear_stage_hook();
}

#[test]
fn distinct_authenticated_actor_vs_claimed_requester_and_cross_project_isolation() {
    let f = build_fixture();
    let claimed_requester = RequesterAttribution::Claimed {
        requester: "external-product-manager".into(),
    };
    let capture_req = build_capture_request(
        &f,
        "cmd-distinct-cap",
        "cap-distinct",
        1,
        0,
        None,
        "Customer reported issue #99",
        "src-distinct",
        "rev-1",
        claimed_requester.clone(),
    );
    let cap_res = f
        .service
        .capture_raw(&f.operator, capture_req)
        .expect("capture with distinct requester");
    assert!(!cap_res.replayed);

    let stored_cap = f
        .service
        .get_captured_intake(&f.operator, &f.project_id, &f.root_id, "cap-distinct", None)
        .expect("read captured")
        .expect("captured exists");
    assert_eq!(
        stored_cap.request.scope.principal.as_str(),
        "intake-operator"
    );
    assert_eq!(stored_cap.request.provenance.requester, claimed_requester);

    // Foreign project credential cannot read or query project-alpha.
    assert_eq!(
        f.service
            .get_captured_intake(&f.foreign, &f.project_id, &f.root_id, "cap-distinct", None),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    assert_eq!(
        f.service.find_duplicate_candidates(
            &f.foreign,
            &f.project_id,
            &f.root_id,
            DuplicateQuery {
                schema_version: INTAKE_VERSION,
                project_id: f.project_id.clone(),
                raw_digest: stored_cap.receipt.raw_digest.clone(),
                canonical_digest: stored_cap.receipt.canonical_digest.clone(),
                origin: None,
                command_id: None,
                cursor: None,
                limit: 10,
            }
        ),
        Err(WorkOrderIntakeError::AccessDenied)
    );
}

#[test]
fn capture_and_record_preserve_exact_historical_replay_after_later_revisions_and_reject_collisions()
{
    let f = build_fixture();

    // 1. Raw capture rev 1, then rev 2, then historical replay of rev 1.
    let cap_r1 = build_capture_request(
        &f,
        "cmd-cap-r1",
        "cap-lineage",
        1,
        0,
        None,
        "Original capture v1 text",
        "src-cap-lineage",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    let res_cap_r1 = f
        .service
        .capture_raw(&f.operator, cap_r1.clone())
        .expect("capture rev 1");
    assert!(!res_cap_r1.replayed);

    let cap_r2 = build_capture_request(
        &f,
        "cmd-cap-r2",
        "cap-lineage",
        2,
        1,
        Some(res_cap_r1.receipt_digest.clone()),
        "Updated capture v2 text",
        "src-cap-lineage",
        "rev-2",
        RequesterAttribution::Unknown {},
    );
    let res_cap_r2 = f
        .service
        .capture_raw(&f.operator, cap_r2.clone())
        .expect("capture rev 2");
    assert!(!res_cap_r2.replayed);

    // Replay rev 1 after rev 2 has been committed.
    let replay_cap_r1 = f
        .service
        .capture_raw(&f.operator, cap_r1.clone())
        .expect("replay capture rev 1 after rev 2");
    assert!(replay_cap_r1.replayed);
    assert_eq!(replay_cap_r1.receipt, res_cap_r1.receipt);
    assert_eq!(replay_cap_r1.receipt_digest, res_cap_r1.receipt_digest);

    // Stale expected_revision fails with StaleCurrent.
    let stale_cap = build_capture_request(
        &f,
        "cmd-cap-stale",
        "cap-lineage",
        2,
        1,
        Some(res_cap_r1.receipt_digest.clone()),
        "Conflicting stale revision",
        "src-cap-lineage",
        "rev-stale",
        RequesterAttribution::Unknown {},
    );
    assert_eq!(
        f.service.capture_raw(&f.operator, stale_cap),
        Err(WorkOrderIntakeError::StaleCurrent)
    );

    // 2. Enriched record rev 1, then rev 2, then historical replay of rev 1.
    let rec_r1 = build_enriched_record_request(
        &f,
        "cmd-wo-r1",
        "wo-lineage",
        1,
        0,
        "plan-lineage",
        0,
        "task-lineage-1",
        "src-wo-lineage",
        "rev-1",
        RequesterAttribution::Claimed {
            requester: "operator".into(),
        },
    );
    let res_wo_r1 = f
        .service
        .record_enriched(&f.operator, rec_r1.clone())
        .expect("record enriched rev 1");
    assert!(!res_wo_r1.replayed);
    assert_eq!(res_wo_r1.receipt.work_order_revision, 1);
    assert_eq!(res_wo_r1.receipt.plan_revision, 1);

    let rec_r2 = build_enriched_record_request(
        &f,
        "cmd-wo-r2",
        "wo-lineage",
        2,
        1,
        "plan-lineage",
        1,
        "task-lineage-2",
        "src-wo-lineage",
        "rev-2",
        RequesterAttribution::Claimed {
            requester: "operator".into(),
        },
    );
    let res_wo_r2 = f
        .service
        .record_enriched(&f.operator, rec_r2)
        .expect("record enriched rev 2");
    assert!(!res_wo_r2.replayed);
    assert_eq!(res_wo_r2.receipt.work_order_revision, 2);
    assert_eq!(res_wo_r2.receipt.plan_revision, 2);

    // Historical replay of rev 1 after rev 2 is head returns original rev 1 receipt!
    let replay_wo_r1 = f
        .service
        .record_enriched(&f.operator, rec_r1.clone())
        .expect("replay enriched rev 1 after rev 2");
    assert!(replay_wo_r1.replayed);
    assert_eq!(replay_wo_r1.receipt, res_wo_r1.receipt);
    assert_eq!(replay_wo_r1.receipt_digest, res_wo_r1.receipt_digest);

    // Same command_id with altered payload fails closed with Collision.
    let mut colliding = build_enriched_record_request(
        &f,
        "cmd-wo-r1",
        "wo-lineage-other",
        1,
        0,
        "plan-lineage",
        2,
        "task-lineage-3",
        "src-wo-other",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    colliding.command_id = CommandId::new("cmd-wo-r1").expect("command id");
    assert_eq!(
        f.service.record_enriched(&f.operator, colliding),
        Err(WorkOrderIntakeError::Collision)
    );

    // Read both historical revision 1 and current head revision 2.
    let wo_rev1 = f
        .service
        .get_work_order(
            &f.operator,
            &f.project_id,
            &f.root_id,
            "wo-lineage",
            Some(1),
        )
        .expect("read rev 1")
        .expect("rev 1 exists");
    assert_eq!(wo_rev1.work_order.revision, 1);

    let wo_head = f
        .service
        .get_work_order(&f.operator, &f.project_id, &f.root_id, "wo-lineage", None)
        .expect("read head")
        .expect("head exists");
    assert_eq!(wo_head.work_order.revision, 2);

    // Duplicate query finds exact command identity.
    let dup_page = f
        .service
        .find_duplicate_candidates(
            &f.operator,
            &f.project_id,
            &f.root_id,
            DuplicateQuery {
                schema_version: INTAKE_VERSION,
                project_id: f.project_id.clone(),
                raw_digest: rec_r1.work_order.raw_digest.clone(),
                canonical_digest: rec_r1.work_order.canonical_digest.clone(),
                origin: rec_r1.work_order.provenance.origin.clone(),
                command_id: Some(rec_r1.command_id.clone()),
                cursor: None,
                limit: 10,
            },
        )
        .expect("duplicate query");
    assert_eq!(dup_page.candidates.len(), 1);
    assert_eq!(
        dup_page.candidates[0].reason,
        DuplicateReason::ExactCommandIdentity
    );
}

#[test]
fn raw_plaintext_remains_unresolved_and_artifact_claim_or_oversized_inputs_fail_closed_with_zero_effects()
 {
    let f = build_fixture();

    // JSON-looking text under PlainText format stays one unresolved request.
    let json_looking_prose = r#"{"schema_version":1,"items":[{"text":"do not split me"}]}"#;
    let preview = f
        .service
        .preview_raw(
            &f.operator,
            &PreviewIntakeInput {
                project_id: f.project_id.clone(),
                root_id: f.root_id.clone(),
                format: IntakeFormat::PlainText,
                provenance: sample_provenance(
                    "src-prose",
                    "rev-1",
                    RequesterAttribution::Unknown {},
                ),
                source_payload: SourcePayload::InlineUtf8 {
                    text: json_looking_prose.into(),
                },
            },
        )
        .expect("preview plain text");
    assert_eq!(preview.requests.len(), 1);
    assert!(preview.requests[0].outcomes.is_empty());
    assert!(
        preview.requests[0]
            .unresolved
            .contains(&UnresolvedFact::OutcomeUnknown)
    );
    assert!(
        preview.requests[0]
            .unresolved
            .contains(&UnresolvedFact::CriteriaUnknown)
    );

    // Attempt to record PlainText as an enriched work order fails with UnresolvedPlainTextEnrichment.
    let mut record_req = build_enriched_record_request(
        &f,
        "cmd-plain-reject",
        "wo-plain",
        1,
        0,
        "plan-plain",
        0,
        "task-plain",
        "src-plain",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    record_req.work_order.format = IntakeFormat::PlainText;
    assert_eq!(
        f.service.record_enriched(&f.operator, record_req.clone()),
        Err(WorkOrderIntakeError::UnresolvedPlainTextEnrichment)
    );

    // ArtifactClaim fails explicitly with ArtifactClaimUnavailable across preview, capture, and record.
    let artifact_claim = SourcePayload::ArtifactClaim {
        raw_digest: preview.raw_digest.clone(),
        size_bytes: 64,
        artifact_ref: "artifact:sha256-claim".into(),
        pin_receipt_ref: "pin:receipt-1".into(),
    };
    assert_eq!(
        f.service.preview_raw(
            &f.operator,
            &PreviewIntakeInput {
                project_id: f.project_id.clone(),
                root_id: f.root_id.clone(),
                format: IntakeFormat::PlainText,
                provenance: sample_provenance("src-art", "rev-1", RequesterAttribution::Unknown {}),
                source_payload: artifact_claim.clone(),
            }
        ),
        Err(WorkOrderIntakeError::ArtifactClaimUnavailable)
    );
    let mut cap_claim = build_capture_request(
        &f,
        "cmd-cap-claim",
        "cap-claim",
        1,
        0,
        None,
        "inline text",
        "src-claim",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    cap_claim.source_payload = artifact_claim.clone();
    assert_eq!(
        f.service.capture_raw(&f.operator, cap_claim),
        Err(WorkOrderIntakeError::ArtifactClaimUnavailable)
    );

    let mut rec_claim = build_enriched_record_request(
        &f,
        "cmd-rec-claim",
        "wo-claim",
        1,
        0,
        "plan-claim",
        0,
        "task-claim",
        "src-rec-claim",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    rec_claim.work_order.source_payload = artifact_claim;
    assert_eq!(
        f.service.record_enriched(&f.operator, rec_claim),
        Err(WorkOrderIntakeError::ArtifactClaimUnavailable)
    );

    // Policy tightening max_source_bytes rejects oversized input with CapacityExceeded.
    let tight_policy = IntakePolicy {
        revision: "wo1-tight".into(),
        max_source_bytes: 32,
        ..IntakePolicy::default()
    };
    f.registry
        .update_grant("cred-operator", &f.project_id, &f.root_id, 1, tight_policy)
        .expect("tighten policy");
    assert_eq!(
        f.service.preview_raw(
            &f.operator,
            &PreviewIntakeInput {
                project_id: f.project_id.clone(),
                root_id: f.root_id.clone(),
                format: IntakeFormat::PlainText,
                provenance: sample_provenance("src-big", "rev-1", RequesterAttribution::Unknown {}),
                source_payload: SourcePayload::InlineUtf8 {
                    text: "This input is longer than thirty-two bytes and must be rejected".into(),
                },
            }
        ),
        Err(WorkOrderIntakeError::CapacityExceeded)
    );

    // Zero task submissions, zero domain events, zero outbox intents.
    let snapshot = f.store.snapshot().expect("store snapshot");
    assert!(snapshot.projection.tasks.is_empty());
    assert!(f.store.events_after(0, 100).expect("events").is_empty());
    assert!(
        f.store
            .claim_outbox_due(Some(0), "test-dispatcher".into(), 30)
            .expect("outbox")
            .is_none()
    );
}

#[test]
fn late_workplan_task_conflict_rolls_back_work_order_and_plan_atomically() {
    let f = build_fixture();
    let existing_task = sample_task(
        &f.project_id,
        &f.root_id,
        "task-conflict",
        "Existing submitted contract",
    );
    f.store
        .append(
            SourceIdentity {
                producer_id: "trusted-fixture".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: existing_task,
            },
            "2026-10-19T04:00:00Z".into(),
        )
        .expect("submit existing task to projection");

    // Build an enriched work order that tries to register a DIFFERENT contract for "task-conflict".
    let conflicting_req = build_enriched_record_request(
        &f,
        "cmd-conflict-rollback",
        "wo-rollback",
        1,
        0,
        "plan-rollback",
        0,
        "task-conflict",
        "src-rollback",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    assert_eq!(
        f.service.record_enriched(&f.operator, conflicting_req),
        Err(WorkOrderIntakeError::Collision)
    );

    // Neither the work order nor the workplan was persisted.
    assert_eq!(
        f.service
            .get_work_order(&f.operator, &f.project_id, &f.root_id, "wo-rollback", None)
            .expect("get rolled-back work order"),
        None
    );
    assert_eq!(
        f.store
            .workplan(f.project_id.clone(), "plan-rollback".into(), None)
            .expect("get rolled-back workplan"),
        None
    );
    assert_eq!(f.service.admission_bridge().active_inflight_count(), Ok(0));
}

#[test]
fn inflight_ttl_expiry_at_writer_turn_and_selection_mismatch_and_store_reopen() {
    let dir = TestDir::new();
    let db_path = dir.0.join("reopen.sqlite");
    let project_id = ProjectId::new("project-alpha").expect("project");
    let root_id = RootId::new("root-alpha").expect("root");
    let operator_principal = PrincipalId::new("intake-operator").expect("operator principal");

    let registry = Arc::new(
        WorkOrderIntakeAuthRegistry::new(vec![WorkOrderIntakeCredential::configured(
            "cred-operator".into(),
            OPERATOR_TOKEN.into(),
            operator_principal,
            vec![IntakeScopeGrant {
                project_id: project_id.clone(),
                root_id: root_id.clone(),
                authority_ref: "grant:intake-operator".into(),
                authority_generation: 1,
                policy: IntakePolicy::default(),
                allowed_actions: all_actions(),
            }],
        )])
        .expect("registry"),
    );
    let policy = Arc::new(ProgrammablePolicy::new());
    let clock = Arc::new(TestClock::new(10_000));
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::with_clock_and_limits(
        Arc::clone(&registry),
        Some(policy.clone() as Arc<dyn WorkOrderIntakePolicy>),
        clock.clone(),
        Duration::from_nanos(500),
        4,
    ));

    let store = Store::open_with_startup_authorities(
        &db_path,
        StoreAuthorities {
            lifecycle: None,
            intake: Some(bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
            intake_capture: Some(
                bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
            ),
            questions: None,
        },
    )
    .expect("open store");
    let operator = registry
        .authenticate(OPERATOR_TOKEN)
        .expect("authenticate operator");
    let service = WorkOrderIntakeService::new(store.clone(), Arc::clone(&bridge));

    let f = ServiceFixture {
        _dir: dir,
        store,
        registry: Arc::clone(&registry),
        policy: Arc::clone(&policy),
        clock: Arc::clone(&clock),
        service,
        operator,
        reader: registry.authenticate(OPERATOR_TOKEN).expect("auth"),
        foreign: registry.authenticate(OPERATOR_TOKEN).expect("auth"),
        project_id: project_id.clone(),
        root_id: root_id.clone(),
    };

    let cap_req = build_capture_request(
        &f,
        "cmd-ttl-expire",
        "cap-ttl",
        1,
        0,
        None,
        "Request that expires while queued before writer turn",
        "src-ttl",
        "rev-1",
        RequesterAttribution::Unknown {},
    );

    // With a 500ns TTL and auto_step = 600ns per clock read, admit_inflight records
    // expires_at = t + 500ns, and the Store writer's take_matching_inflight observes
    // t + 600ns >= expires_at, sweeping the expired ticket and denying the command.
    f.clock.set_auto_step(600);
    assert_eq!(
        f.service.capture_raw(&f.operator, cap_req.clone()),
        Err(WorkOrderIntakeError::AccessDenied)
    );
    f.clock.set_auto_step(0);
    assert_eq!(f.service.admission_bridge().active_inflight_count(), Ok(0));

    // Selection mismatch: tamper with work_order.title so it disagrees with the previewed item.
    let mut tampered_rec = build_enriched_record_request(
        &f,
        "cmd-tampered",
        "wo-tampered",
        1,
        0,
        "plan-tampered",
        0,
        "task-tampered",
        "src-tampered",
        "rev-1",
        RequesterAttribution::Unknown {},
    );
    tampered_rec.work_order.title = Some("Tampered title not matching JSON source".into());
    assert_eq!(
        f.service.record_enriched(&f.operator, tampered_rec),
        Err(WorkOrderIntakeError::SelectionMismatch)
    );

    // Persist valid capture and enriched work order, drop Store, and reopen to verify durable schema/integrity.
    let valid_cap = f
        .service
        .capture_raw(&f.operator, cap_req)
        .expect("valid capture");
    assert!(!valid_cap.replayed);

    let valid_rec = build_enriched_record_request(
        &f,
        "cmd-reopen-wo",
        "wo-reopen",
        1,
        0,
        "plan-reopen",
        0,
        "task-reopen",
        "src-reopen",
        "rev-1",
        RequesterAttribution::Claimed {
            requester: "dana".into(),
        },
    );
    let rec_res = f
        .service
        .record_enriched(&f.operator, valid_rec)
        .expect("valid enriched record");
    assert!(!rec_res.replayed);

    let ServiceFixture {
        _dir: kept_dir,
        store,
        service,
        ..
    } = f;
    drop(service);
    drop(store);

    let reopened = Store::open_with_startup_authorities(
        &db_path,
        StoreAuthorities {
            lifecycle: None,
            intake: Some(bridge.clone() as Arc<dyn admiralty_store::intake::IntakeAuthority>),
            intake_capture: Some(
                bridge.clone() as Arc<dyn admiralty_store::intake_capture::IntakeCaptureAuthority>
            ),
            questions: None,
        },
    )
    .expect("reopen store and pass initialize validate_all checks");
    let reopened_service = WorkOrderIntakeService::new(reopened, bridge);
    let operator = registry.authenticate(OPERATOR_TOKEN).expect("auth");
    assert!(
        reopened_service
            .get_captured_intake(&operator, &project_id, &root_id, "cap-ttl", Some(1))
            .expect("read capture after reopen")
            .is_some()
    );
    assert!(
        reopened_service
            .get_work_order(&operator, &project_id, &root_id, "wo-reopen", Some(1))
            .expect("read work order after reopen")
            .is_some()
    );
    drop(kept_dir);
}
