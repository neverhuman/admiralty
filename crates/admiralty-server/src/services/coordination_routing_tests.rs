use super::*;
use crate::startup::{NativeServices, open_native_services};
use admiralty_core::{TaskContract, content_digest};
use admiralty_runtime::agent_tools::{AgentClient, ToolCall};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};

const OWNER: &str = "owner-fixture-token-0123456789abcdef";
const ADMIRAL: &str = "admiral-fixture-token-0123456789abcdef";
const CAPTAIN: &str = "captain-fixture-token-0123456789abcdef";
const READER: &str = "reader-fixture-token-0123456789abcdef";

struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
    database: PathBuf,
    contract: TaskContract,
}

fn private_file(path: &std::path::Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("private fixture file");
    file.write_all(bytes).expect("write fixture");
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("native-communication-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private root");
        let path = root.path().canonicalize().expect("absolute fixture path");
        let contract: TaskContract = serde_json::from_value(json!({
            "schema_version":1,"project_id":"admiralty","root_id":"release","task_id":"native-coordination",
            "revision":1,"title":"Native dogfood communication","objective":"Deliver exact task context and application receipts",
            "non_goals":["Provider execution"],"owned_paths":["src"],"forbidden_paths":[],"read_first":[],"dependencies":[],
            "criteria":[{"id":"C1","text":"Durable communication survives restart","check_ids":["check"]}],
            "checks":[{"id":"check","program":"true","args":[],"cwd":".","timeout_secs":1}],
            "authority_ref":"grant:native-coordination","max_attempts":1,"simulated":true
        })).expect("contract fixture");
        contract.validate().expect("valid fixture contract");
        let scope = |recipient: &str| json!({"project_id":"admiralty","recipient":recipient,"context_epoch":"release-1"});
        let route = |id: &str, recipient: &str| {
            json!({
                "subscription":{"subscription_id":id,"project_id":"admiralty","task_id":"native-coordination",
                    "recipient":recipient,"context_epoch":"release-1","channel":"release","contract_revision":1,
                    "contract_digest":content_digest(&contract).expect("contract digest"),"expires_at_utc":null},
                "authority_ref":"grant:native-coordination"
            })
        };
        let mut agents = Vec::new();
        for (actor, token, routes, writable) in [
            (
                "admiral",
                ADMIRAL,
                vec![route("captain-release", "captain")],
                true,
            ),
            (
                "captain",
                CAPTAIN,
                vec![route("admiral-release", "admiral")],
                true,
            ),
            ("reader", READER, vec![], false),
        ] {
            let token_file = path.join(format!("{actor}-token"));
            private_file(&token_file, token.as_bytes());
            let mut grants = vec![json!({"scope":scope(actor),"action":"read_pending"})];
            if writable {
                grants.push(json!({"scope":scope(actor),"action":"acknowledge_pending"}));
            }
            agents.push(json!({"credential_id":format!("{actor}-credential"),"actor":actor,"token_file":token_file,"grants":grants,"routes":routes}));
        }
        let config = path.join("native.json");
        private_file(&config, &serde_json::to_vec(&json!({"schema_version":1,"operator":{"principal":"owner","projects":["admiralty"]},"agents":agents})).expect("config JSON"));
        Self {
            database: path.join("native.sqlite"),
            config,
            contract,
            _root: root,
        }
    }

    fn open(&self) -> NativeServices {
        open_native_services(&self.database, &self.config, OWNER)
            .expect("actual protected native startup")
    }
}

