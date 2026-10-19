#![cfg(unix)]
use admiralty_core::{
    CommandId, PrincipalId, ProjectId, RootId, TaskContract, content_digest,
    intake::{
        DeliveryRequirement, DeliveryStage, DeliveryTarget, DeliveryTargetKind, IntakeBatch,
        IntakeFormat, IntakeItem, OutcomeInput, RecordIntakeRequest, RequesterAttribution,
        SelectedTask, SourcePayload, SourceProvenance, SourceSpan, TargetRequirement,
        WorkOrderRevision, WorkOrderTaskBinding,
    },
};
use admiralty_server::{
    intake_configuration::load_private_intake_configuration,
    services::work_order_intake::{
        AuthorizationStage, PreviewIntakeInput, WorkOrderIntakeAction,
        WorkOrderIntakeAdmissionBridge, WorkOrderIntakeAuthRegistry, WorkOrderIntakeBinding,
        WorkOrderIntakeService,
    },
    startup::{TrustedIntakeComposition, open_native_services_with_intake},
};
use admiralty_store::{Store, StoreAuthorities, intake_capture::CaptureIntakeRequest};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::Arc,
};
type Error = Box<dyn std::error::Error + Send + Sync>;
const TOKEN: &str = "intake-private-test-012345678901234567890123456789";
const OWNER: &str = "owner-private-test-012345678901234567890123456789";
const ACTIONS: [&str; 6] = [
    "preview_raw",
    "capture_raw",
    "record_enriched",
    "read_captured",
    "read_work_order",
    "query_duplicates",
];
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Result<Self, Error> {
        let p = std::env::temp_dir()
            .canonicalize()?
            .join(format!("ic-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&p)?;
        let f = Self(p);
        write_private(&f.0.join("intake-token"), TOKEN.as_bytes())?;
        write_private(&f.0.join("native.json"),br#"{"schema_version":1,"operator":{"principal":"owner","projects":["allowed"]},"agents":[]}"#)?;
        Ok(f)
    }
    fn config(&self) -> Value {
        json!({"schema_version":1,"origins":["http://127.0.0.1:18890"],
        "policies":[{"id":"bounded","revision":"intake-v1","max_source_bytes":"1048576","max_requests":"256","max_outcomes":"1000","max_json_item_text_bytes":"65536"}],
        "credentials":[{"credential_id":"intake-user","token_file":self.0.join("intake-token"),"actor":"submitter","grants":[{
            "project_id":"allowed","root_id":"root","authority_ref":"grant:intake","authority_generation":"9007199254740993","policy_id":"bounded","actions":ACTIONS}]}],
        "action_rules":[{"credential_id":"intake-user","actor":"submitter","project_id":"allowed","root_id":"root","authority_ref":"grant:intake","authority_generation":"9007199254740993","policy_id":"bounded","actions":ACTIONS}]})
    }
    fn write(&self, v: &Value) -> Result<PathBuf, Error> {
        let p = self.0.join(format!("config-{}.json", uuid::Uuid::new_v4()));
        write_private(&p, &serde_json::to_vec(v)?)?;
        Ok(p)
    }
    fn load(&self, v: &Value) -> Result<TrustedIntakeComposition, Error> {
        Ok(load_private_intake_configuration(&self.write(v)?)?)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write_private(p: &Path, b: &[u8]) -> Result<(), Error> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(p)?;
    f.write_all(b)?;
    Ok(())
}
fn service(
    f: &Fixture,
    c: TrustedIntakeComposition,
) -> Result<
    (
        Store,
        WorkOrderIntakeService,
        Arc<WorkOrderIntakeAuthRegistry>,
    ),
    Error,
> {
    let bridge = Arc::new(WorkOrderIntakeAdmissionBridge::new(
        c.registry.clone(),
        Some(c.policy),
    ));
    let store = Store::open_with_startup_authorities(
        f.0.join(format!("store-{}.sqlite", uuid::Uuid::new_v4())),
        StoreAuthorities {
            intake: Some(bridge.clone()),
            intake_capture: Some(bridge.clone()),
            ..Default::default()
        },
    )?;
    Ok((
        store.clone(),
        WorkOrderIntakeService::new(store, bridge),
        c.registry,
    ))
}
fn input(format: IntakeFormat, text: String) -> Result<PreviewIntakeInput, Error> {
    Ok(PreviewIntakeInput {
        project_id: ProjectId::new("allowed")?,
        root_id: RootId::new("root")?,
        format,
        provenance: SourceProvenance {
            schema_version: 1,
            source_id: "operator-request".into(),
            revision: "one".into(),
            captured_at: "2026-10-19T06:00:00Z".into(),
            requester: RequesterAttribution::Unknown {},
            origin: None,
        },
        source_payload: SourcePayload::InlineUtf8 { text },
    })
}
fn capture(
    s: &WorkOrderIntakeService,
    r: &WorkOrderIntakeAuthRegistry,
) -> Result<CaptureIntakeRequest, Error> {
    let actor = r.authenticate(TOKEN)?;
    let i = input(
        IntakeFormat::PlainText,
        "One request\n\n第二段 remains content.".into(),
    )?;
    let p = s.preview_raw(&actor, &i)?;
    assert_eq!(p.requests.len(), 1);
    Ok(CaptureIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new("capture-once")?,
        capture_id: "original".into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: p.authorized_scope,
        format: i.format,
        provenance: i.provenance,
        source_payload: i.source_payload,
        preview_manifest_digest: p.manifest_digest,
        policy_revision: p.policy_revision,
        policy_digest: p.policy_digest,
    })
}
fn enriched(
    s: &WorkOrderIntakeService,
    r: &WorkOrderIntakeAuthRegistry,
) -> Result<RecordIntakeRequest, Error> {
    let task: TaskContract = serde_json::from_value(
        json!({"schema_version":1,"project_id":"allowed","root_id":"root","task_id":"task1","revision":1,
        "title":"Review source","objective":"Record one independently testable outcome","non_goals":["No launch"],"owned_paths":["src/lib.rs"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],
        "criteria":[{"id":"c1","text":"Unit check passes","check_ids":["unit"]}],"checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":60}],"authority_ref":"grant:intake","max_attempts":1,"simulated":false}),
    )?;
    let delivery = DeliveryRequirement {
        schema_version: 1,
        task_id: task.task_id.clone(),
        contract_revision: 1,
        contract_digest: content_digest(&task)?,
        generation: 1,
        targets: vec![DeliveryTarget {
            id: "main".into(),
            target_kind: DeliveryTargetKind::Repository,
            requirement: TargetRequirement::Required {
                stage: DeliveryStage::Merged,
            },
        }],
    };
    let text = "Explicit reviewed outcome";
    let mut batch = IntakeBatch {
        schema_version: 1,
        items: vec![IntakeItem {
            text: text.into(),
            title: Some("Work order".into()),
            rationale: Some("Reviewed before recording".into()),
            requested_timing: None,
            constraints: vec![],
            claimed_project_id: Some(task.project_id.clone()),
            outcomes: vec![OutcomeInput {
                id: "one".into(),
                description: "Review source".into(),
                source_spans: vec![SourceSpan { start: 1, end: 2 }],
                constraints: vec![],
                task: Some(task.clone()),
                delivery: Some(delivery.clone()),
            }],
        }],
    };
    let original = serde_json::to_string(&batch)?;
    let start = original.find(text).ok_or("missing text fixture")? as u32;
    batch.items[0].outcomes[0].source_spans = vec![SourceSpan {
        start,
        end: start + text.len() as u32,
    }];
    let i = input(IntakeFormat::JsonBatch, serde_json::to_string(&batch)?)?;
    let actor = r.authenticate(TOKEN)?;
    let p = s.preview_raw(&actor, &i)?;
    let item = &p.requests[0];
    let outcome = &item.outcomes[0];
    assert!(outcome.unresolved.is_empty());
    Ok(RecordIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new("record-once")?,
        expected_work_order_revision: 0,
        plan_id: "plan1".into(),
        expected_plan_revision: 0,
        preview_manifest_digest: p.manifest_digest,
        policy_revision: p.policy_revision,
        policy_digest: p.policy_digest,
        authority_generation: p.authorized_scope.authority_generation,
        work_order: WorkOrderRevision {
            schema_version: 1,
            id: "order1".into(),
            revision: 1,
            scope: p.authorized_scope,
            provenance: i.provenance,
            title: item.title.clone(),
            outcome: Some(outcome.outcome.description.clone()),
            rationale: item.rationale.clone(),
            constraints: item.constraints.clone(),
            requested_timing: item.requested_timing.clone(),
            raw_digest: p.raw_digest,
            canonical_digest: p.canonical_digest,
            parser_revision: p.parser_revision,
            format: i.format,
            source_payload: i.source_payload,
        },
        selected_tasks: vec![SelectedTask {
            binding: WorkOrderTaskBinding {
                work_order_id: "order1".into(),
                work_order_revision: 1,
                request_index: 0,
                outcome_id: outcome.outcome.id.clone(),
                outcome_digest: outcome.outcome_digest.clone(),
                source_spans: outcome.outcome.source_spans.clone(),
                task_id: task.task_id.clone(),
                contract_revision: 1,
                contract_digest: content_digest(&task)?,
            },
            task,
            delivery,
        }],
    })
}

