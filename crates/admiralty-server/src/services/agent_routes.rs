//! Standalone bearer-authenticated routes for bounded coordination inbox access.
//!
//! The server composition layer owns credential loading and mounts this router.
//! This module exposes no raw append/enqueue or event-store operation.
use super::coordination_routing::{
    CommunicationReceipt, NativeRoutingAuthority, PendingAckRequest, SendMessageRequest,
};
use crate::services::coordination::{
    AuthenticatedCoordinationActor, CoordinationCredentialAuthority, CoordinationService,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, SourceIdentity, TaskId};
use admiralty_store::{
    Store, StoreError,
    inbox::{InboxCursor, InboxScope, PendingInboxPage},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedRwLockReadGuard, OwnedSemaphorePermit, RwLock, Semaphore},
    time::timeout,
};

const MAX_AGENT_REQUEST_BYTES: usize = 16 * 1024;
const MAX_AGENT_QUERY_BYTES: usize = 8 * 1024;
const MAX_AGENT_REQUESTS: usize = 8;
const CREDENTIAL_LOCK_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub struct AgentRouteState {
    store: Store,
    credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
    capacity: Arc<Semaphore>,
    routing: Option<Arc<NativeRoutingAuthority>>,
}

impl AgentRouteState {
    pub fn new(store: Store, credentials: Arc<RwLock<CoordinationCredentialAuthority>>) -> Self {
        Self {
            store,
            credentials,
            capacity: Arc::new(Semaphore::new(MAX_AGENT_REQUESTS)),
            routing: None,
        }
    }

    pub fn with_routing(
        store: Store,
        credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
        routing: Arc<NativeRoutingAuthority>,
    ) -> Self {
        Self {
            routing: Some(routing),
            ..Self::new(store, credentials)
        }
    }
}

/// Build routes for scope-indexed pending inbox reads and application ACKs.
/// The application must mount this router only after installing protected credentials.
/// Historical Store replay remains an internal service helper: its global event
/// cursor and scan counts must not disclose other projects' activity to agents.
pub fn router(state: AgentRouteState) -> Router {
    Router::new()
        .route("/pending", get(pending))
        .route("/context", get(task_context))
        .route("/application-ack", post(application_ack))
        .route("/send", post(send_message))
        .route("/ack-pending", post(acknowledge_pending))
        .layer(DefaultBodyLimit::max(MAX_AGENT_REQUEST_BYTES))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authenticate_before_extract,
        ))
        .with_state(state)
}

async fn send_message(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Json(input): Json<SendMessageRequest>,
) -> Result<Json<CommunicationReceipt>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let routing = state
        .routing
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "message routing is not configured"))?;
    let operation = tokio::task::spawn_blocking(move || {
        let _credentials = credentials;
        let _permit = permit;
        routing.send(&state.store, &actor, input)
    })
    .await
    .map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "message routing task failed",
        )
    })?;
    operation
        .map(Json)
        .map_err(|error| store_error(error, "message routing failed"))
}

async fn acknowledge_pending(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Json(input): Json<PendingAckRequest>,
) -> Result<Json<CommunicationReceipt>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let operation = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        super::coordination_routing::acknowledge_pending(&state.store, &actor, &credentials, input)
    })
    .await
    .map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "application receipt task failed",
        )
    })?;
    operation
        .map(Json)
        .map_err(|error| store_error(error, "application receipt failed"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextQuery {
    project_id: ProjectId,
    task_id: TaskId,
    context_epoch: String,
    known_contract_digest: Option<String>,
    max_estimated_tokens: Option<usize>,
    limit: Option<usize>,
    cursor_generation: Option<String>,
    cursor_scope_digest: Option<String>,
    cursor_after_message_id: Option<String>,
}

async fn task_context(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Query(query): Query<ContextQuery>,
) -> Result<Json<super::agent_tools::AgentTaskContext>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let cursor = parse_pending_cursor(
        query.cursor_generation,
        query.cursor_scope_digest,
        query.cursor_after_message_id,
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid context cursor"))?;
    let operation = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        super::agent_tools::task_context_get(
            &state.store,
            &actor,
            &credentials,
            query.project_id,
            query.task_id,
            query.context_epoch,
            query.known_contract_digest.as_deref(),
            query
                .max_estimated_tokens
                .unwrap_or(super::agent_tools::MAX_AGENT_CONTEXT_TOKENS),
            cursor,
            query.limit.unwrap_or(16),
        )
    })
    .await
    .map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "context read task failed",
        )
    })?;
    operation.map(Json).map_err(|error| {
        use super::agent_tools::AgentToolError;
        let status = match error {
            AgentToolError::AccessDenied | AgentToolError::TaskNotFound => StatusCode::FORBIDDEN,
            AgentToolError::CapsuleBudgetExceeded | AgentToolError::Backpressure => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            AgentToolError::Store(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        api_error(status, "task context unavailable for this scope")
    })
}

async fn authenticate_before_extract(
    State(state): State<AgentRouteState>,
    request: Request,
    next: Next,
) -> Response {
    let token = match bearer_token(request.headers()) {
        Some(token) => token,
        None => return api_error(StatusCode::UNAUTHORIZED, "bearer authentication required"),
    };
    let guard = match timeout(
        CREDENTIAL_LOCK_TIMEOUT,
        state.credentials.clone().read_owned(),
    )
    .await
    {
        Ok(guard) => guard,
        Err(_) => return api_error(StatusCode::SERVICE_UNAVAILABLE, "credential authority busy"),
    };
    if guard.authenticate(token).is_err() {
        return api_error(StatusCode::UNAUTHORIZED, "bearer authentication required");
    }
    drop(guard);
    if request
        .uri()
        .query()
        .is_some_and(|query| query.len() > MAX_AGENT_QUERY_BYTES)
    {
        return api_error(
            StatusCode::URI_TOO_LONG,
            "agent query exceeds admission bound",
        );
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_AGENT_REQUEST_BYTES)
    {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "agent request exceeds admission bound",
        );
    }
    next.run(request).await
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return None;
    }
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

