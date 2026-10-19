#![cfg(unix)]
use admiralty_core::intake::{
    IntakeFormat, IntakePolicy, IntakePreview, RequesterAttribution, SourcePayload,
    SourceProvenance,
};
use admiralty_server::{
    ServerConfig, app_with_work_order_routes,
    services::{
        work_order_intake::{
            AuthorizationStage, IntakeScopeGrant, WorkOrderIntakeAction,
            WorkOrderIntakeAuthRegistry, WorkOrderIntakeBinding, WorkOrderIntakeCredential,
            WorkOrderIntakePolicy,
        },
        work_order_routes::ExactLoopbackOrigins,
    },
    startup::{TrustedIntakeComposition, open_native_services_with_intake},
};
use admiralty_store::intake_capture::CaptureIntakeRequest;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const OWNER: &str = "startup-owner-012345678901234567890123456789";
const INTAKE: &str = "startup-intake-012345678901234567890123456789";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Result<Self, Error> {
        let root = std::env::temp_dir()
            .canonicalize()?
            .join(format!("intake-startup-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let mut config = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("native.json"))?;
        config.write_all(br#"{"schema_version":1,"operator":{"principal":"owner","projects":["allowed"]},"agents":[]}"#)?;
        Ok(Self(root))
    }
    fn config(&self) -> PathBuf {
        self.0.join("native.json")
    }
    fn database(&self) -> PathBuf {
        self.0.join("native.sqlite")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Server {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    fn new(listener: tokio::net::TcpListener, app: axum::Router) -> Result<Self, Error> {
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { address, task })
    }
    async fn call(&self, path: &str, auth: &str, body: &str) -> Result<(u16, String), Error> {
        http_call(self.address, path, auth, body).await
    }
}
async fn http_call(
    address: SocketAddr,
    path: &str,
    auth: &str,
    body: &str,
) -> Result<(u16, String), Error> {
    let (path, auth, body) = (path.to_owned(), auth.to_owned(), body.to_owned());
    tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            write!(stream,"POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://{address}\r\nX-Admiralty-Requested-With: intake\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())?;
            let mut response = Vec::new();
            stream.take(2*1024*1024+1).read_to_end(&mut response)?;
            if response.len()>2*1024*1024 { return Err("bounded response exceeded".into()); }
            let response=String::from_utf8(response)?;
            let (headers,body)=response.split_once("\r\n\r\n").ok_or("missing response separator")?;
            let status=headers.split_whitespace().nth(1).ok_or("missing response status")?.parse()?;
            Ok((status,body.to_owned()))
        }).await?
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn config(address: SocketAddr) -> ServerConfig {
    ServerConfig {
        listen: address,
        simulated: false,
        token: Some(OWNER.into()),
        assets: None,
        legacy: None,
        models_file: None,
    }
}
fn auth() -> String {
    format!(
        "Authorization: Bearer {OWNER}\r\nX-Admiralty-Intake-Authorization: Bearer {INTAKE}\r\n"
    )
}

#[derive(Default)]
struct Policy {
    writer_turns: AtomicUsize,
}
impl WorkOrderIntakePolicy for Policy {
    fn authorize_current(&self, binding: &WorkOrderIntakeBinding<'_>) -> bool {
        if binding.stage == AuthorizationStage::WriterTurn {
            self.writer_turns.fetch_add(1, Ordering::SeqCst);
        }
        binding.authenticated_actor.as_str() == "submitter"
            && binding
                .scope
                .project_id
                .as_ref()
                .is_some_and(|v| v.as_str() == "allowed")
            && binding
                .scope
                .root_id
                .as_ref()
                .is_some_and(|v| v.as_str() == "root")
    }
}
fn composition(
    address: SocketAddr,
    policy: Arc<Policy>,
) -> Result<TrustedIntakeComposition, Error> {
    composition_with_token(address, policy, INTAKE)
}
fn composition_with_token(
    address: SocketAddr,
    policy: Arc<Policy>,
    token: &str,
) -> Result<TrustedIntakeComposition, Error> {
    let registry = Arc::new(WorkOrderIntakeAuthRegistry::new(vec![
        WorkOrderIntakeCredential::configured(
            "intake-only".into(),
            token.into(),
            admiralty_core::PrincipalId::new("submitter")?,
            vec![IntakeScopeGrant {
                project_id: admiralty_core::ProjectId::new("allowed")?,
                root_id: admiralty_core::RootId::new("root")?,
                authority_ref: "intake-grant".into(),
                authority_generation: 1,
                policy: IntakePolicy::default(),
                allowed_actions: [
                    WorkOrderIntakeAction::PreviewRaw,
                    WorkOrderIntakeAction::CaptureRaw,
                    WorkOrderIntakeAction::ReadCaptured,
                ]
                .into_iter()
                .collect(),
            }],
        ),
    ])?);
    Ok(TrustedIntakeComposition {
        registry,
        policy,
        origins: ExactLoopbackOrigins::new(vec![format!("http://{address}")])?,
    })
}

#[tokio::test]
async fn existing_startup_and_default_router_never_mint_intake_authority() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let services =
        open_native_services_with_intake(&fixture.database(), &fixture.config(), OWNER, None)?;
    assert!(services.intake.is_none());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = Server::new(
        listener,
        app_with_work_order_routes(
            services.native.store.clone(),
            config(address),
            Some(services.native.routes),
            None,
        )?,
    )?;
    // Private transport authentication precedes malformed JSON or intake parsing.
    assert_eq!(server.call("/v2/intake/preview", "", "{").await?.0, 401);
    assert_eq!(
        server.call("/v2/intake/preview", &auth(), "{").await?.0,
        503
    );
    assert!(
        services
            .native
            .store
            .snapshot()?
            .projection
            .tasks
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn trusted_startup_pairs_writer_policies_and_keeps_transport_and_scope_independent()
-> Result<(), Error> {
    let fixture = Fixture::new()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let policy = Arc::new(Policy::default());
    let services = open_native_services_with_intake(
        &fixture.database(),
        &fixture.config(),
        OWNER,
        Some(composition(address, policy.clone())?),
    )?;
    let server = Server::new(
        listener,
        app_with_work_order_routes(
            services.native.store.clone(),
            config(address),
            Some(services.native.routes),
            services.intake,
        )?,
    )?;
    let provenance = SourceProvenance {
        schema_version: 1,
        source_id: "original-request".into(),
        revision: "one".into(),
        captured_at: "2026-10-19T05:40:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    };
    let source_payload = SourcePayload::InlineUtf8 {
        text: "One request\n\n第二段 remains in that request.".into(),
    };
    let preview_input=json!({"project_id":"allowed","root_id":"root","format":IntakeFormat::PlainText,"provenance":provenance,"source_payload":source_payload}).to_string();
    let owner_only = format!("Authorization: Bearer {OWNER}\r\n");
    assert_eq!(
        server
            .call("/v2/intake/preview", &owner_only, &preview_input)
            .await?
            .0,
        401
    );
    let intake_only = format!(
        "Authorization: Bearer {INTAKE}\r\nX-Admiralty-Intake-Authorization: Bearer {INTAKE}\r\n"
    );
    assert_eq!(
        server
            .call("/v2/intake/preview", &intake_only, &preview_input)
            .await?
            .0,
        401
    );
    let duplicate_auth = format!("{}Authorization: Bearer invalid\r\n", auth());
    assert_eq!(
        server
            .call("/v2/intake/preview", &duplicate_auth, &preview_input)
            .await?
            .0,
        401
    );
    let response = server
        .call("/v2/intake/preview", &auth(), &preview_input)
        .await?;
    assert_eq!(response.0, 200, "{}", response.1);
    let wrapper: Value = serde_json::from_str(&response.1)?;
    assert_eq!(wrapper["execution_granted"], false);
    assert_eq!(wrapper["acceptance_granted"], false);
    let preview: IntakePreview = serde_json::from_value(wrapper["data"].clone())?;
    assert_eq!(preview.requests.len(), 1);
    let request = CaptureIntakeRequest {
        schema_version: 1,
        command_id: admiralty_core::CommandId::new("capture-once")?,
        capture_id: "original".into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: preview.authorized_scope,
        format: IntakeFormat::PlainText,
        provenance,
        source_payload,
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
    };
    let body = serde_json::to_string(&request)?;
    let response = server.call("/v2/intake/captures", &auth(), &body).await?;
    assert_eq!(response.0, 200, "{}", response.1);
    let recorded: Value = serde_json::from_str(&response.1)?;
    assert_eq!(recorded["data"]["replayed"], false);
    let replay = server.call("/v2/intake/captures", &auth(), &body).await?;
    assert_eq!(replay.0, 200, "{}", replay.1);
    assert_eq!(
        serde_json::from_str::<Value>(&replay.1)?["data"]["replayed"],
        true
    );
    assert!(policy.writer_turns.load(Ordering::SeqCst) >= 2);
    assert!(
        services
            .native
            .store
            .snapshot()?
            .projection
            .tasks
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn trusted_intake_cannot_be_exposed_by_demo_or_remote_bind() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let address = "127.0.0.1:18890".parse()?;
    let services = open_native_services_with_intake(
        &fixture.database(),
        &fixture.config(),
        OWNER,
        Some(composition(address, Arc::new(Policy::default()))?),
    )?;
    let mut demo = config(address);
    demo.simulated = true;
    assert!(
        app_with_work_order_routes(
            services.native.store.clone(),
            demo,
            None,
            services.intake.clone()
        )
        .is_err()
    );
    let mut remote = config(address);
    remote.listen = "0.0.0.0:18890".parse()?;
    assert!(
        app_with_work_order_routes(services.native.store, remote, None, services.intake).is_err()
    );
    Ok(())
}

#[test]
fn intake_credentials_cannot_reuse_owner_or_inbox_before_database_open() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let address = "127.0.0.1:18890".parse()?;
    let reused_owner = composition_with_token(address, Arc::new(Policy::default()), OWNER)?;
    assert!(
        open_native_services_with_intake(
            &fixture.database(),
            &fixture.config(),
            OWNER,
            Some(reused_owner)
        )
        .is_err()
    );
    assert!(!fixture.database().exists());
    let token_path = fixture.0.join("agent-token");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&token_path)?
        .write_all(INTAKE.as_bytes())?;
    fs::write(
        fixture.config(),
        serde_json::to_vec(
            &json!({"schema_version":1,"operator":{"principal":"owner","projects":["allowed"]},"agents":[{
                "credential_id":"reader-only","token_file":token_path,"actor":"reader","grants":[{
                    "scope":{"project_id":"allowed","recipient":"reader","context_epoch":"epoch1"},"action":"read_pending"
                }]
            }]}),
        )?,
    )?;
    let reused_inbox = composition(address, Arc::new(Policy::default()))?;
    assert!(
        open_native_services_with_intake(
            &fixture.database(),
            &fixture.config(),
            OWNER,
            Some(reused_inbox)
        )
        .is_err()
    );
    assert!(!fixture.database().exists());
    Ok(())
}

struct OwnedServerChild {
    child: Child,
    stderr_reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for OwnedServerChild {
    fn drop(&mut self) {
        // This child was created by this test and has no provider/process launcher.
        // Never address a service, process group or another session by name.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

#[tokio::test]
async fn actual_native_binary_loads_private_example_and_serves_scoped_capture() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    let write_private = |name: &str, bytes: &[u8]| -> Result<PathBuf, Error> {
        let path = fixture.0.join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .write_all(bytes)?;
        Ok(path)
    };
    let owner_path = write_private("owner-token", OWNER.as_bytes())?;
    let intake_token = write_private("intake-token", INTAKE.as_bytes())?;
    // Reserve an exact origin for the explicit startup configuration. A bind
    // race fails this isolated test; it never takes control of the other binder.
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = reserved.local_addr()?;
    let mut intake: Value =
        serde_json::from_str(include_str!("../../../configs/examples/intake.json"))?;
    intake["origins"] = json!([format!("http://{address}")]);
    intake["credentials"][0]["token_file"] = json!(intake_token);
    let intake_path = write_private("intake.json", &serde_json::to_vec(&intake)?)?;
    drop(reserved);
    let child = Command::new(env!("CARGO_BIN_EXE_admiraltyd"))
        .env_clear()
        .env("TMPDIR", &fixture.0)
        .arg("--database")
        .arg(fixture.database())
        .arg("--token-file")
        .arg(owner_path)
        .arg("--native-config")
        .arg(fixture.config())
        .arg("--intake-config")
        .arg(intake_path)
        .arg("--listen")
        .arg(address.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child = OwnedServerChild {
        child,
        stderr_reader: None,
    };
    let stderr = child
        .child
        .stderr
        .take()
        .ok_or("missing owned child stderr")?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    child.stderr_reader = Some(std::thread::spawn(move || {
        // Bound diagnostics even if startup fails. No credential values are
        // passed in argv or printed by the service's typed configuration errors.
        for line in BufReader::new(stderr).take(16 * 1024).lines() {
            match line {
                Ok(line) if line.starts_with("Admiralty listening on http://") => {
                    let _ = send.send(());
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }));
    receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "owned native binary did not become ready")?;
    let provenance = SourceProvenance {
        schema_version: 1,
        source_id: "actual-cli-request".into(),
        revision: "one".into(),
        captured_at: "2026-10-19T05:50:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    };
    let source_payload = SourcePayload::InlineUtf8 {
        text: "Keep the original request.\n\nKeep its second paragraph.".into(),
    };
    let request=json!({"project_id":"my-project","root_id":"my-root","format":"plain_text","provenance":provenance,"source_payload":source_payload}).to_string();
    let response = http_call(address, "/v2/intake/preview", &auth(), &request).await?;
    assert_eq!(response.0, 200, "{}", response.1);
    let wrapper: Value = serde_json::from_str(&response.1)?;
    let preview: IntakePreview = serde_json::from_value(wrapper["data"].clone())?;
    assert_eq!(preview.authorized_scope.principal.as_str(), "operator");
    assert_eq!(preview.requests.len(), 1);
    let request = CaptureIntakeRequest {
        schema_version: 1,
        command_id: admiralty_core::CommandId::new("actual-cli-capture")?,
        capture_id: "actual-original".into(),
        revision: 1,
        expected_revision: 0,
        parent_receipt_digest: None,
        scope: preview.authorized_scope,
        format: IntakeFormat::PlainText,
        provenance,
        source_payload,
        preview_manifest_digest: preview.manifest_digest,
        policy_revision: preview.policy_revision,
        policy_digest: preview.policy_digest,
    };
    let response = http_call(
        address,
        "/v2/intake/captures",
        &auth(),
        &serde_json::to_string(&request)?,
    )
    .await?;
    assert_eq!(response.0, 200, "{}", response.1);
    let wrapper: Value = serde_json::from_str(&response.1)?;
    assert_eq!(wrapper["execution_granted"], false);
    assert_eq!(wrapper["acceptance_granted"], false);
    assert_eq!(wrapper["data"]["replayed"], false);
    let response = http_call(
        address,
        "/v2/intake/captures/actual-original",
        &auth(),
        &json!({"project_id":"my-project","root_id":"my-root","revision":1}).to_string(),
    )
    .await?;
    assert_eq!(response.0, 200, "{}", response.1);
    assert_eq!(
        serde_json::from_str::<Value>(&response.1)?["data"]["request"]["source_payload"],
        serde_json::to_value(&request.source_payload)?
    );
    drop(child);
    assert!(
        admiralty_store::Store::open(fixture.database())?
            .snapshot()?
            .projection
            .tasks
            .is_empty()
    );
    Ok(())
}