#[test]
fn actual_loaded_policy_pairs_writers_records_replays_and_revokes() -> Result<(), Error> {
    let f = Fixture::new()?;
    let (store, s, r) = service(&f, f.load(&f.config())?)?;
    let actor = r.authenticate(TOKEN)?;
    let c = capture(&s, &r)?;
    assert_eq!(c.scope.authority_generation, 9007199254740993);
    assert_eq!(
        serde_json::to_value(&c.scope)?["authority_generation"],
        "9007199254740993"
    );
    let first = s.capture_raw(&actor, c.clone())?;
    assert!(!first.replayed);
    assert!(s.capture_raw(&actor, c.clone())?.replayed);
    assert!(store.snapshot()?.projection.tasks.is_empty());
    let record = enriched(&s, &r)?;
    let original = s.record_enriched(&actor, record.clone())?;
    assert!(!original.replayed);
    assert!(s.record_enriched(&actor, record.clone())?.replayed);
    assert!(store.snapshot()?.projection.tasks.is_empty());
    let registered = store
        .workplan(ProjectId::new("allowed")?, "plan1".into(), None)?
        .ok_or("expected stored plan")?;
    let recorded_bytes = serde_json::to_vec(&registered)?;
    assert!(
        s.get_work_order(
            &actor,
            &ProjectId::new("allowed")?,
            &RootId::new("root")?,
            "order1",
            None
        )?
        .is_some()
    );
    r.revoke("intake-user")?;
    assert!(s.capture_raw(&actor, c).is_err());
    assert!(s.record_enriched(&actor, record).is_err());
    assert!(store.snapshot()?.projection.tasks.is_empty());
    assert_eq!(
        serde_json::to_vec(
            &store
                .workplan(ProjectId::new("allowed")?, "plan1".into(), None)?
                .ok_or("retained plan")?
        )?,
        recorded_bytes
    );
    Ok(())
}
#[test]
fn absent_rules_and_action_intersection_deny_without_mutation() -> Result<(), Error> {
    let f = Fixture::new()?;
    let mut config = f.config();
    config["action_rules"] = json!([]);
    let (store, s, r) = service(&f, f.load(&config)?)?;
    let a = r.authenticate(TOKEN)?;
    assert!(
        s.preview_raw(&a, &input(IntakeFormat::PlainText, "One".into())?)
            .is_err()
    );
    assert!(store.snapshot()?.projection.tasks.is_empty());
    let mut config = f.config();
    config["action_rules"][0]["actions"] = json!(["preview_raw"]);
    let (store, s, r) = service(&f, f.load(&config)?)?;
    let c = capture(&s, &r)?;
    assert!(s.capture_raw(&r.authenticate(TOKEN)?, c).is_err());
    assert!(store.snapshot()?.projection.tasks.is_empty());
    Ok(())
}
#[test]
fn policy_matches_full_identity_generation_caps_and_current_scope() -> Result<(), Error> {
    let f = Fixture::new()?;
    let c = f.load(&f.config())?;
    let p = admiralty_core::intake::IntakePolicy {
        revision: "intake-v1".into(),
        ..Default::default()
    };
    let actor = PrincipalId::new("submitter")?;
    let scope = admiralty_core::intake::AuthorizedIntakeScope {
        principal: actor.clone(),
        project_id: Some(ProjectId::new("allowed")?),
        root_id: Some(RootId::new("root")?),
        authority_ref: "grant:intake".into(),
        authority_generation: 9007199254740993,
    };
    let digest = p.identity_digest()?;
    fn binding<'a>(
        actor: &'a PrincipalId,
        scope: &'a admiralty_core::intake::AuthorizedIntakeScope,
        policy: &'a admiralty_core::intake::IntakePolicy,
        digest: &'a str,
    ) -> WorkOrderIntakeBinding<'a> {
        WorkOrderIntakeBinding {
            stage: AuthorizationStage::WriterTurn,
            action: WorkOrderIntakeAction::CaptureRaw,
            credential_id: "intake-user",
            authenticated_actor: actor,
            attributed_requester: None,
            scope,
            policy,
            policy_digest: digest,
            command_id: None,
            target_id: None,
            revision: None,
            request_digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }
    }
    assert!(
        c.policy
            .authorize_current(&binding(&actor, &scope, &p, &digest))
    );
    let mut other = scope.clone();
    other.authority_generation += 1;
    assert!(
        !c.policy
            .authorize_current(&binding(&actor, &other, &p, &digest))
    );
    other = scope.clone();
    other.root_id = Some(RootId::new("foreign")?);
    assert!(
        !c.policy
            .authorize_current(&binding(&actor, &other, &p, &digest))
    );
    other = scope.clone();
    other.principal = PrincipalId::new("foreign")?;
    assert!(
        !c.policy
            .authorize_current(&binding(&actor, &other, &p, &digest))
    );
    let mut changed = p.clone();
    changed.max_requests -= 1;
    assert!(
        !c.policy
            .authorize_current(&binding(&actor, &scope, &changed, &digest))
    );
    assert!(
        !c.policy
            .authorize_current(&binding(&actor, &scope, &p, "sha256:wrong"))
    );
    Ok(())
}
#[test]
fn closed_config_rejects_unknown_duplicate_references_rules_and_counters_before_db()
-> Result<(), Error> {
    let f = Fixture::new()?;
    let mut cases = Vec::new();
    let mut c = f.config();
    c["schema_version"] = json!(2);
    cases.push(c);
    let mut c = f.config();
    c["allowed_formats"] = json!(["plain_text"]);
    cases.push(c);
    for n in [
        json!(1),
        json!("0"),
        json!("01"),
        json!("+1"),
        json!("18446744073709551616"),
    ] {
        let mut c = f.config();
        c["credentials"][0]["grants"][0]["authority_generation"] = n;
        cases.push(c);
    }
    let mut c = f.config();
    c["policies"][0]["max_source_bytes"] = json!(1048576);
    cases.push(c);
    let mut c = f.config();
    c["policies"][0]["max_requests"] = json!("257");
    cases.push(c);
    let mut c = f.config();
    c["action_rules"][0]["credential_id"] = json!("unknown");
    cases.push(c);
    let mut c = f.config();
    c["action_rules"][0]["actor"] = json!("foreign");
    cases.push(c);
    let mut c = f.config();
    c["action_rules"][0]["actions"] = json!(["dispatch"]);
    cases.push(c);
    let mut c = f.config();
    c["action_rules"][0]["actions"] = json!(["capture_raw", "capture_raw"]);
    cases.push(c);
    let mut c = f.config();
    let x = c["policies"][0].clone();
    c["policies"].as_array_mut().ok_or("array")?.push(x);
    cases.push(c);
    let mut c = f.config();
    let x = c["action_rules"][0].clone();
    c["action_rules"].as_array_mut().ok_or("array")?.push(x);
    cases.push(c);
    let mut c = f.config();
    c["action_rules"] = json!(vec![c["action_rules"][0].clone(); 1025]);
    cases.push(c);
    for c in cases {
        assert!(f.load(&c).is_err());
    }
    let duplicate = f.0.join("duplicate.json");
    write_private(&duplicate, br#"{"schema_version":1,"schema_version":1}"#)?;
    assert!(load_private_intake_configuration(&duplicate).is_err());
    let large = f.0.join("large.json");
    write_private(&large, &vec![b' '; 256 * 1024 + 1])?;
    assert!(load_private_intake_configuration(&large).is_err());
    assert!(!f.0.join("native.sqlite").exists());
    Ok(())
}
#[test]
fn private_file_checks_cover_modes_symlinks_hardlinks_and_generic_errors() -> Result<(), Error> {
    let f = Fixture::new()?;
    let path = f.write(&f.config())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
    assert!(load_private_intake_configuration(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let link = f.0.join("config-link");
    symlink(&path, &link)?;
    assert!(load_private_intake_configuration(&link).is_err());
    let hard = f.0.join("config-hard");
    fs::hard_link(&path, &hard)?;
    assert!(load_private_intake_configuration(&path).is_err());
    fs::remove_file(hard)?;
    let token = f.0.join("intake-token");
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644))?;
    assert!(load_private_intake_configuration(&path).is_err());
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600))?;
    let tl = f.0.join("token-link");
    symlink(&token, &tl)?;
    let mut cfg = f.config();
    cfg["credentials"][0]["token_file"] = json!(tl);
    assert!(f.load(&cfg).is_err());
    let th = f.0.join("token-hard");
    fs::hard_link(&token, &th)?;
    assert!(load_private_intake_configuration(&path).is_err());
    fs::remove_file(th)?;
    let secret = f.0.join("SENSITIVE_PATH_DO_NOT_ECHO");
    write_private(&secret, b"{SENSITIVE_TOKEN_DO_NOT_ECHO")?;
    let error = load_private_intake_configuration(&secret)
        .err()
        .ok_or("expected failure")?;
    let text = format!("{error:?}:{error}");
    assert!(!text.contains("SENSITIVE"));
    Ok(())
}
#[test]
fn startup_rejects_owner_or_inbox_token_reuse_before_database_open() -> Result<(), Error> {
    let f = Fixture::new()?;
    let mut config = f.config();
    let ownerfile = f.0.join("owner-token");
    write_private(&ownerfile, OWNER.as_bytes())?;
    config["credentials"][0]["token_file"] = json!(ownerfile);
    let c = f.load(&config)?;
    assert!(
        open_native_services_with_intake(
            &f.0.join("native.sqlite"),
            &f.0.join("native.json"),
            OWNER,
            Some(c)
        )
        .is_err()
    );
    assert!(!f.0.join("native.sqlite").exists());
    let native = f.0.join("native-agents.json");
    write_private(
        &native,
        &serde_json::to_vec(
            &json!({"schema_version":1,"operator":null,"agents":[{"credential_id":"reader","token_file":f.0.join("intake-token"),"actor":"reader","grants":[{"scope":{"project_id":"allowed","recipient":"reader","context_epoch":"one"},"action":"read_pending","message_id":null,"source":null}]}]}),
        )?,
    )?;
    let c = f.load(&f.config())?;
    assert!(
        open_native_services_with_intake(&f.0.join("native.sqlite"), &native, OWNER, Some(c))
            .is_err()
    );
    assert!(!f.0.join("native.sqlite").exists());
    Ok(())
}
#[test]
fn static_snapshot_no_hot_reload_and_no_implicit_default_startup() -> Result<(), Error> {
    let f = Fixture::new()?;
    let p = f.write(&f.config())?;
    let c = load_private_intake_configuration(&p)?;
    fs::write(&p, b"invalid after startup")?;
    assert!(c.registry.authenticate(TOKEN).is_ok());
    assert!(load_private_intake_configuration(&p).is_err());
    let n = open_native_services_with_intake(
        &f.0.join("native.sqlite"),
        &f.0.join("native.json"),
        OWNER,
        None,
    )?;
    assert!(n.intake.is_none());
    assert!(n.native.store.snapshot()?.projection.tasks.is_empty());
    Ok(())
}

