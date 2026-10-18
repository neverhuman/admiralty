//! Standalone bearer-authenticated routes for bounded coordination inbox access.
//!
//! The server composition layer owns credential loading and mounts this router.
//! This module exposes no raw append/enqueue or event-store operation.
use crate::services::coordination::{
    AuthenticatedCoordinationActor, CoordinationCredentialAuthority, CoordinationReadScope,
    CoordinationService,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, SourceIdentity};
use admiralty_store::{
    Cursor, Store, StoreError,
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
}

impl AgentRouteState {
    pub fn new(store: Store, credentials: Arc<RwLock<CoordinationCredentialAuthority>>) -> Self {
        Self {
            store,
            credentials,
            capacity: Arc::new(Semaphore::new(MAX_AGENT_REQUESTS)),
        }
    }
}

/// Build routes for pending inbox reads, bounded queue history, and application ACKs.
/// The application must mount this router only after installing protected credentials.
pub fn router(state: AgentRouteState) -> Router {
    Router::new()
        .route("/pending", get(pending))
        .route("/history", get(history))
        .route("/application-ack", post(application_ack))
        .layer(DefaultBodyLimit::max(MAX_AGENT_REQUEST_BYTES))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authenticate_before_extract,
        ))
        .with_state(state)
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
struct HistoryQuery {
    project_id: String,
    recipient: String,
    context_epoch: String,
    recovery_generation: String,
    event_seq: String,
    scan_limit: Option<usize>,
}

async fn history(
    State(state): State<AgentRouteState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<crate::services::coordination::QueuedMessageBatch>, Response> {
    let (actor, credentials, permit) = authenticated_state(&state, &headers).await?;
    let project_id = ProjectId::new(query.project_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid project identifier"))?;
    let recipient = PrincipalId::new(query.recipient)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid recipient identifier"))?;
    let scope = CoordinationReadScope {
        project_id,
        recipient,
        context_epoch: query.context_epoch,
    };
    let cursor = Cursor {
        recovery_generation: query.recovery_generation,
        event_seq: query.event_seq,
    };
    let scan_limit = query.scan_limit.unwrap_or(16);
    let store = state.store.clone();
    let operation = tokio::task::spawn_blocking(move || {
        let _credentials = credentials;
        let _permit = permit;
        CoordinationService::new(&store).queued_messages_since_authenticated(
            &actor,
            &_credentials,
            &scope,
            &cursor,
            scan_limit,
        )
    })
    .await
    .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "agent read task failed"))?;
    operation
        .map(Json)
        .map_err(|error| store_error(error, "queue history read failed"))
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