async fn authenticated_state(
    state: &AgentRouteState,
    headers: &HeaderMap,
) -> Result<
    (
        AuthenticatedCoordinationActor,
        OwnedRwLockReadGuard<CoordinationCredentialAuthority>,
        OwnedSemaphorePermit,
    ),
    Response,
> {
    let token = bearer_token(headers)
        .ok_or_else(|| api_error(StatusCode::UNAUTHORIZED, "bearer authentication required"))?;
    let permit = state.capacity.clone().try_acquire_owned().map_err(|_| {
        api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "agent request capacity reached",
        )
    })?;
    let guard = timeout(
        CREDENTIAL_LOCK_TIMEOUT,
        state.credentials.clone().read_owned(),
    )
    .await
    .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "credential authority busy"))?;
    let actor = guard
        .authenticate(token)
        .map_err(|_| api_error(StatusCode::UNAUTHORIZED, "bearer authentication required"))?;
    Ok((actor, guard, permit))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingQuery {
    project_id: String,
    recipient: String,
    context_epoch: String,
    limit: Option<usize>,
    cursor_generation: Option<String>,
    cursor_scope_digest: Option<String>,
    cursor_after_message_id: Option<String>,
}

async fn pending(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Query(query): Query<PendingQuery>,
) -> Result<Json<PendingInboxPage>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let scope = parse_scope(query.project_id, query.recipient, query.context_epoch)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid inbox scope"))?;
    let cursor = parse_pending_cursor(
        query.cursor_generation,
        query.cursor_scope_digest,
        query.cursor_after_message_id,
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid pending cursor"))?;
    let limit = query.limit.unwrap_or(16);
    let store = state.store.clone();
    let operation = tokio::task::spawn_blocking(move || {
        let _credentials = credentials;
        let _permit = permit;
        CoordinationService::new(&store).pending_messages_authenticated(
            &actor,
            &_credentials,
            scope,
            cursor,
            limit,
        )
    })
    .await
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "agent read task failed"))?;
    operation
        .map(Json)
        .map_err(|error| store_error(error, "pending inbox read failed"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationAck {
    project_id: String,
    recipient: String,
    context_epoch: String,
    message_id: String,
    producer_id: String,
    incarnation: String,
    sequence: String,
    observed_utc: String,
}

async fn application_ack(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Json(body): Json<ApplicationAck>,
) -> Result<Json<admiralty_store::AppendReceipt>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let scope = parse_scope(body.project_id, body.recipient, body.context_epoch)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid inbox scope"))?;
    let message_id = MessageId::new(body.message_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid message identifier"))?;
    let source = SourceIdentity {
        producer_id: body.producer_id,
        incarnation: body.incarnation,
        sequence: body.sequence,
    };
    let observed_utc = body.observed_utc;
    let store = state.store.clone();
    let operation = tokio::task::spawn_blocking(move || {
        let _credentials = credentials;
        let _permit = permit;
        CoordinationService::new(&store).acknowledge_application_authenticated(
            &actor,
            &_credentials,
            source,
            scope,
            message_id,
            observed_utc,
        )
    })
    .await
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "agent ACK task failed"))?;
    operation
        .map(Json)
        .map_err(|error| store_error(error, "application ACK failed"))
}

fn parse_scope(
    project: String,
    recipient: String,
    context_epoch: String,
) -> Result<InboxScope, &'static str> {
    Ok(InboxScope {
        project_id: ProjectId::new(project).map_err(|_| "invalid project identifier")?,
        recipient: PrincipalId::new(recipient).map_err(|_| "invalid recipient identifier")?,
        context_epoch,
    })
}

fn parse_pending_cursor(
    generation: Option<String>,
    scope_digest: Option<String>,
    after_message_id: Option<String>,
) -> Result<Option<InboxCursor>, &'static str> {
    match (generation, scope_digest, after_message_id) {
        (None, None, None) => Ok(None),
        (Some(recovery_generation), Some(scope_digest), Some(after_message_id)) => {
            Ok(Some(InboxCursor {
                recovery_generation,
                scope_digest,
                after_message_id: MessageId::new(after_message_id)
                    .map_err(|_| "invalid pending cursor")?,
            }))
        }
        _ => Err("incomplete pending cursor"),
    }
}