#[test]
fn origin_catalog_and_grant_rule_intersection_are_closed() -> Result<(), Error> {
    let f = Fixture::new()?;
    let mut cases = Vec::new();
    let mut c = f.config();
    c["origins"] = json!(["http://example.com:18890"]);
    cases.push(c);
    let mut c = f.config();
    c["origins"] = json!(vec!["http://127.0.0.1:18890"; 9]);
    cases.push(c);
    let mut c = f.config();
    c["credentials"][0]["grants"][0]["policy_id"] = json!("missing");
    cases.push(c);
    let mut c = f.config();
    c["credentials"][0]["grants"][0]["actions"] = json!(["preview_raw"]);
    cases.push(c);
    let mut c = f.config();
    let duplicate = c["credentials"][0]["grants"][0].clone();
    c["credentials"][0]["grants"]
        .as_array_mut()
        .ok_or("fixture grants")?
        .push(duplicate);
    cases.push(c);
    let mut c = f.config();
    let mut duplicate = c["credentials"][0].clone();
    duplicate["credential_id"] = json!("second-credential");
    c["credentials"]
        .as_array_mut()
        .ok_or("fixture credentials")?
        .push(duplicate);
    cases.push(c);
    for c in cases {
        assert!(f.load(&c).is_err());
    }
    Ok(())
}