struct Running {
    address: SocketAddr,
    store: Store,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn new(services: NativeServices) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let address = listener.local_addr().expect("address");
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, services.routes)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
                .expect("native HTTP fixture");
        });
        Self {
            address,
            store: services.store,
            stop,
            server,
        }
    }
    async fn close(self) {
        self.stop.send(()).expect("stop owned fixture");
        self.server.await.expect("server stopped");
        drop(self.store);
    }
    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let address = self.address;
        let token = token.to_owned();
        let call = ToolCall {
            name: name.into(),
            arguments,
        };
        tokio::task::spawn_blocking(move || {
            AgentClient::new(address, token)
                .expect("agent client")
                .call(&call)
                .expect("native agent response")
        })
        .await
        .expect("client thread")
    }
    async fn request(&self, token: Option<&str>, endpoint: &str, input: Value) -> (u16, String) {
        let address = self.address;
        let token = token.map(str::to_owned);
        let endpoint = endpoint.to_owned();
        tokio::task::spawn_blocking(move || {
            let body = serde_json::to_vec(&input).expect("request JSON");
            let bearer = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).expect("connect fixture");
            stream.set_read_timeout(Some(Duration::from_secs(5))).expect("read deadline");
            stream.set_write_timeout(Some(Duration::from_secs(5))).expect("write deadline");
            write!(stream, "POST {endpoint} HTTP/1.1\r\nHost: {address}\r\n{bearer}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).expect("write header");
            stream.write_all(&body).expect("write body");
            let mut response = String::new();
            stream.take(128 * 1024).read_to_string(&mut response).expect("bounded response");
            let status = response.split_whitespace().nth(1).expect("status").parse().expect("numeric status");
            (status,response)
        }).await.expect("request thread")
    }
}

