//! Real isolated TCP + actual Store/service. No provider/model/dispatch effects.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{
    CommandId, LifecycleAction, LifecycleCommand, PrincipalId, ProjectId, RootId, SourceIdentity,
    TaskContract, TaskView, content_digest, intake::*,
};
use admiralty_server::{
    ServerConfig, app_with_work_order_routes,
    services::{
        work_order_intake::{
            AuthorizationStage, IntakeScopeGrant, WorkOrderIntakeAction,
            WorkOrderIntakeAdmissionBridge, WorkOrderIntakeAuthRegistry, WorkOrderIntakeBinding,
            WorkOrderIntakeCredential, WorkOrderIntakePolicy, WorkOrderIntakeService,
        },
        work_order_routes::{
            ExactLoopbackOrigins, MAX_HTTP_BODY_BYTES, MAX_HTTP_QUERY_BYTES, WorkOrderRouteState,
        },
    },
};
use admiralty_store::{
    LifecycleAuthority, LifecycleEffect, Store, StoreAuthorities,
    intake::IntakeRecordResult,
    intake_capture::{CaptureIntakeRequest, CaptureResult, CapturedIntake},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
const MAIN: &str = "private-http-fixture-main-0123456789abcdef";
const INTAKE: &str = "intake-fixture-credential-0123456789abcdef";
const READER: &str = "read-only-fixture-credential-0123456789abcdef";
const FOREIGN: &str = "foreign-fixture-credential-0123456789abcdef";
type Hook = Arc<dyn Fn(AuthorizationStage) + Send + Sync>;
struct Policy {
    hook: Mutex<Option<Hook>>,
    prequeue: AtomicUsize,
}
impl WorkOrderIntakePolicy for Policy {
    fn authorize_current(&self, b: &WorkOrderIntakeBinding<'_>) -> bool {
        if b.stage == AuthorizationStage::ServicePreQueue {
            self.prequeue.fetch_add(1, Ordering::SeqCst);
        }
        let hook = self.hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(b.stage);
        }
        true
    }
}
struct CancelOnly;
impl LifecycleAuthority for CancelOnly {
    fn authorize(
        &self,
        _: &SourceIdentity,
        c: &LifecycleCommand,
        _: &TaskView,
        e: Option<&LifecycleEffect<'_>>,
    ) -> bool {
        c.actor.as_str() == "fixture-operator"
            && matches!(c.action, LifecycleAction::CancelPending { .. })
            && e.is_none()
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    registry: Arc<WorkOrderIntakeAuthRegistry>,
    service: Arc<WorkOrderIntakeService>,
    policy: Arc<Policy>,
}
fn all_actions() -> BTreeSet<WorkOrderIntakeAction> {
    use WorkOrderIntakeAction::*;
    BTreeSet::from([
        PreviewRaw,
        CaptureRaw,
        ReadCaptured,
        RecordEnriched,
        ReadWorkOrder,
        QueryDuplicates,
    ])
}
fn registry() -> Arc<WorkOrderIntakeAuthRegistry> {
    let grants = |project: &str, root: &str, actions: BTreeSet<WorkOrderIntakeAction>| {
        vec![IntakeScopeGrant {
            project_id: ProjectId::new(project).unwrap(),
            root_id: RootId::new(root).unwrap(),
            authority_ref: "grant:intake".into(),
            authority_generation: 1,
            policy: IntakePolicy::default(),
            allowed_actions: actions,
        }]
    };
    Arc::new(
        WorkOrderIntakeAuthRegistry::new(vec![
            WorkOrderIntakeCredential::configured(
                "operator".into(),
                INTAKE.into(),
                PrincipalId::new("fixture-operator").unwrap(),
                grants("project", "root", all_actions()),
            ),
            WorkOrderIntakeCredential::configured(
                "reader".into(),
                READER.into(),
                PrincipalId::new("fixture-operator").unwrap(),
                grants(
                    "project",
                    "root",
                    BTreeSet::from([
                        WorkOrderIntakeAction::ReadCaptured,
                        WorkOrderIntakeAction::ReadWorkOrder,
                        WorkOrderIntakeAction::QueryDuplicates,
                    ]),
                ),
            ),
            WorkOrderIntakeCredential::configured(
                "foreign".into(),
                FOREIGN.into(),
                PrincipalId::new("foreign").unwrap(),
                grants("other", "other-root", all_actions()),
            ),
        ])
        .unwrap(),
    )
}
fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let registry = registry();
    let policy = Arc::new(Policy {
        hook: Mutex::new(None),
        prequeue: AtomicUsize::new(0),
    });
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::new(
        registry.clone(),
        Some(policy.clone()),
    ));
    let store = Store::open_with_startup_authorities(
        dir.path().join("store.sqlite"),
        StoreAuthorities {
            intake: Some(bridge.clone()),
            intake_capture: Some(bridge.clone()),
            lifecycle: Some(Arc::new(CancelOnly)),
            ..Default::default()
        },
    )
    .unwrap();
    let service = Arc::new(WorkOrderIntakeService::new(store.clone(), bridge));
    Fixture {
        _dir: dir,
        store,
        registry,
        service,
        policy,
    }
}
struct Http {
    addr: SocketAddr,
    task: JoinHandle<()>,
}
impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Answer {
    status: u16,
    body: Value,
    raw: String,
}
impl Http {
    async fn start(
        f: &Fixture,
        configured: bool,
        alternate: Option<Arc<WorkOrderIntakeAuthRegistry>>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = configured.then(|| {
            WorkOrderRouteState::new(
                f.service.clone(),
                alternate.unwrap_or_else(|| f.registry.clone()),
                ExactLoopbackOrigins::new(vec![format!("http://{addr}")]).unwrap(),
            )
            .unwrap()
        });
        let app = app_with_work_order_routes(
            f.store.clone(),
            ServerConfig {
                listen: addr,
                simulated: false,
                token: Some(MAIN.into()),
                assets: None,
                legacy: None,
                models_file: None,
            },
            None,
            state,
        )
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, task }
    }
    fn headers(&self, token: &str) -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {MAIN}")),
            (
                "X-Admiralty-Intake-Authorization".into(),
                format!("Bearer {token}"),
            ),
            ("Origin".into(), format!("http://{}", self.addr)),
            ("X-Admiralty-Requested-With".into(), "intake".into()),
            ("Content-Type".into(), "application/json".into()),
        ]
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        headers: Vec<(String, String)>,
    ) -> Answer {
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.addr,
            body.len()
        );
        for (k, v) in headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        let mut data = request.into_bytes();
        data.extend_from_slice(body);
        self.raw(&data).await
    }
    async fn raw(&self, bytes: &[u8]) -> Answer {
        let stream = TcpStream::connect(self.addr).await.unwrap();
        let raw = tokio::time::timeout(Duration::from_secs(15), async {
            let mut sent = 0;
            while sent < bytes.len() {
                stream.writable().await.unwrap();
                match stream.try_write(&bytes[sent..]) {
                    Ok(0) => break,
                    Ok(n) => sent += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
            let mut raw = Vec::new();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                stream.readable().await.unwrap();
                match stream.try_read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        assert!(raw.len() + n <= 17 * 1024 * 1024);
                        raw.extend_from_slice(&buffer[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    // Early rejection can close with unread request bytes.
                    // A reset is usable only after actual response bytes; the
                    // parser below still requires complete HTTP headers/JSON.
                    Err(e)
                        if e.kind() == std::io::ErrorKind::ConnectionReset && !raw.is_empty() =>
                    {
                        break;
                    }
                    Err(e) => panic!("fixture HTTP read failed: {e}"),
                }
            }
            raw
        })
        .await
        .unwrap();
        let raw = String::from_utf8(raw).unwrap();
        let split = raw.find("\r\n\r\n").unwrap();
        let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = serde_json::from_str(&raw[split + 4..]).unwrap_or(Value::Null);
        Answer { status, body, raw }
    }
    async fn json(&self, path: &str, value: &impl serde::Serialize) -> Answer {
        self.request(
            "POST",
            path,
            &serde_json::to_vec(value).unwrap(),
            self.headers(INTAKE),
        )
        .await
    }
    async fn get(&self, path: &str, token: &str) -> Answer {
        self.request("GET", path, b"", self.headers(token)).await
    }
}
fn provenance(id: &str) -> SourceProvenance {
    SourceProvenance {
        schema_version: 1,
        source_id: id.into(),
        revision: "v1".into(),
        captured_at: "2026-10-19T05:20:00Z".into(),
        requester: RequesterAttribution::Claimed {
            requester: "attributed request author".into(),
        },
        origin: None,
    }
}
fn preview_input(text: &str, id: &str) -> Value {
    json!({"project_id":"project","root_id":"root","format":"plain_text","provenance":provenance(id),"source_payload":{"kind":"inline_utf8","text":text}})
}
fn capture_from(preview: IntakePreview, text: &str, id: &str, cmd: &str) -> CaptureIntakeRequest {
    CaptureIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new(cmd).unwrap(),
        capture_id: id.into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: preview.authorized_scope,
        format: preview.format,
        provenance: preview.provenance,
        source_payload: SourcePayload::InlineUtf8 { text: text.into() },
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
    }
}
async fn capture_request(h: &Http, id: &str, cmd: &str, text: &str) -> CaptureIntakeRequest {
    let answer = h.json("/v2/intake/preview", &preview_input(text, id)).await;
    assert_eq!(answer.status, 200, "{}", answer.raw);
    capture_from(
        serde_json::from_value(answer.body["data"].clone()).unwrap(),
        text,
        id,
        cmd,
    )
}
fn task() -> TaskContract {
    serde_json::from_value(json!({"schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,"title":"Explicit task","objective":"Only record an explicit contract","non_goals":["No execution"],"owned_paths":["src/example.rs"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],"criteria":[{"id":"c","text":"Explicit check","check_ids":["check"]}],"checks":[{"id":"check","program":"cargo","args":["test"],"cwd":".","timeout_secs":1}],"authority_ref":"grant:intake","max_attempts":1,"simulated":false})).unwrap()
}
async fn enriched(h: &Http) -> RecordIntakeRequest {
    let task = task();
    let delivery = DeliveryRequirement {
        schema_version: 1,
        task_id: task.task_id.clone(),
        contract_revision: 1,
        contract_digest: content_digest(&task).unwrap(),
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "repository-main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let mut batch = IntakeBatch {
        schema_version: 1,
        items: vec![IntakeItem {
            text: "Exact enriched request".into(),
            title: Some("Work order".into()),
            rationale: Some("Explicit enrichment".into()),
            constraints: vec![],
            requested_timing: None,
            claimed_project_id: Some(task.project_id.clone()),
            outcomes: vec![OutcomeInput {
                id: "outcome".into(),
                description: "Explicit outcome".into(),
                source_spans: vec![SourceSpan { start: 10, end: 20 }],
                constraints: vec![],
                task: Some(task.clone()),
                delivery: Some(delivery.clone()),
            }],
        }],
    };
    let initial = serde_json::to_string(&batch).unwrap();
    let start = initial.find("Exact enriched request").unwrap() as u32;
    batch.items[0].outcomes[0].source_spans = vec![SourceSpan {
        start,
        end: start + 22,
    }];
    let raw = serde_json::to_string(&batch).unwrap();
    let input = json!({"project_id":"project","root_id":"root","format":"json_batch","provenance":provenance("enriched"),"source_payload":{"kind":"inline_utf8","text":raw}});
    let answer = h.json("/v2/intake/preview", &input).await;
    assert_eq!(answer.status, 200, "{}", answer.raw);
    let p: IntakePreview = serde_json::from_value(answer.body["data"].clone()).unwrap();
    let item = &p.requests[0];
    let outcome = &item.outcomes[0];
    RecordIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new("record-command").unwrap(),
        expected_work_order_revision: 0,
        plan_id: "plan".into(),
        expected_plan_revision: 0,
        preview_manifest_digest: p.manifest_digest.clone(),
        policy_revision: p.policy_revision.clone(),
        policy_digest: p.policy_digest.clone(),
        authority_generation: p.authorized_scope.authority_generation,
        work_order: WorkOrderRevision {
            schema_version: 1,
            id: "work-order".into(),
            revision: 1,
            scope: p.authorized_scope.clone(),
            provenance: p.provenance.clone(),
            title: item.title.clone(),
            outcome: Some(outcome.outcome.description.clone()),
            rationale: item.rationale.clone(),
            constraints: item.constraints.clone(),
            requested_timing: item.requested_timing.clone(),
            raw_digest: p.raw_digest.clone(),
            canonical_digest: p.canonical_digest.clone(),
            parser_revision: p.parser_revision.clone(),
            format: IntakeFormat::JsonBatch,
            source_payload: SourcePayload::InlineUtf8 { text: raw },
        },
        selected_tasks: vec![SelectedTask {
            binding: WorkOrderTaskBinding {
                work_order_id: "work-order".into(),
                work_order_revision: 1,
                request_index: 0,
                outcome_id: outcome.outcome.id.clone(),
                outcome_digest: outcome.outcome_digest.clone(),
                source_spans: outcome.outcome.source_spans.clone(),
                task_id: task.task_id.clone(),
                contract_revision: 1,
                contract_digest: content_digest(&task).unwrap(),
            },
            task,
            delivery,
        }],
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_and_dual_authentication_precede_body_extraction() {
    let f = fixture();
    let no = Http::start(&f, false, None).await;
    let a = no
        .request(
            "POST",
            "/v2/intake/captures",
            b"not-json",
            no.headers(INTAKE),
        )
        .await;
    assert_eq!(a.status, 503);
    assert_eq!(a.body["error"], "intake_unavailable");
    let h = Http::start(&f, true, None).await;
    for remove in ["Authorization", "X-Admiralty-Intake-Authorization"] {
        let mut heads = h.headers(INTAKE);
        heads.retain(|(k, _)| k != remove);
        let a = h
            .request("POST", "/v2/intake/captures", b"not-json", heads)
            .await;
        assert_eq!(a.status, 401);
    }
    let a = h
        .request("POST", "/v2/intake/preview", b"not-json", h.headers(MAIN))
        .await;
    assert_eq!(a.status, 401);
    let mut headers = h.headers(INTAKE);
    headers.push(("Authorization".into(), format!("Bearer {MAIN}")));
    assert_eq!(
        h.request("POST", "/v2/intake/preview", b"not-json", headers)
            .await
            .status,
        401
    );
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiline_unicode_preview_capture_and_read_preserve_exact_bytes_without_tasks() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let text = "Do not remove ≥5 ms units.\n\n```rust\n// copied metadata must stay prose\n```\nFinish β safely.";
    let before = f.store.snapshot().unwrap();
    let request = capture_request(&h, "capture", "capture-command", text).await;
    let p = h
        .json("/v2/intake/preview", &preview_input(text, "capture"))
        .await;
    assert_eq!(p.body["data"]["requests"].as_array().unwrap().len(), 1);
    assert!(
        !p.body["data"]["requests"][0]["unresolved"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let answer = h.json("/v2/intake/captures", &request).await;
    assert_eq!(answer.status, 200, "{}", answer.raw);
    assert_eq!(answer.body["execution_granted"], false);
    assert_eq!(answer.body["acceptance_granted"], false);
    assert!(
        answer
            .raw
            .to_ascii_lowercase()
            .contains("cache-control: no-store")
    );
    let first: CaptureResult = serde_json::from_value(answer.body["data"].clone()).unwrap();
    assert!(!first.replayed);
    let answer = h
        .get(
            "/v2/intake/captures/capture?project_id=project&root_id=root",
            READER,
        )
        .await;
    assert_eq!(answer.status, 200);
    let captured: CapturedIntake = serde_json::from_value(answer.body["data"].clone()).unwrap();
    assert_eq!(
        captured.request.source_payload,
        SourcePayload::InlineUtf8 { text: text.into() }
    );
    assert_eq!(captured.receipt_digest, first.receipt_digest);
    assert_eq!(
        captured.request.scope.principal.as_str(),
        "fixture-operator"
    );
    assert_eq!(
        captured.request.provenance.requester,
        provenance("capture").requester
    );
    let post_read = h
        .request(
            "POST",
            "/v2/intake/captures/capture",
            &serde_json::to_vec(&json!({"project_id":"project","root_id":"root","revision":null}))
                .unwrap(),
            h.headers(READER),
        )
        .await;
    assert_eq!(post_read.status, 200);
    let post_capture: CapturedIntake =
        serde_json::from_value(post_read.body["data"].clone()).unwrap();
    assert_eq!(post_capture, captured);
    let retry = h.json("/v2/intake/captures", &request).await;
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body["data"]["receipt_digest"], first.receipt_digest);
    assert_eq!(retry.body["data"]["replayed"], true);
    assert_eq!(f.store.snapshot().unwrap(), before);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enriched_record_retry_cas_and_all_state_duplicate_query_use_actual_store() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let request = enriched(&h).await;
    let answer = h.json("/v2/intake/work-orders", &request).await;
    assert_eq!(answer.status, 200, "{}", answer.raw);
    let first: IntakeRecordResult = serde_json::from_value(answer.body["data"].clone()).unwrap();
    assert!(!first.replayed);
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
    let registered = f
        .store
        .workplan(ProjectId::new("project").unwrap(), "plan".into(), None)
        .unwrap()
        .unwrap();
    assert_eq!(
        registered.plan.tasks,
        vec![request.selected_tasks[0].task.clone()]
    );
    assert_eq!(
        h.get(
            "/v2/intake/work-orders/work-order?project_id=project&root_id=root",
            READER
        )
        .await
        .status,
        200
    );
    let post_read = h
        .request(
            "POST",
            "/v2/intake/work-orders/work-order",
            &serde_json::to_vec(&json!({"project_id":"project","root_id":"root","revision":1}))
                .unwrap(),
            h.headers(READER),
        )
        .await;
    assert_eq!(post_read.status, 200);
    assert_eq!(post_read.body["data"]["work_order"]["id"], "work-order");
    let retry = h.json("/v2/intake/work-orders", &request).await;
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body["data"]["replayed"], true);
    assert_eq!(retry.body["data"]["receipt_digest"], first.receipt_digest);
    let mut collision = request.clone();
    collision.plan_id = "conflicting-plan".into();
    assert_eq!(
        h.json("/v2/intake/work-orders", &collision).await.status,
        409
    );
    let mut stale = request.clone();
    stale.command_id = CommandId::new("other-command").unwrap();
    assert_eq!(h.json("/v2/intake/work-orders", &stale).await.status, 409);
    // Explicit trusted fixture admission is separate from HTTP record. The
    // HTTP mutation above emitted no domain task or launch/outbox intent.
    f.store
        .append(
            SourceIdentity {
                producer_id: "fixture-domain".into(),
                incarnation: "boot".into(),
                sequence: "1".into(),
            },
            admiralty_core::DomainEvent::TaskSubmitted {
                contract: request.selected_tasks[0].task.clone(),
            },
            "2026-10-19T05:20:00Z".into(),
        )
        .unwrap();
    let task = f
        .store
        .snapshot()
        .unwrap()
        .projection
        .tasks
        .get(&request.selected_tasks[0].task.task_id)
        .unwrap()
        .clone();
    let cmd = LifecycleCommand {
        protocol_version: 2,
        task_id: task.contract.task_id.clone(),
        expected_task_digest: content_digest(&task).unwrap(),
        actor: PrincipalId::new("fixture-operator").unwrap(),
        action: LifecycleAction::CancelPending {
            reason: "fixture explicit cancellation".into(),
        },
    };
    f.store
        .execute_lifecycle(
            SourceIdentity {
                producer_id: "fixture-lifecycle".into(),
                incarnation: "boot".into(),
                sequence: "1".into(),
            },
            cmd,
            "2026-10-19T05:21:00Z".into(),
            None,
        )
        .unwrap();
    let q = json!({"project_id":"project","root_id":"root","query":{"schema_version":1,"project_id":"project","raw_digest":request.work_order.raw_digest,"canonical_digest":request.work_order.canonical_digest,"origin":null,"command_id":request.command_id,"cursor":null,"limit":10}});
    let answer = h.json("/v2/intake/duplicates/query", &q).await;
    assert_eq!(answer.status, 200, "{}", answer.raw);
    let page: DuplicatePage = serde_json::from_value(answer.body["data"].clone()).unwrap();
    assert!(!page.candidates.is_empty());
    assert!(page.candidates.iter().any(|c| c.lifecycle == "cancelled"));
    assert_eq!(answer.body["execution_granted"], false);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_scopes_labels_read_only_and_generation_rotation_deny_without_writes() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let request =
        capture_request(&h, "deny", "deny-command", "Only explicit scope authorizes").await;
    for token in [READER, FOREIGN] {
        assert_eq!(
            h.request(
                "POST",
                "/v2/intake/captures",
                &serde_json::to_vec(&request).unwrap(),
                h.headers(token)
            )
            .await
            .status,
            403
        );
    }
    let mut labels = preview_input("text", "labels");
    labels["authenticated_actor"] = json!("fixture-operator");
    assert_eq!(h.json("/v2/intake/preview", &labels).await.status, 400);
    let foreign_registry = registry();
    let wrong = Http::start(&f, true, Some(foreign_registry)).await;
    assert_eq!(
        wrong
            .json(
                "/v2/intake/preview",
                &preview_input("text", "wrong-registry")
            )
            .await
            .status,
        403
    );
    let historical = capture_request(
        &h,
        "historical",
        "historical-command",
        "Committed before current revocation",
    )
    .await;
    let original = h.json("/v2/intake/captures", &historical).await;
    assert_eq!(original.status, 200);
    let reg = f.registry.clone();
    *f.policy.hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == AuthorizationStage::WriterTurn {
            reg.update_grant(
                "operator",
                &ProjectId::new("project").unwrap(),
                &RootId::new("root").unwrap(),
                2,
                IntakePolicy::default(),
            )
            .unwrap();
        }
    }));
    assert_eq!(h.json("/v2/intake/captures", &request).await.status, 403);
    *f.policy.hook.lock().unwrap() = None;
    assert_eq!(
        h.get(
            "/v2/intake/captures/deny?project_id=project&root_id=root",
            READER
        )
        .await
        .body["data"],
        Value::Null
    );
    f.registry
        .update_grant(
            "operator",
            &ProjectId::new("project").unwrap(),
            &RootId::new("root").unwrap(),
            1,
            IntakePolicy::default(),
        )
        .unwrap();
    let reg = f.registry.clone();
    *f.policy.hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == AuthorizationStage::WriterTurn {
            reg.revoke("operator").unwrap();
        }
    }));
    assert_eq!(h.json("/v2/intake/captures", &historical).await.status, 403);
    *f.policy.hook.lock().unwrap() = None;
    let retained = h
        .get(
            "/v2/intake/captures/historical?project_id=project&root_id=root",
            READER,
        )
        .await;
    assert_eq!(retained.status, 200);
    assert_eq!(
        retained.body["data"]["receipt_digest"],
        original.body["data"]["receipt_digest"]
    );
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_host_marker_json_query_and_chunked_body_bounds_fail_closed() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let valid = serde_json::to_vec(&preview_input("text", "bounds")).unwrap();
    for bad in ["null", "http://example.invalid:80", "http://localhost:1"] {
        let mut heads = h.headers(INTAKE);
        heads.iter_mut().find(|(k, _)| k == "Origin").unwrap().1 = bad.into();
        assert_eq!(
            h.request("POST", "/v2/intake/preview", &valid, heads)
                .await
                .status,
            403
        );
    }
    let mut heads = h.headers(INTAKE);
    heads.retain(|(k, _)| k != "X-Admiralty-Requested-With");
    assert_eq!(
        h.request("POST", "/v2/intake/preview", &valid, heads)
            .await
            .status,
        403
    );
    for name in ["Origin", "X-Admiralty-Intake-Authorization"] {
        let mut heads = h.headers(INTAKE);
        let extra = heads.iter().find(|(k, _)| k == name).unwrap().clone();
        heads.push(extra);
        let status = h
            .request("POST", "/v2/intake/preview", &valid, heads)
            .await
            .status;
        assert_eq!(status, if name == "Origin" { 403 } else { 401 });
    }
    let mut heads = h.headers(INTAKE);
    heads.push(("Host".into(), "localhost:1".into()));
    assert_eq!(
        h.request("POST", "/v2/intake/preview", &valid, heads)
            .await
            .status,
        403
    );
    let mut wrong_host = format!(
        "POST /v2/intake/preview HTTP/1.1\r\nHost: localhost:1\r\nConnection: close\r\nContent-Length: {}\r\n",
        valid.len()
    );
    for (k, v) in h.headers(INTAKE) {
        wrong_host.push_str(&format!("{k}: {v}\r\n"));
    }
    wrong_host.push_str("\r\n");
    let mut wrong_host = wrong_host.into_bytes();
    wrong_host.extend_from_slice(&valid);
    assert_eq!(h.raw(&wrong_host).await.status, 403);
    assert_eq!(
        h.json("/v2/intake/preview", &json!({"unexpected":"field"}))
            .await
            .status,
        400
    );
    let mut artifact = preview_input("x", "artifact");
    artifact["source_payload"] = json!({"kind":"artifact_claim","raw_digest":content_digest(&"claimed").unwrap(),"size_bytes":1,"artifact_ref":"artifact:claimed","pin_receipt_ref":"pin:claimed"});
    assert_eq!(h.json("/v2/intake/preview", &artifact).await.status, 422);
    let path = format!(
        "/v2/intake/captures/x?project_id=project&root_id=root&unused={}",
        "x".repeat(MAX_HTTP_QUERY_BYTES)
    );
    assert_eq!(h.get(&path, INTAKE).await.status, 414);
    assert_eq!(
        h.get(
            "/v2/intake/captures/x?project_id=project&root_id=root&unknown=x",
            INTAKE
        )
        .await
        .status,
        400
    );
    let oversized = vec![b'x'; MAX_HTTP_BODY_BYTES + 1];
    assert_eq!(
        h.request("POST", "/v2/intake/preview", &oversized, h.headers(INTAKE))
            .await
            .status,
        413
    );
    let mut chunked = format!(
        "POST /v2/intake/preview HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n",
        h.addr
    );
    for (k, v) in h.headers(INTAKE) {
        chunked.push_str(&format!("{k}: {v}\r\n"));
    }
    chunked.push_str(&format!("\r\n{:x}\r\n", oversized.len()));
    let mut wire = chunked.into_bytes();
    wire.extend(oversized);
    wire.extend_from_slice(b"\r\n0\r\n\r\n");
    assert_eq!(h.raw(&wire).await.status, 413);
    let deep = format!("{}0{}", "[".repeat(140), "]".repeat(140));
    assert_eq!(
        h.request(
            "POST",
            "/v2/intake/preview",
            deep.as_bytes(),
            h.headers(INTAKE)
        )
        .await
        .status,
        400
    );
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
}
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_requests_retain_capacity_and_writer_revocation_denies_commit_and_retry() {
    let f = fixture();
    let h = Arc::new(Http::start(&f, true, None).await);
    let mut requests = Vec::new();
    for n in 0..8 {
        requests.push(
            capture_request(
                &h,
                &format!("cap{n}"),
                &format!("cmd{n}"),
                "Original capture before revocation",
            )
            .await,
        );
    }
    let hold = Arc::new((Mutex::new(false), Condvar::new()));
    let _release = Release(hold.clone());
    let entered = Arc::new(AtomicUsize::new(0));
    let signal = entered.clone();
    let held = hold.clone();
    *f.policy.hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == AuthorizationStage::WriterTurn {
            signal.fetch_add(1, Ordering::SeqCst);
            let (lock, cv) = &*held;
            let mut ready = lock.lock().unwrap();
            while !*ready {
                ready = cv.wait(ready).unwrap();
            }
        }
    }));
    let baseline = f.policy.prequeue.load(Ordering::SeqCst);
    let mut clients = Vec::new();
    for request in &requests {
        let h = h.clone();
        let request = request.clone();
        clients.push(tokio::spawn(async move {
            h.json("/v2/intake/captures", &request).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.policy.prequeue.load(Ordering::SeqCst) < baseline + 8
            || entered.load(Ordering::SeqCst) == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for task in clients {
        task.abort();
    }
    tokio::task::yield_now().await;
    assert_eq!(
        h.get(
            "/v2/intake/captures/none?project_id=project&root_id=root",
            READER
        )
        .await
        .status,
        429
    );
    f.registry.revoke("operator").unwrap();
    {
        let (lock, cv) = &*hold;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    *f.policy.hook.lock().unwrap() = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let answer = h
                .get(
                    "/v2/intake/captures/cap0?project_id=project&root_id=root",
                    READER,
                )
                .await;
            if answer.status != 429 {
                assert_eq!(answer.status, 200);
                assert_eq!(answer.body["data"], Value::Null);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.json("/v2/intake/captures", &requests[0]).await.status,
        401
    );
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
}
#[test]
fn origin_constructor_accepts_exact_loopback_pairs_and_rejects_ambiguous_urls() {
    for origin in [
        "http://localhost:18891",
        "http://127.0.0.2:99",
        "http://[::1]:18891",
    ] {
        ExactLoopbackOrigins::new(vec![origin.into()]).unwrap();
    }
    for origin in [
        "https://localhost:18891",
        "http://localhost",
        "http://localhost:0",
        "http://localhost:01",
        "http://localhost:18891/",
        "http://user@localhost:18891",
        "http://localhost:18891?x",
        "http://localhost:18891#x",
        "http://example.com:18891",
        "http://[::]:18891",
        "http://localhost.evil:18891",
        "http://*:18891",
    ] {
        assert!(
            ExactLoopbackOrigins::new(vec![origin.into()]).is_err(),
            "{origin}"
        );
    }
    assert!(ExactLoopbackOrigins::new(Vec::new()).is_err());
    assert!(ExactLoopbackOrigins::new(vec!["http://localhost:1".into(); 2]).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_routes_reject_framed_bodies_without_consuming_them() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let path = "/v2/intake/captures/none?project_id=project&root_id=root";
    assert_eq!(
        h.request("GET", path, b"x", h.headers(INTAKE)).await.status,
        400
    );
    let mut wire = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n",
        h.addr
    );
    for (k, v) in h.headers(INTAKE) {
        wire.push_str(&format!("{k}: {v}\r\n"));
    }
    wire.push_str("\r\n1\r\nx\r\n0\r\n\r\n");
    assert_eq!(h.raw(wire.as_bytes()).await.status, 400);
    assert!(f.store.snapshot().unwrap().projection.tasks.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_reads_require_closed_bounded_json_independent_current_scope_and_no_query() {
    let f = fixture();
    let h = Http::start(&f, true, None).await;
    let request = capture_request(
        &h,
        "post-read",
        "post-read-command",
        "Browser read original",
    )
    .await;
    assert_eq!(h.json("/v2/intake/captures", &request).await.status, 200);
    let path = "/v2/intake/captures/post-read";
    let query = json!({"project_id":"project","root_id":"root","revision":null});
    let bytes = serde_json::to_vec(&query).unwrap();
    let before = f.store.snapshot().unwrap();
    let valid = h.request("POST", path, &bytes, h.headers(READER)).await;
    assert_eq!(valid.status, 200);
    assert_eq!(valid.body["execution_granted"], false);
    let mut no_origin = h.headers(READER);
    no_origin.retain(|(k, _)| k != "Origin");
    assert_eq!(
        h.request(
            "GET",
            "/v2/intake/captures/post-read?project_id=project&root_id=root",
            b"",
            no_origin.clone()
        )
        .await
        .status,
        403
    );
    assert_eq!(h.request("POST", path, &bytes, no_origin).await.status, 403);
    assert_eq!(
        h.request("POST", path, &bytes, h.headers(FOREIGN))
            .await
            .status,
        403
    );
    let foreign = json!({"project_id":"other","root_id":"other-root","revision":null});
    assert_eq!(h.json(path, &foreign).await.status, 403);
    assert_eq!(
        h.json(&format!("{path}?project_id=project"), &query)
            .await
            .status,
        400
    );
    let mut unknown = query.clone();
    unknown["actor"] = json!("fixture-operator");
    assert_eq!(h.json(path, &unknown).await.status, 400);
    assert_eq!(
        h.request(
            "POST",
            path,
            br#"{"project_id":"project","project_id":"other","root_id":"root"}"#,
            h.headers(READER)
        )
        .await
        .status,
        400
    );
    assert_eq!(
        h.request(
            "POST",
            path,
            &vec![b'x'; MAX_HTTP_BODY_BYTES + 1],
            h.headers(READER)
        )
        .await
        .status,
        413
    );
    let reg = f.registry.clone();
    *f.policy.hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == AuthorizationStage::ServicePreQueue {
            reg.revoke("reader").unwrap();
        }
    }));
    assert_eq!(
        h.request("POST", path, &bytes, h.headers(READER))
            .await
            .status,
        403
    );
    *f.policy.hook.lock().unwrap() = None;
    assert_eq!(f.store.snapshot().unwrap(), before);
}
