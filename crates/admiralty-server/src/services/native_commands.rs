//! Authenticated, owner-scoped HTTP commands for native task intake and
//! pending-task lifecycle changes. This module deliberately exposes no generic
//! event append or execution/acceptance operation.

use admiralty_core::{
    DomainEvent, LifecycleAction, LifecycleCommand, PrincipalId, ProjectId, SourceIdentity,
    TaskContract, TaskId, TaskView, limits::bounded_json_size,
};
use admiralty_store::{LifecycleAuthority, LifecycleEffect, Store};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_CONCURRENT_COMMANDS: usize = 8;
const SOURCE_PREFIX: &str = "native-operator-http-";

#[derive(Clone, Debug, Serialize)]
struct CommandReceipt {
    event_seq: String,
    source_contiguous_seq: String,
    duplicate: bool,
    execution_granted: bool,
    acceptance_granted: bool,
}

#[derive(Serialize)]
struct PublicError {
    error: &'static str,
}

fn public_error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(PublicError { error: message })).into_response()
}

#[derive(Debug)]
pub enum NativeCommandConfigError {
    InvalidToken,
    EmptyProjectGrant,
}

impl std::fmt::Display for NativeCommandConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidToken => "invalid native command token configuration",
            Self::EmptyProjectGrant => "native command project grant is empty",
        })
    }
}

impl std::error::Error for NativeCommandConfigError {}

/// The only lifecycle authority this HTTP surface installs. Trusted startup
/// constructs it before opening the Store with
/// `Store::open_with_lifecycle_authority`.
pub struct OperatorLifecycleAuthority {
    owner: PrincipalId,
    allowed_projects: BTreeSet<ProjectId>,
    producer_id: String,
}

impl OperatorLifecycleAuthority {
    pub fn new(
        owner: PrincipalId,
        allowed_projects: BTreeSet<ProjectId>,
    ) -> Result<Self, NativeCommandConfigError> {
        if allowed_projects.is_empty() {
            return Err(NativeCommandConfigError::EmptyProjectGrant);
        }
        Ok(Self {
            producer_id: source_producer(&owner),
            owner,
            allowed_projects,
        })
    }
}

impl LifecycleAuthority for OperatorLifecycleAuthority {
    fn authorize(
        &self,
        source: &SourceIdentity,
        command: &LifecycleCommand,
        task: &TaskView,
        effect: Option<&LifecycleEffect<'_>>,
    ) -> bool {
        if source.producer_id != self.producer_id
            || source.sequence != "1"
            || !is_canonical_uuid(&source.incarnation)
            || command.actor != self.owner
            || !self.allowed_projects.contains(&task.contract.project_id)
            || effect.is_some()
        {
            return false;
        }
        match &command.action {
            // Authorize the operation's identity and scope, not its transition
            // against the current state. The Store must be able to return the
            // exact retained receipt after a lost response, even if this task
            // has since advanced. New events still pass the reducer's CAS and
            // pending/unstarted guards; changed source payloads fail replay.
            LifecycleAction::CancelPending { .. } => true,
            LifecycleAction::ReviseContract { contract } => {
                contract.project_id == task.contract.project_id
                    && self.allowed_projects.contains(&contract.project_id)
            }
            LifecycleAction::DecideCandidate { .. }
            | LifecycleAction::CloseAttempt { .. }
            | LifecycleAction::Retry { .. } => false,
        }
    }
}

struct ProtectedToken(Vec<u8>);