async fn http(address: std::net::SocketAddr, path: &str, body: String) -> Result<Value, Error> {
    use std::io::Read;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(3))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
        write!(stream,"POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nAuthorization: Bearer {OWNER}\r\nX-Admiralty-Intake-Authorization: Bearer {TOKEN}\r\nX-Admiralty-Requested-With: intake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())?;
        let mut response = Vec::new(); stream.take(2*1024*1024+1).read_to_end(&mut response)?;
        if response.len()>2*1024*1024 { return Err("fixture response bound".into()); }
        let response = String::from_utf8(response)?;
        let (headers,body) = response.split_once("\r\n\r\n").ok_or("fixture HTTP framing")?;
        if headers.split_whitespace().nth(1) != Some("200") { return Err("fixture expected HTTP200".into()); }
        Ok(serde_json::from_str(body)?)
    }).await?
}

#[tokio::test]
async fn owner_file_loader_drives_actual_startup_http_preview_capture_and_replay()
-> Result<(), Error> {
    let f = Fixture::new()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let mut config = f.config();
    config["origins"] = json!([format!("http://{address}")]);
    let n = open_native_services_with_intake(
        &f.0.join("http.sqlite"),
        &f.0.join("native.json"),
        OWNER,
        Some(f.load(&config)?),
    )?;
    let store = n.native.store.clone();
    let app = admiralty_server::app_with_work_order_routes(
        n.native.store,
        admiralty_server::ServerConfig {
            listen: address,
            simulated: false,
            token: Some(OWNER.into()),
            assets: None,
            legacy: None,
            models_file: None,
        },
        Some(n.native.routes),
        n.intake,
    )?;
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Server(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    }));
    let i = input(
        IntakeFormat::PlainText,
        "Original request\n\nSecond paragraph".into(),
    )?;
    let body = json!({"project_id":i.project_id,"root_id":i.root_id,"format":i.format,"provenance":i.provenance,"source_payload":i.source_payload}).to_string();
    let wrapper = http(address, "/v2/intake/preview", body).await?;
    assert_eq!(wrapper["execution_granted"], false);
    let p: admiralty_core::intake::IntakePreview = serde_json::from_value(wrapper["data"].clone())?;
    assert_eq!(p.requests.len(), 1);
    let c = CaptureIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new("http-capture")?,
        capture_id: "http-original".into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: p.authorized_scope,
        format: i.format,
        provenance: i.provenance,
        source_payload: i.source_payload,
        preview_manifest_digest: p.manifest_digest,
        policy_revision: p.policy_revision,
        policy_digest: p.policy_digest,
    };
    let body = serde_json::to_string(&c)?;
    let recorded = http(address, "/v2/intake/captures", body.clone()).await?;
    assert_eq!(recorded["data"]["replayed"], false);
    assert_eq!(recorded["acceptance_granted"], false);
    assert_eq!(
        http(address, "/v2/intake/captures", body).await?["data"]["replayed"],
        true
    );
    assert!(store.snapshot()?.projection.tasks.is_empty());
    Ok(())
}