fn store_error(error: StoreError, fallback: &'static str) -> Response {
    match error {
        StoreError::SourceCollision { .. } => api_error(
            StatusCode::CONFLICT,
            "request identity collision; reconcile the original message",
        ),
        StoreError::Transition(_) => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "message does not match the current task or conversation",
        ),
        StoreError::Invalid(_) | StoreError::Inbox(_) => api_error(
            StatusCode::BAD_REQUEST,
            "agent request is invalid or unauthorized",
        ),
        StoreError::Closed => api_error(StatusCode::SERVICE_UNAVAILABLE, "store unavailable"),
        _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, fallback),
    }
}

fn api_error(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialGrant,
    };
    use std::{
        fs,
        io::{Read, Write},
        net::{SocketAddr, TcpStream},
        os::unix::fs::DirBuilderExt,
        path::PathBuf,
    };

    struct TestRoot(PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn get_request(address: SocketAddr, path: &str, token: Option<&str>) -> String {
        let path = path.to_owned();
        let token = token.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(3))
                .expect("connect owned HTTP fixture");
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("bound response wait");
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .expect("bound request write");
            let bearer = token
                .map(|token| format!("Authorization: Bearer {token}\r\n"))
                .unwrap_or_default();
            socket
                .write_all(
                    format!(
                        "GET {path} HTTP/1.1\r\nHost: {address}\r\n{bearer}Connection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .expect("write owned request");
            let mut response = Vec::new();
            socket
                .take(64 * 1024)
                .read_to_end(&mut response)
                .expect("read bounded response");
            String::from_utf8(response).expect("HTTP fixture response")
        })
        .await
        .expect("owned HTTP client")
    }

    #[tokio::test]
    async fn public_routes_deny_global_history_and_recheck_revoked_credentials() {
        const TOKEN: &str = "private-agent-fixture-token-0123456789abcdef";
        let path = std::env::temp_dir().join(format!("agent-route-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create private fixture root");
        let directory = TestRoot(path);
        let store = Store::open(directory.0.join("store.sqlite3")).expect("fixture Store");
        let scope = InboxScope {
            project_id: ProjectId::new("project-a").expect("project"),
            recipient: PrincipalId::new("worker-a").expect("recipient"),
            context_epoch: "epoch-a".into(),
        };
        let credentials = Arc::new(RwLock::new(
            CoordinationCredentialAuthority::new(vec![CoordinationCredential::configured(
                "worker-credential".into(),
                TOKEN.into(),
                scope.recipient.clone(),
                vec![CoordinationCredentialGrant {
                    scope,
                    action: CoordinationAction::ReadPending,
                    message_id: None,
                    source: None,
                }],
            )])
            .expect("exact inbox grant"),
        ));
        let app = router(AgentRouteState::new(store.clone(), credentials.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned loopback listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let query = "/pending?project_id=project-a&recipient=worker-a&context_epoch=epoch-a";
        let initial = store.snapshot().expect("initial state");

        assert!(
            get_request(address, "/history", Some(TOKEN))
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(
            get_request(address, "/pending?unknown=malformed", None)
                .await
                .starts_with("HTTP/1.1 401")
        );
        let pending = get_request(address, query, Some(TOKEN)).await;
        assert!(pending.starts_with("HTTP/1.1 200"));
        assert!(pending.contains("\"messages\":[]"));
        credentials
            .write()
            .await
            .revoke("worker-credential")
            .expect("revoke configured credential");
        assert!(
            get_request(address, query, Some(TOKEN))
                .await
                .starts_with("HTTP/1.1 401")
        );
        assert_eq!(store.snapshot().expect("read-only final state"), initial);

        server.abort();
        let _ = server.await;
    }

    #[test]
    fn bearer_headers_require_one_unambiguous_credential() {
        let mut headers = HeaderMap::new();
        assert!(bearer_token(&headers).is_none());
        headers.append(
            header::AUTHORIZATION,
            "Bearer configured-token".parse().expect("header value"),
        );
        assert_eq!(bearer_token(&headers), Some("configured-token"));
        headers.append(
            header::AUTHORIZATION,
            "Bearer another-token".parse().expect("header value"),
        );
        assert!(bearer_token(&headers).is_none());
    }

    #[test]
    fn pending_cursor_requires_complete_bounded_identifier_shape() {
        assert!(parse_pending_cursor(None, None, None).is_ok());
        assert!(parse_pending_cursor(Some("g".into()), None, None).is_err());
        assert!(
            parse_pending_cursor(
                Some("g".into()),
                Some("sha256:abc".into()),
                Some("message-a".into()),
            )
            .is_ok()
        );
        assert!(parse_scope("../foreign".into(), "worker".into(), "epoch".into()).is_err());
    }
}