fn assignment(fixture: &Fixture, request_id: &str) -> Value {
    json!({"request_id":request_id,"route_id":"captain-release","payload":{"kind":"task_assigned","task_id":"native-coordination","revision":1,"contract_digest":content_digest(&fixture.contract).expect("digest")}})
}
fn pending(actor: &str) -> Value {
    json!({"project_id":"admiralty","recipient":actor,"context_epoch":"release-1"})
}
fn ack(actor: &str, message: &str, request_id: &str) -> Value {
    let mut value = pending(actor);
    value["request_id"] = json!(request_id);
    value["message_id"] = json!(message);
    value
}
async fn submit(running: &Running, fixture: &Fixture) {
    let (status, _) = running
        .request(
            Some(OWNER),
            "/v2/tasks",
            json!({"request_id":Uuid::new_v4().to_string(),"contract":fixture.contract}),
        )
        .await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_native_captain_admiral_exchange_survives_restart_without_granting_execution() {
    let fixture = Fixture::new();
    let running = Running::new(fixture.open()).await;
    submit(&running, &fixture).await;
    let send = assignment(&fixture, &Uuid::new_v4().to_string());
    let first = running.tool(ADMIRAL, "message_send", send.clone()).await;
    assert_eq!(first["duplicate"], false);
    assert_eq!(first["execution_granted"], false);
    assert_eq!(first["acceptance_granted"], false);
    let message = first["message_id"].as_str().expect("message id").to_owned();
    // Simulate a lost response: retry the original request before observing it.
    let duplicate = running.tool(ADMIRAL, "message_send", send.clone()).await;
    assert_eq!(duplicate["event_seq"], first["event_seq"]);
    assert_eq!(duplicate["duplicate"], true);
    let page = running
        .tool(CAPTAIN, "inbox_pending", pending("captain"))
        .await;
    assert_eq!(page["messages"].as_array().expect("messages").len(), 1);
    assert_eq!(page["messages"][0]["sender"], "admiral");
    let context = running.tool(CAPTAIN, "task_context_get", json!({"project_id":"admiralty","task_id":"native-coordination","context_epoch":"release-1"})).await;
    assert_eq!(
        context["contract_digest"],
        content_digest(&fixture.contract).expect("digest")
    );
    running.close().await;

    let running = Running::new(fixture.open()).await;
    let page = running
        .tool(CAPTAIN, "inbox_pending", pending("captain"))
        .await;
    assert_eq!(page["messages"][0]["message_id"], message);
    let acknowledgment = ack("captain", &message, &Uuid::new_v4().to_string());
    let acked = running
        .tool(CAPTAIN, "inbox_ack_pending", acknowledgment.clone())
        .await;
    assert_eq!(acked["duplicate"], false);
    assert!(
        running
            .tool(CAPTAIN, "inbox_pending", pending("captain"))
            .await["messages"]
            .as_array()
            .expect("pending")
            .is_empty()
    );
    // Different principals can safely choose the same request UUID: both the
    // producer namespace and message ID retain the authenticated sender.
    let report = json!({"request_id":send["request_id"],"route_id":"admiral-release","causation_id":message,"payload":{"kind":"handoff","task_id":"native-coordination","candidate_ref":"commit:reviewed-candidate","failed_check":"CI is pending","next_action":"Verify exact source and all hosted checks before integration"}});
    running.tool(CAPTAIN, "message_send", report.clone()).await;
    let replies = running
        .tool(ADMIRAL, "inbox_pending", pending("admiral"))
        .await;
    assert_eq!(replies["messages"][0]["sender"], "captain");
    assert_ne!(replies["messages"][0]["message_id"], first["message_id"]);
    assert_eq!(
        replies["messages"][0]["payload"]["failed_check"],
        "CI is pending"
    );
    let snapshot = running.store.snapshot().expect("post exchange");
    assert_eq!(
        snapshot.projection.tasks[&fixture.contract.task_id].state,
        admiralty_core::SchedulingState::Ready
    );
    assert_eq!(snapshot.projection.messages.len(), 2);
    running.close().await;

    let running = Running::new(fixture.open()).await;
    assert_eq!(
        running.tool(ADMIRAL, "message_send", send).await["event_seq"],
        first["event_seq"]
    );
    let repeated = running
        .tool(CAPTAIN, "inbox_ack_pending", acknowledgment)
        .await;
    assert_eq!(repeated["duplicate"], true);
    assert_eq!(repeated["event_seq"], acked["event_seq"]);
    assert_eq!(
        running.tool(CAPTAIN, "message_send", report).await["duplicate"],
        true
    );
    assert_eq!(
        running.store.snapshot().expect("reopened projection"),
        snapshot
    );
    running.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_commands_reject_forged_routes_foreign_inboxes_and_read_only_acknowledgments() {
    let fixture = Fixture::new();
    let running = Running::new(fixture.open()).await;
    submit(&running, &fixture).await;
    let original = running.store.snapshot().expect("initial state");
    let input = assignment(&fixture, &Uuid::new_v4().to_string());
    for token in [
        None,
        Some(OWNER),
        Some("invalid-credential-0123456789abcdef"),
    ] {
        assert_eq!(
            running
                .request(token, "/v2/agents/inbox/send", json!({"invalid":true}))
                .await
                .0,
            401
        );
    }
    assert_eq!(
        running
            .request(Some(CAPTAIN), "/v2/agents/inbox/send", input.clone())
            .await
            .0,
        400
    );
    let mut forged = input.clone();
    forged["sender"] = json!("admiral");
    assert_eq!(
        running
            .request(Some(CAPTAIN), "/v2/agents/inbox/send", forged)
            .await
            .0,
        422
    );
    let mut foreign = input.clone();
    foreign["payload"]["task_id"] = json!("foreign-task");
    assert_eq!(
        running
            .request(Some(ADMIRAL), "/v2/agents/inbox/send", foreign)
            .await
            .0,
        400
    );
    let oversized = json!({"request_id":Uuid::new_v4().to_string(),"route_id":"captain-release","payload":{"kind":"handoff","task_id":"native-coordination","candidate_ref":"candidate","next_action":"x".repeat(17000)}});
    assert_eq!(
        running
            .request(Some(ADMIRAL), "/v2/agents/inbox/send", oversized)
            .await
            .0,
        413
    );
    assert_eq!(
        running
            .store
            .snapshot()
            .expect("rejections preserve domain state"),
        original
    );
    let sent = running.tool(ADMIRAL, "message_send", input).await;
    let message = sent["message_id"].as_str().expect("id");
    for (token, body) in [
        (
            ADMIRAL,
            ack("captain", message, &Uuid::new_v4().to_string()),
        ),
        (READER, ack("reader", message, &Uuid::new_v4().to_string())),
        (CAPTAIN, {
            let mut body = ack("captain", message, &Uuid::new_v4().to_string());
            body["context_epoch"] = json!("foreign-epoch");
            body
        }),
    ] {
        assert_eq!(
            running
                .request(Some(token), "/v2/agents/inbox/ack-pending", body)
                .await
                .0,
            400
        );
    }
    assert_eq!(
        running
            .tool(CAPTAIN, "inbox_pending", pending("captain"))
            .await["messages"]
            .as_array()
            .expect("pending")
            .len(),
        1
    );
    running.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_routing_retains_identity_collision_evidence() {
    let fixture = Fixture::new();
    let running = Running::new(fixture.open()).await;
    submit(&running, &fixture).await;
    let input = assignment(&fixture, &Uuid::new_v4().to_string());
    running.tool(ADMIRAL, "message_send", input.clone()).await;
    let mut collision = input.clone();
    collision["payload"] = json!({"kind":"handoff","task_id":"native-coordination","candidate_ref":"changed","next_action":"review"});
    assert_eq!(
        running
            .request(Some(ADMIRAL), "/v2/agents/inbox/send", collision)
            .await
            .0,
        409
    );
    assert_eq!(
        running
            .store
            .snapshot()
            .expect("collision evidence")
            .integrity_incidents,
        "1"
    );
    assert_eq!(
        running
            .store
            .snapshot()
            .expect("only one message")
            .projection
            .messages
            .len(),
        1
    );
    running.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_routing_rechecks_current_contract_before_registering_or_sending() {
    let fixture = Fixture::new();
    let running = Running::new(fixture.open()).await;
    submit(&running, &fixture).await;
    let input = assignment(&fixture, &Uuid::new_v4().to_string());
    let task = running
        .store
        .snapshot()
        .expect("task snapshot")
        .projection
        .tasks[&fixture.contract.task_id]
        .clone();
    let mut revised = fixture.contract.clone();
    revised.revision = 2;
    revised.title = "Revised contract".into();
    assert_eq!(running.request(Some(OWNER),"/v2/tasks/lifecycle",json!({"request_id":Uuid::new_v4().to_string(),"task_id":revised.task_id,"expected_task_digest":content_digest(&task).expect("task digest"),"action":{"action":"revise_contract","contract":revised}})).await.0,200);
    // The route was installed for revision 1; a current task at revision 2
    // cannot receive an assignment through it, even with valid credentials.
    assert_eq!(
        running
            .request(Some(ADMIRAL), "/v2/agents/inbox/send", input)
            .await
            .0,
        400
    );
    assert_eq!(
        running
            .store
            .snapshot()
            .expect("only one message")
            .projection
            .messages
            .len(),
        0
    );
    running.close().await;
}

#[test]
fn startup_rejects_cross_actor_pending_ack_grants_and_route_definition_collisions() {
    let fixture = Fixture::new();
    let valid = fixture.open();
    drop(valid);
    let original: Value =
        serde_json::from_slice(&fs::read(&fixture.config).expect("config")).expect("JSON");
    let mut cross_actor = original.clone();
    cross_actor["agents"][0]["grants"][1]["scope"]["recipient"] = json!("captain");
    fs::write(
        &fixture.config,
        serde_json::to_vec(&cross_actor).expect("JSON"),
    )
    .expect("rewrite owned fixture");
    assert!(open_native_services(&fixture.database, &fixture.config, OWNER).is_err());
    let mut duplicate = original.clone();
    duplicate["agents"][1]["routes"][0]["subscription"]["subscription_id"] =
        json!("captain-release");
    fs::write(
        &fixture.config,
        serde_json::to_vec(&duplicate).expect("JSON"),
    )
    .expect("rewrite owned fixture");
    assert!(open_native_services(&fixture.database, &fixture.config, OWNER).is_err());
    fs::write(
        &fixture.config,
        serde_json::to_vec(&original).expect("JSON"),
    )
    .expect("restore owned fixture");
    let store = fixture.open();
    drop(store);
}

#[test]
fn routing_rejects_old_registry_handles_expiry_and_revoked_exact_replays() {
    use crate::services::coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialGrant,
    };
    let fixture = Fixture::new();
    let registry = || {
        Arc::new(RwLock::new(
            CoordinationCredentialAuthority::new(vec![CoordinationCredential::configured(
                "admiral-credential".into(),
                ADMIRAL.into(),
                admiralty_core::PrincipalId::new("admiral").expect("actor"),
                vec![CoordinationCredentialGrant {
                    scope: serde_json::from_value(pending("admiral")).expect("scope"),
                    action: CoordinationAction::ReadPending,
                    message_id: None,
                    source: None,
                }],
            )])
            .expect("registry"),
        ))
    };
    let old_registry = registry();
    let old_actor = old_registry
        .try_read()
        .expect("registry")
        .authenticate(ADMIRAL)
        .expect("old actor");
    let credentials = registry();
    let configuration: Value =
        serde_json::from_slice(&fs::read(&fixture.config).expect("config")).expect("JSON");
    let route = serde_json::from_value(configuration["agents"][0]["routes"][0].clone())
        .expect("configured route");
    let routing = Arc::new(
        NativeRoutingAuthority::new(
            credentials.clone(),
            vec![InstalledRoute {
                credential_id: "admiral-credential".into(),
                route,
            }],
        )
        .expect("routing policy"),
    );
    let store = Store::open_with_startup_authorities(
        &fixture.database,
        admiralty_store::StoreAuthorities {
            subscriptions: Some(routing.clone()),
            ..Default::default()
        },
    )
    .expect("protected Store");
    store
        .append(
            SourceIdentity {
                producer_id: "fixture".into(),
                incarnation: "initial".into(),
                sequence: "1".into(),
            },
            admiralty_core::DomainEvent::TaskSubmitted {
                contract: fixture.contract.clone(),
            },
            "2026-10-19T00:00:00Z".into(),
        )
        .expect("fixture task");
    let input = assignment(&fixture, &Uuid::new_v4().to_string());
    assert!(
        routing
            .send(
                &store,
                &old_actor,
                serde_json::from_value(input.clone()).expect("request")
            )
            .is_err()
    );
    let actor = credentials
        .try_read()
        .expect("registry")
        .authenticate(ADMIRAL)
        .expect("current actor");
    routing
        .send(
            &store,
            &actor,
            serde_json::from_value(input.clone()).expect("request"),
        )
        .expect("authorized send");
    let before = store.snapshot().expect("queued");
    let task = &before.projection.tasks[&fixture.contract.task_id];
    let proof = RoutingProof {
        actor: actor.clone(),
    };
    let current_utc = now().expect("fixture clock");
    assert!(routing.authorize(
        &routing.routes[0].route.subscription,
        SubscriptionAction::Subscribe,
        current_utc,
        task,
        &proof
    ));
    let mut expired_route = routing.routes[0].route.clone();
    expired_route.subscription.expires_at_utc = Some(current_utc - 1);
    let expired = NativeRoutingAuthority::new(
        credentials.clone(),
        vec![InstalledRoute {
            credential_id: "admiral-credential".into(),
            route: expired_route,
        }],
    )
    .expect("expired policy remains inert");
    // A recently captured operation time cannot extend an expired route.
    assert!(!expired.authorize(
        &expired.routes[0].route.subscription,
        SubscriptionAction::Subscribe,
        current_utc - 2,
        task,
        &proof
    ));
    credentials
        .try_write()
        .expect("exclusive revocation")
        .revoke("admiral-credential")
        .expect("revoke");
    assert!(
        routing
            .send(
                &store,
                &actor,
                serde_json::from_value(input).expect("request")
            )
            .is_err()
    );
    assert_eq!(store.snapshot().expect("no new event"), before);
}