impl Drop for ProtectedToken {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

struct CommandState {
    store: Store,
    token: ProtectedToken,
    owner: PrincipalId,
    allowed_projects: BTreeSet<ProjectId>,
    command_slots: Arc<Semaphore>,
}

/// A configured standalone router. The token is never formatted, serialized,
/// or included in an error; its owning allocation is cleared on drop.
pub struct NativeCommands {
    state: Arc<CommandState>,
}

impl NativeCommands {
    pub fn new(
        store: Store,
        owner_token: String,
        owner: PrincipalId,
        allowed_projects: BTreeSet<ProjectId>,
    ) -> Result<Self, NativeCommandConfigError> {
        let mut token = owner_token.into_bytes();
        if !(32..=MAX_TOKEN_BYTES).contains(&token.len())
            || !token.iter().all(|byte| (b'!'..=b'~').contains(byte))
        {
            token.fill(0);
            return Err(NativeCommandConfigError::InvalidToken);
        }
        if allowed_projects.is_empty() {
            token.fill(0);
            return Err(NativeCommandConfigError::EmptyProjectGrant);
        }
        Ok(Self {
            state: Arc::new(CommandState {
                store,
                token: ProtectedToken(token),
                owner,
                allowed_projects,
                command_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_COMMANDS)),
            }),
        })
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/v2/tasks", post(submit_task))
            .route("/v2/tasks/lifecycle", post(change_lifecycle))
            .with_state(Arc::clone(&self.state))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitTaskRequest {
    request_id: String,
    contract: TaskContract,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum PendingMutation {
    CancelPending { reason: String },
    ReviseContract { contract: Box<TaskContract> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleRequest {
    request_id: String,
    task_id: TaskId,
    expected_task_digest: String,
    action: PendingMutation,
}

async fn submit_task(State(state): State<Arc<CommandState>>, request: Request<Body>) -> Response {
    if !authenticated(&state, request.headers()) {
        return public_error(StatusCode::UNAUTHORIZED, "owner authentication required");
    }
    let _slot = match state.command_slots.clone().try_acquire_owned() {
        Ok(slot) => slot,
        Err(_) => return public_error(StatusCode::SERVICE_UNAVAILABLE, "command capacity reached"),
    };
    let bytes = match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return public_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "command body is invalid or too large",
            );
        }
    };
    let input: SubmitTaskRequest = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => return public_error(StatusCode::BAD_REQUEST, "command body is invalid"),
    };
    let request_id = match canonical_request_id(&input.request_id) {
        Some(request_id) => request_id,
        None => {
            return public_error(
                StatusCode::BAD_REQUEST,
                "request_id must be a canonical UUID",
            );
        }
    };
    if !state.allowed_projects.contains(&input.contract.project_id)
        || input.contract.validate().is_err()
        || bounded_json_size(&input.contract, MAX_BODY_BYTES).is_err()
    {
        return public_error(
            StatusCode::FORBIDDEN,
            "task contract is outside the owner grant",
        );
    }
    let source = source_for_request(&state.owner, request_id);
    let observed_utc = utc_timestamp();
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        // A disconnected HTTP caller does not cancel an admitted Store turn.
        // Retain its capacity until the blocking operation actually finishes.
        let _slot = _slot;
        store.append(
            source,
            DomainEvent::TaskSubmitted {
                contract: input.contract,
            },
            observed_utc,
        )
    })
    .await
    {
        Ok(Ok(receipt)) => Json(CommandReceipt {
            event_seq: receipt.event_seq,
            source_contiguous_seq: receipt.source_contiguous_seq,
            duplicate: receipt.duplicate,
            execution_granted: false,
            acceptance_granted: false,
        })
        .into_response(),
        Ok(Err(_)) => public_error(StatusCode::CONFLICT, "task command was not applied"),
        Err(_) => public_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "task command is unavailable",
        ),
    }
}

