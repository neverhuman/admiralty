//! Bounded HTTP intake composition over the actual writer-authorized service.
//!
//! Root MUST independently wrap this router in the private server's existing
//! Authorization gate. This layer authenticates only a distinct intake header;
//! neither a browser profile, an owner label, nor an inbox grant is an issuer.
//! The router has no credential/configuration mutation or provider effect.
use super::work_order_intake::{
    AuthenticatedIntakeActor, PreviewIntakeInput, WorkOrderIntakeAuthRegistry,
    WorkOrderIntakeError, WorkOrderIntakeService,
};
use admiralty_core::{
    ProjectId, RootId,
    intake::{self, DuplicateQuery, IntakeFormat, SourcePayload, SourceProvenance},
    limits::bounded_json_size,
};
use admiralty_store::intake_capture::CaptureIntakeRequest;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_HTTP_QUERY_BYTES: usize = 8192;
/// Existing capture rows bound request and preview independently at8MiB.
/// HTTP-created requests are <=1MiB, so their full <=8MiB preview plus receipt
/// fits this response budget. Larger historical offline captures fail closed;
/// this is not an increase to request, source, Store or artifact bounds.
pub const MAX_HTTP_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS: usize = 8;
const BODY_DEADLINE: Duration = Duration::from_secs(5);
const INTAKE_HEADER: &str = "x-admiralty-intake-authorization";
const MARKER_HEADER: &str = "x-admiralty-requested-with";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkOrderRouteConfigError {
    InvalidOrigins,
}
impl std::fmt::Display for WorkOrderRouteConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("intake HTTP origins are invalid")
    }
}
impl std::error::Error for WorkOrderRouteConfigError {}
/// Trusted exact origin-to-authority pairs. No wildcard, URL lookup or DNS.
#[derive(Clone, Debug)]
pub struct ExactLoopbackOrigins {
    pairs: BTreeMap<String, String>,
}
impl ExactLoopbackOrigins {
    pub fn new(origins: Vec<String>) -> Result<Self, WorkOrderRouteConfigError> {
        if origins.is_empty() || origins.len() > 8 {
            return Err(WorkOrderRouteConfigError::InvalidOrigins);
        }
        let mut pairs = BTreeMap::new();
        for origin in origins {
            if origin.len() > 256
                || !origin.is_ascii()
                || origin
                    .bytes()
                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            {
                return Err(WorkOrderRouteConfigError::InvalidOrigins);
            }
            let authority = origin
                .strip_prefix("http://")
                .ok_or(WorkOrderRouteConfigError::InvalidOrigins)?;
            if authority.is_empty() || authority.contains(['@', '/', '?', '#']) {
                return Err(WorkOrderRouteConfigError::InvalidOrigins);
            }
            let valid = if let Some(port) = authority.strip_prefix("localhost:") {
                port.parse::<u16>()
                    .is_ok_and(|p| p != 0 && p.to_string() == port)
            } else {
                authority.parse::<SocketAddr>().is_ok_and(|a| {
                    a.ip().is_loopback() && a.port() != 0 && a.to_string() == authority
                })
            };
            if !valid || pairs.insert(origin.clone(), authority.to_owned()).is_some() {
                return Err(WorkOrderRouteConfigError::InvalidOrigins);
            }
        }
        Ok(Self { pairs })
    }
    fn permits(&self, h: &HeaderMap) -> bool {
        let Some(origin) = single(h, header::ORIGIN.as_str()) else {
            return false;
        };
        let Some(host) = single(h, header::HOST.as_str()) else {
            return false;
        };
        self.pairs.get(origin).is_some_and(|a| a == host)
            && single(h, MARKER_HEADER) == Some("intake")
    }
}
#[derive(Clone)]
pub struct WorkOrderRouteState {
    service: Arc<WorkOrderIntakeService>,
    registry: Arc<WorkOrderIntakeAuthRegistry>,
    origins: ExactLoopbackOrigins,
    capacity: Arc<Semaphore>,
}
impl WorkOrderRouteState {
    pub fn new(
        service: Arc<WorkOrderIntakeService>,
        registry: Arc<WorkOrderIntakeAuthRegistry>,
        origins: ExactLoopbackOrigins,
    ) -> Result<Self, WorkOrderRouteConfigError> {
        if origins.pairs.is_empty() {
            return Err(WorkOrderRouteConfigError::InvalidOrigins);
        }
        Ok(Self {
            service,
            registry,
            origins,
            capacity: Arc::new(Semaphore::new(MAX_REQUESTS)),
        })
    }
}
#[derive(Clone)]
struct Admitted {
    actor: AuthenticatedIntakeActor,
    permit: Arc<OwnedSemaphorePermit>,
}
fn single<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    if h.get_all(name).iter().count() != 1 {
        return None;
    }
    h.get(name)?.to_str().ok()
}
struct HttpFailure {
    status: StatusCode,
    code: &'static str,
}
fn reject(status: StatusCode, code: &'static str) -> HttpFailure {
    HttpFailure { status, code }
}
impl IntoResponse for HttpFailure {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            axum::Json(serde_json::json!({"error":self.code})),
        )
            .into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        response
    }
}
fn error(status: StatusCode, code: &'static str) -> Response {
    reject(status, code).into_response()
}
/// Relative routes; root mounts at /v2/intake and applies its main private gate.
/// None is unavailable before body/query extraction, never a demo/fallback grant.
pub fn router(state: Option<WorkOrderRouteState>) -> Router {
    Router::new()
        .route("/preview", post(preview))
        .route("/captures", post(capture))
        .route("/captures/{id}", get(read_capture).post(post_read_capture))
        .route("/duplicates/query", post(duplicates))
        .route("/work-orders", post(record))
        .route(
            "/work-orders/{id}",
            get(read_work_order).post(post_read_work_order),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), gate))
        .fallback(|| async { error(StatusCode::NOT_FOUND, "intake_route_not_found") })
        .with_state(state)
}
async fn gate(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(state) = state else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "intake_unavailable");
    };
    // Authentication precedes body/query extraction. Main private Authorization is
    // checked separately by root's outer gate; it can never supply this actor.
    let token = single(request.headers(), INTAKE_HEADER)
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| (32..=4096).contains(&s.len()));
    let actor = match token.and_then(|t| state.registry.authenticate(t).ok()) {
        Some(actor) => actor,
        None => return error(StatusCode::UNAUTHORIZED, "intake_credentials_required"),
    };
    if !state.origins.permits(request.headers()) {
        return error(StatusCode::FORBIDDEN, "intake_origin_or_marker_denied");
    }
    if request
        .uri()
        .query()
        .is_some_and(|q| q.len() > MAX_HTTP_QUERY_BYTES)
    {
        return error(StatusCode::URI_TOO_LONG, "intake_query_too_large");
    }
    if request
        .headers()
        .get_all(header::CONTENT_LENGTH)
        .iter()
        .count()
        > 1
    {
        return error(StatusCode::BAD_REQUEST, "invalid_content_length");
    }
    if let Some(length) = request.headers().get(header::CONTENT_LENGTH) {
        match length.to_str().ok().and_then(|s| s.parse::<usize>().ok()) {
            Some(n) if n <= MAX_HTTP_BODY_BYTES => {}
            Some(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "intake_body_too_large"),
            None => return error(StatusCode::BAD_REQUEST, "invalid_content_length"),
        }
    }
    if request.method() == axum::http::Method::GET
        && (request.headers().contains_key(header::TRANSFER_ENCODING)
            || request
                .headers()
                .get(header::CONTENT_LENGTH)
                .is_some_and(|v| v.as_bytes() != b"0"))
    {
        return error(StatusCode::BAD_REQUEST, "intake_read_body_forbidden");
    }
    if request.method() == axum::http::Method::POST
        && single(request.headers(), header::CONTENT_TYPE.as_str())
            .is_none_or(|v| v.split(';').next() != Some("application/json"))
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "intake_json_required");
    }
    let permit = match state.capacity.clone().try_acquire_owned() {
        Ok(p) => Arc::new(p),
        Err(_) => return error(StatusCode::TOO_MANY_REQUESTS, "intake_capacity_reached"),
    };
    request.extensions_mut().insert(Admitted { actor, permit });
    next.run(request).await
}
fn admitted(request: &mut Request) -> Result<Admitted, HttpFailure> {
    request
        .extensions_mut()
        .remove::<Admitted>()
        .ok_or_else(|| reject(StatusCode::SERVICE_UNAVAILABLE, "intake_admission_missing"))
}
async fn bytes(request: Request) -> Result<axum::body::Bytes, HttpFailure> {
    match tokio::time::timeout(
        BODY_DEADLINE,
        to_bytes(request.into_body(), MAX_HTTP_BODY_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(_)) => Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "intake_body_too_large",
        )),
        Err(_) => Err(reject(StatusCode::REQUEST_TIMEOUT, "intake_body_timeout")),
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, HttpFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| reject(StatusCode::BAD_REQUEST, "invalid_intake_json"))
}
fn service(state: Option<WorkOrderRouteState>) -> Result<Arc<WorkOrderIntakeService>, HttpFailure> {
    state
        .map(|s| s.service)
        .ok_or_else(|| reject(StatusCode::SERVICE_UNAVAILABLE, "intake_unavailable"))
}
#[derive(Serialize)]
struct Reply<T> {
    schema_version: u32,
    data: T,
    execution_granted: bool,
    acceptance_granted: bool,
}
fn success<T: Serialize>(data: T) -> Response {
    let reply = Reply {
        schema_version: 1,
        data,
        execution_granted: false,
        acceptance_granted: false,
    };
    if bounded_json_size(&reply, MAX_HTTP_RESPONSE_BYTES).is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "intake_response_bound");
    }
    match serde_json::to_vec(&reply) {
        Ok(bytes) => {
            let mut response = Body::from(bytes).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "intake_response_unavailable",
        ),
    }
}
fn failure(e: WorkOrderIntakeError) -> Response {
    use WorkOrderIntakeError::*;
    let (status, code) = match e {
        AccessDenied => (StatusCode::FORBIDDEN, "intake_action_denied"),
        InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_intake_request"),
        ArtifactClaimUnavailable => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "verified_source_unavailable",
        ),
        UnresolvedPlainTextEnrichment => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "intake_enrichment_unresolved",
        ),
        SelectionMismatch => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "intake_selection_mismatch",
        ),
        Collision | ConcurrentInflightCollision => {
            (StatusCode::CONFLICT, "intake_identity_conflict")
        }
        StaleCurrent => (StatusCode::CONFLICT, "intake_revision_stale"),
        CapacityExceeded => (StatusCode::TOO_MANY_REQUESTS, "intake_bounded_capacity"),
        IntegrityFailure | StorageUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "intake_storage_unavailable",
        ),
    };
    error(status, code)
}
async fn run<T: Serialize + Send + 'static>(
    admission: Admitted,
    operation: impl FnOnce(AuthenticatedIntakeActor) -> Result<T, WorkOrderIntakeError> + Send + 'static,
) -> Response {
    let _response_permit = Arc::clone(&admission.permit);
    match tokio::task::spawn_blocking(move || {
        let _permit = admission.permit;
        operation(admission.actor)
    })
    .await
    {
        Ok(Ok(data)) => success(data),
        Ok(Err(e)) => failure(e),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "intake_operation_unavailable",
        ),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    project_id: ProjectId,
    root_id: RootId,
    format: IntakeFormat,
    provenance: SourceProvenance,
    source_payload: SourcePayload,
}
async fn preview(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let bytes = match bytes(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let input: PreviewRequest = match decode(&bytes) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let input = PreviewIntakeInput {
        project_id: input.project_id,
        root_id: input.root_id,
        format: input.format,
        provenance: input.provenance,
        source_payload: input.source_payload,
    };
    run(admission, move |actor| service.preview_raw(&actor, &input)).await
}
async fn capture(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let bytes = match bytes(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let input: CaptureIntakeRequest = match decode(&bytes) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| service.capture_raw(&actor, input)).await
}
async fn record(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let bytes = match bytes(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let input = match intake::parse_record_request(&bytes) {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_intake_record_json"),
    };
    run(admission, move |actor| {
        service.record_enriched(&actor, input)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DuplicateRequest {
    project_id: ProjectId,
    root_id: RootId,
    query: DuplicateQuery,
}
async fn duplicates(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let bytes = match bytes(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let input: DuplicateRequest = match decode(&bytes) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| {
        service.find_duplicate_candidates(&actor, &input.project_id, &input.root_id, input.query)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadQuery {
    project_id: ProjectId,
    root_id: RootId,
    revision: Option<u32>,
}
async fn read_parts(request: Request) -> Result<(String, ReadQuery), HttpFailure> {
    let (mut parts, _) = request.into_parts();
    let id = Path::<String>::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| reject(StatusCode::BAD_REQUEST, "invalid_intake_id"))?
        .0;
    if id.is_empty() || id.len() > 128 {
        return Err(reject(StatusCode::BAD_REQUEST, "invalid_intake_id"));
    }
    let q = Query::<ReadQuery>::try_from_uri(&parts.uri)
        .map_err(|_| reject(StatusCode::BAD_REQUEST, "invalid_intake_query"))?
        .0;
    Ok((id, q))
}
async fn read_capture(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (id, q) = match read_parts(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| {
        service.get_captured_intake(&actor, &q.project_id, &q.root_id, &id, q.revision)
    })
    .await
}
async fn read_work_order(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (id, q) = match read_parts(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| {
        service.get_work_order(&actor, &q.project_id, &q.root_id, &id, q.revision)
    })
    .await
}

/// Browser-safe read alternative: same-origin POST supplies Origin itself.
/// URL query and JSON body may not compete for the same scope fields.
async fn post_read_parts(request: Request) -> Result<(String, ReadQuery), HttpFailure> {
    if request.uri().query().is_some_and(|q| !q.is_empty()) {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "ambiguous_intake_read_query",
        ));
    }
    let (mut parts, body) = request.into_parts();
    let id = Path::<String>::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| reject(StatusCode::BAD_REQUEST, "invalid_intake_id"))?
        .0;
    if id.is_empty() || id.len() > 128 {
        return Err(reject(StatusCode::BAD_REQUEST, "invalid_intake_id"));
    }
    let bytes = bytes(Request::from_parts(parts, body)).await?;
    Ok((id, decode(&bytes)?))
}
async fn post_read_capture(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (id, q) = match post_read_parts(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| {
        service.get_captured_intake(&actor, &q.project_id, &q.root_id, &id, q.revision)
    })
    .await
}
async fn post_read_work_order(
    State(state): State<Option<WorkOrderRouteState>>,
    mut request: Request,
) -> Response {
    let admission = match admitted(&mut request) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let service = match service(state) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (id, q) = match post_read_parts(request).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    run(admission, move |actor| {
        service.get_work_order(&actor, &q.project_id, &q.root_id, &id, q.revision)
    })
    .await
}