async fn change_lifecycle(
    State(state): State<Arc<CommandState>>,
    request: Request<Body>,
) -> Response {
    if !authenticated(&state, request.headers()) {
        return public_error(StatusCode::UNAUTHORIZED, "owner authentication required");
    }
    let _slot = match state.command_slots.clone().try_acquire_owned() {
        Ok(slot) => slot,
        Err(_) => return public_error(StatusCode::SERVICE_UNAVAILABLE, "command capacity reached"),
    };
    let bytes = match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return public_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "command body is invalid or too large",
            );
        }
    };
    let input: LifecycleRequest = match serde_json::from_slice(&bytes) {
        Ok(input) => input,
        Err(_) => return public_error(StatusCode::BAD_REQUEST, "command body is invalid"),
    };
    let request_id = match canonical_request_id(&input.request_id) {
        Some(request_id) => request_id,
        None => {
            return public_error(
                StatusCode::BAD_REQUEST,
                "request_id must be a canonical UUID",
            );
        }
    };
    if !is_digest(&input.expected_task_digest) {
        return public_error(
            StatusCode::BAD_REQUEST,
            "expected_task_digest must be a SHA-256 digest",
        );
    }
    let action = match input.action {
        PendingMutation::CancelPending { reason } => {
            if reason.is_empty() || reason.len() > 512 || reason.chars().any(char::is_control) {
                return public_error(
                    StatusCode::BAD_REQUEST,
                    "pending cancellation reason is invalid",
                );
            }
            LifecycleAction::CancelPending { reason }
        }
        PendingMutation::ReviseContract { contract } => {
            if contract.task_id != input.task_id
                || !state.allowed_projects.contains(&contract.project_id)
                || contract.validate().is_err()
                || bounded_json_size(&contract, MAX_BODY_BYTES).is_err()
            {
                return public_error(
                    StatusCode::FORBIDDEN,
                    "contract revision is outside the owner grant",
                );
            }
            LifecycleAction::ReviseContract {
                contract: *contract,
            }
        }
    };
    let command = LifecycleCommand {
        protocol_version: 2,
        task_id: input.task_id,
        expected_task_digest: input.expected_task_digest,
        actor: state.owner.clone(),
        action,
    };
    let source = source_for_request(&state.owner, request_id);
    let observed_utc = utc_timestamp();
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        store.execute_lifecycle(source, command, observed_utc, None)
    })
    .await
    {
        Ok(Ok(receipt)) => Json(CommandReceipt {
            event_seq: receipt.event_seq,
            source_contiguous_seq: receipt.source_contiguous_seq,
            duplicate: receipt.duplicate,
            execution_granted: false,
            acceptance_granted: false,
        })
        .into_response(),
        Ok(Err(_)) => public_error(StatusCode::CONFLICT, "lifecycle command was not applied"),
        Err(_) => public_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "lifecycle command is unavailable",
        ),
    }
}

fn authenticated(state: &CommandState, headers: &HeaderMap) -> bool {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return false;
    }
    let Some(candidate) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::as_bytes)
    else {
        return false;
    };
    if candidate.len() > MAX_TOKEN_BYTES {
        return false;
    }
    constant_time_equal(&state.token.0, candidate)
}

fn constant_time_equal(expected: &[u8], candidate: &[u8]) -> bool {
    let mut difference = expected.len() ^ candidate.len();
    for index in 0..expected.len().max(candidate.len()) {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ candidate.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn source_producer(owner: &PrincipalId) -> String {
    format!("{SOURCE_PREFIX}{}", owner.as_str())
}

fn source_for_request(owner: &PrincipalId, request_id: Uuid) -> SourceIdentity {
    SourceIdentity {
        producer_id: source_producer(owner),
        incarnation: request_id.to_string(),
        sequence: "1".into(),
    }
}

fn canonical_request_id(value: &str) -> Option<Uuid> {
    let parsed = Uuid::parse_str(value).ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn is_canonical_uuid(value: &str) -> bool {
    canonical_request_id(value).is_some()
}

fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn utc_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs();
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z",
        nanos = duration.subsec_nanos()
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, RootId, SchedulingState, content_digest};
    use serde_json::{Value, json};
    use std::{
        fs,
        io::{BufRead, BufReader, Read, Write},
        net::{SocketAddr, TcpStream},
        os::unix::fs::DirBuilderExt,
        path::PathBuf,
        time::Duration,
    };

    const OWNER_TOKEN: &str = "round22-test-owner-token-0123456789abcdef";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("native-commands-{}", Uuid::new_v4()));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .expect("create private test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct TestServer {
        address: SocketAddr,
        task: tokio::task::JoinHandle<()>,
    }

    impl TestServer {
        async fn start(router: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local test server");
            let address = listener.local_addr().expect("test server address");
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            });
            Self { address, task }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn contract(project: &str) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new(project).expect("test project"),
            root_id: RootId::new("root-a").expect("test root"),
            task_id: TaskId::new("task-a").expect("test task"),
            revision: 1,
            title: "Native operator intake".into(),
            objective: "Persist an owner-authorized pending task".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "check-passes".into(),
                text: "The declared check succeeds".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "cargo".into(),
                args: vec!["check".into()],
                cwd: ".".into(),
                timeout_secs: 60,
            }],
            authority_ref: "owner-authority".into(),
            max_attempts: 1,
            simulated: false,
        }
    }

    fn service(directory: &TestDirectory) -> (Router, Store, BTreeSet<ProjectId>) {
        let owner = PrincipalId::new("configured-owner").expect("test owner");
        let project = ProjectId::new("allowed-project").expect("allowed project");
        let projects = BTreeSet::from([project]);
        let authority = Arc::new(
            OperatorLifecycleAuthority::new(owner.clone(), projects.clone())
                .expect("configured lifecycle authority"),
        );
        let store =
            Store::open_with_lifecycle_authority(directory.0.join("store.sqlite"), authority)
                .expect("open authorized test store");
        let commands =
            NativeCommands::new(store.clone(), OWNER_TOKEN.into(), owner, projects.clone())
                .expect("configured native router");
        (commands.router(), store, projects)
    }

    struct TestResponse {
        status: StatusCode,
        body: Value,
    }

    // HTTP completion is the declared body length, not a subsequent TCP FIN.
    // A server rejecting an unread request body may reset after a full response.
    fn read_local_response(reader: impl BufRead) -> std::io::Result<TestResponse> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid bounded fixture HTTP response",
            )
        };
        let mut reader = reader.take(64 * 1024);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || !line.ends_with("\r\n") {
                return Err(invalid());
            }
            head.push_str(&line);
            if head.len() > 16 * 1024 {
                return Err(invalid());
            }
            if line == "\r\n" {
                break;
            }
        }
        let status = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u16>().ok())
            .and_then(|v| StatusCode::from_u16(v).ok())
            .ok_or_else(invalid)?;
        let mut lengths = Vec::new();
        for line in head.lines().skip(1).filter(|l| !l.is_empty()) {
            let (name, value) = line.split_once(':').ok_or_else(invalid)?;
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err(invalid());
            }
            if name.eq_ignore_ascii_case("content-length") {
                let value = value.trim();
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                lengths.push(value.parse::<usize>().map_err(|_| invalid())?);
            }
        }
        if lengths.len() != 1 || lengths[0] > 64 * 1024 - head.len() {
            return Err(invalid());
        }
        let mut bytes = vec![0; lengths[0]];
        reader.read_exact(&mut bytes)?;
        let body = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        Ok(TestResponse { status, body })
    }

    #[test]
    fn fixture_http_accepts_complete_bodies_before_reset_and_rejects_truncation() {
        struct Reset;
        impl Read for Reset {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::ConnectionReset.into())
            }
        }
        let body = r#"{"ok":true}"#;
        let wire = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        let response = read_local_response(BufReader::new(
            std::io::Cursor::new(wire.clone()).chain(Reset),
        ))
        .expect("complete framed response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body["ok"], true);
        let mut truncated = wire;
        truncated.pop();
        assert!(read_local_response(std::io::Cursor::new(truncated)).is_err());
        for head in [
            "Content-Length: 65536",
            "Content-Length: 1\r\nContent-Length: 1",
            "Transfer-Encoding: chunked",
            "Content-Length: +1",
        ] {
            assert!(
                read_local_response(std::io::Cursor::new(format!(
                    "HTTP/1.1 200 OK\r\n{head}\r\n\r\n{{}}"
                )))
                .is_err()
            );
        }
    }

    async fn call(
        server: &TestServer,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> TestResponse {
        let address = server.address;
        let path = path.to_owned();
        let token = token.map(str::to_owned);
        let body = serde_json::to_vec(&body).expect("encode request");
        tokio::task::spawn_blocking(move || {
            let mut request = format!(
                "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(token) = token {
                request.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
            request.push_str("\r\n");
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
                .expect("connect to local test server");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("bound HTTP response wait");
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .expect("bound HTTP request write");
            stream
                .write_all(request.as_bytes())
                .and_then(|()| stream.write_all(&body))
                .expect("write local HTTP request");
            read_local_response(BufReader::new(stream)).expect("read complete bounded local HTTP response")
        })
        .await
        .expect("bounded local HTTP client task")
    }

    #[tokio::test]
    async fn authenticated_router_persists_only_owner_scoped_commands() {
        let directory = TestDirectory::new();
        let (router, store, _) = service(&directory);
        let server = TestServer::start(router).await;
        let empty_before = store.snapshot().expect("initial snapshot");
        let task = contract("allowed-project");
        let task_body = json!({
            "request_id":"d85eab6c-6bdd-4d8e-9d39-c34915dbcb10",
            "contract":task,
        });

        let unauthorized = call(&server, "/v2/tasks", None, task_body.clone()).await;
        assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            store.snapshot().expect("snapshot after auth denial"),
            empty_before
        );

        let wrong_project = call(
            &server,
            "/v2/tasks",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"d85eab6c-6bdd-4d8e-9d39-c34915dbcb11",
                "contract":contract("foreign-project"),
            }),
        )
        .await;
        assert_eq!(wrong_project.status, StatusCode::FORBIDDEN);
        assert_eq!(
            store.snapshot().expect("snapshot after scope denial"),
            empty_before
        );

        let submitted = call(&server, "/v2/tasks", Some(OWNER_TOKEN), task_body.clone()).await;
        assert_eq!(submitted.status, StatusCode::OK);
        let submitted_json = submitted.body;
        assert_eq!(submitted_json["duplicate"], false);
        assert_eq!(submitted_json["execution_granted"], false);
        assert_eq!(submitted_json["acceptance_granted"], false);

        let replay = call(&server, "/v2/tasks", Some(OWNER_TOKEN), task_body).await;
        assert_eq!(replay.status, StatusCode::OK);
        assert_eq!(replay.body["duplicate"], true);

        let changed_payload = call(
            &server,
            "/v2/tasks",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"d85eab6c-6bdd-4d8e-9d39-c34915dbcb10",
                "contract":{
                    "schema_version":1,"project_id":"allowed-project","root_id":"root-a",
                    "task_id":"changed-task","revision":1,"title":"Changed body",
                    "objective":"Collide with existing request identity","non_goals":[],
                    "owned_paths":["src"],"forbidden_paths":[],"read_first":[],"dependencies":[],
                    "criteria":[{"id":"check-passes","text":"pass","check_ids":["check"]}],
                    "checks":[{"id":"check","program":"cargo","args":["check"],"cwd":".","timeout_secs":60}],
                    "authority_ref":"owner-authority","max_attempts":1,"simulated":false
                }
            }),
        )
        .await;
        assert_eq!(changed_payload.status, StatusCode::CONFLICT);
        let after_collision = store
            .snapshot()
            .expect("snapshot after idempotency collision");
        assert_eq!(after_collision.projection.tasks.len(), 1);

        let task_view = after_collision
            .projection
            .tasks
            .values()
            .next()
            .expect("submitted task projection");
        let task_digest = content_digest(task_view).expect("task CAS digest");
        let cancel_body = json!({
            "request_id":"d85eab6c-6bdd-4d8e-9d39-c34915dbcb12",
            "task_id":"task-a",
            "expected_task_digest":task_digest,
            "action":{"action":"cancel_pending","reason":"operator cancelled before execution"}
        });
        let cancelled = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            cancel_body,
        )
        .await;
        assert_eq!(cancelled.status, StatusCode::OK);
        let final_snapshot = store.snapshot().expect("final snapshot");
        assert_eq!(final_snapshot.projection.tasks.len(), 1);
        assert_eq!(
            final_snapshot
                .projection
                .tasks
                .values()
                .next()
                .unwrap()
                .state,
            SchedulingState::Cancelled
        );
    }

    #[tokio::test]
    async fn exact_lifecycle_retries_survive_later_cancellation_without_reapplying() {
        let directory = TestDirectory::new();
        let (router, store, _) = service(&directory);
        let server = TestServer::start(router).await;
        let original_contract = contract("allowed-project");
        let submitted = call(
            &server,
            "/v2/tasks",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"0c272170-8a01-4fbe-a58a-dc695e3b9f20",
                "contract":original_contract,
            }),
        )
        .await;
        assert_eq!(submitted.status, StatusCode::OK);
        let initial = store.snapshot().expect("submitted task snapshot");
        let mut revised_contract = original_contract;
        revised_contract.revision = 2;
        revised_contract.title = "Revised pending task".into();
        let revise_body = json!({
            "request_id":"0c272170-8a01-4fbe-a58a-dc695e3b9f21",
            "task_id":"task-a",
            "expected_task_digest":content_digest(initial.projection.tasks.values().next().unwrap()).unwrap(),
            "action":{"action":"revise_contract","contract":revised_contract},
        });
        let revised = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            revise_body.clone(),
        )
        .await;
        assert_eq!(revised.status, StatusCode::OK);
        let after_revision = store.snapshot().expect("revised task snapshot");
        let cancel_body = json!({
            "request_id":"0c272170-8a01-4fbe-a58a-dc695e3b9f22",
            "task_id":"task-a",
            "expected_task_digest":content_digest(after_revision.projection.tasks.values().next().unwrap()).unwrap(),
            "action":{"action":"cancel_pending","reason":"operator cancelled"},
        });
        let cancelled = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            cancel_body.clone(),
        )
        .await;
        assert_eq!(cancelled.status, StatusCode::OK);
        let terminal = store.snapshot().expect("cancelled task snapshot");

        // Simulate both successful responses being lost. The retained exact
        // receipts remain available after subsequent lifecycle transitions.
        for (body, original_receipt) in [
            (revise_body, revised.body),
            (cancel_body.clone(), cancelled.body),
        ] {
            let replay = call(&server, "/v2/tasks/lifecycle", Some(OWNER_TOKEN), body).await;
            assert_eq!(replay.status, StatusCode::OK);
            assert_eq!(replay.body["duplicate"], true);
            assert_eq!(replay.body["event_seq"], original_receipt["event_seq"]);
        }
        let mut changed = cancel_body.clone();
        changed["action"]["reason"] = json!("changed immutable request");
        let collision = call(&server, "/v2/tasks/lifecycle", Some(OWNER_TOKEN), changed).await;
        assert_eq!(collision.status, StatusCode::CONFLICT);
        let mut new_request = cancel_body.clone();
        new_request["request_id"] = json!("0c272170-8a01-4fbe-a58a-dc695e3b9f23");
        let terminal_mutation = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            new_request,
        )
        .await;
        assert_eq!(terminal_mutation.status, StatusCode::CONFLICT);
        let denied_replay = call(
            &server,
            "/v2/tasks/lifecycle",
            Some("wrong-owner-token-01234567890123456789"),
            cancel_body,
        )
        .await;
        assert_eq!(denied_replay.status, StatusCode::UNAUTHORIZED);
        let after_retries = store.snapshot().expect("unchanged replay snapshot");
        assert_eq!(after_retries.cursor, terminal.cursor);
        assert_eq!(after_retries.projection, terminal.projection);
        assert_eq!(
            after_retries.integrity_incidents, "1",
            "the changed immutable request records a collision incident"
        );
    }

    #[tokio::test]
    async fn forged_acceptance_unknown_fields_and_stale_cas_do_not_mutate() {
        let directory = TestDirectory::new();
        let (router, store, _) = service(&directory);
        let server = TestServer::start(router).await;
        let submitted = call(
            &server,
            "/v2/tasks",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"3944c4e1-679b-4db3-a18f-0e708cf0ec80",
                "contract":contract("allowed-project"),
            }),
        )
        .await;
        assert_eq!(submitted.status, StatusCode::OK);
        let before = store.snapshot().expect("snapshot before rejected commands");
        let forged = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"3944c4e1-679b-4db3-a18f-0e708cf0ec81",
                "task_id":"task-a",
                "expected_task_digest":content_digest(&contract("allowed-project")).expect("digest"),
                "action":{"action":"decide_candidate","attempt_id":"attempt-a",
                    "candidate_ref":"candidate","accepted":true,"evidence":{}},
            }),
        )
        .await;
        assert_eq!(forged.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            store.snapshot().expect("snapshot after forged acceptance"),
            before
        );

        let unknown_field = call(
            &server,
            "/v2/tasks",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"3944c4e1-679b-4db3-a18f-0e708cf0ec82",
                "contract":contract("allowed-project"),
                "actor":"caller-controlled",
            }),
        )
        .await;
        assert_eq!(unknown_field.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            store.snapshot().expect("snapshot after unknown field"),
            before
        );

        let stale_cas = call(
            &server,
            "/v2/tasks/lifecycle",
            Some(OWNER_TOKEN),
            json!({
                "request_id":"3944c4e1-679b-4db3-a18f-0e708cf0ec83",
                "task_id":"task-a",
                "expected_task_digest":format!("sha256:{}", "0".repeat(64)),
                "action":{"action":"cancel_pending","reason":"stale CAS"},
            }),
        )
        .await;
        assert_eq!(stale_cas.status, StatusCode::CONFLICT);
        assert_eq!(store.snapshot().expect("snapshot after stale CAS"), before);
    }

    #[test]
    fn duplicate_bearer_headers_are_rejected() {
        let directory = TestDirectory::new();
        let (_, store, projects) = service(&directory);
        let commands = NativeCommands::new(
            store,
            OWNER_TOKEN.into(),
            PrincipalId::new("configured-owner").unwrap(),
            projects,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.append(
            header::AUTHORIZATION,
            format!("Bearer {OWNER_TOKEN}").parse().unwrap(),
        );
        assert!(authenticated(&commands.state, &headers));
        headers.append(
            header::AUTHORIZATION,
            "Bearer another-token".parse().unwrap(),
        );
        assert!(!authenticated(&commands.state, &headers));
    }

    #[test]
    fn source_namespace_digest_and_timestamp_are_bounded_and_canonical() {
        let owner = PrincipalId::new("owner-a").expect("owner fixture");
        let uuid = Uuid::parse_str("56e54c8b-2ac5-4c50-a844-ffcc6df0ab11").expect("UUID");
        let source = source_for_request(&owner, uuid);
        assert_eq!(source.producer_id, "native-operator-http-owner-a");
        assert_eq!(source.incarnation, uuid.to_string());
        assert_eq!(source.sequence, "1");
        assert!(is_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!is_digest(&format!("sha256:{}", "A".repeat(64))));
        assert!(utc_timestamp().ends_with('Z'));
        assert_eq!(
            civil_from_days(0),
            (1970, 1, 1),
            "server UTC conversion uses the Unix epoch"
        );
    }
}
