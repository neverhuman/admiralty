//! Fleet projection, durable replay and explicitly configured native command services.
pub mod daemon;
pub mod embedded;
#[cfg(test)]
mod embedded_input;
pub mod fleet_brief;
mod http_bytes;
pub mod intake_configuration;
pub mod legacy;
pub mod model_selection;
pub mod models;
pub mod native_attention;
pub mod native_attention_context;
pub mod operator_chat;
pub mod services;
pub mod startup;
pub mod telemetry;
mod views;
use admiralty_core::{Projection, SchedulingState, TaskView};
use admiralty_store::{Cursor, Store};
use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

/// Prototype stream admission bound. Idle streams wait for a store change hint;
/// every wake re-reads durable events under its cursor and recovery generation.
/// Configurable limits require separate qualification. Stream permits never
/// consume snapshot read slots.
pub const MAX_EVENT_STREAMS: usize = 16;

#[derive(Clone)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub simulated: bool,
    pub token: Option<String>,
    pub assets: Option<PathBuf>,
    pub legacy: Option<legacy::LegacyConfig>,
    pub models_file: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    store: Store,
    config: ServerConfig,
    reads: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    models: Option<admiralty_runtime::models::RegistryFile>,
    views: Arc<views::ViewsHub>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    _shutdown_lifetime: tokio::sync::watch::Sender<bool>,
}

#[derive(Serialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub total: usize,
    pub running: usize,
    pub awaiting_verification: usize,
    pub blocked: usize,
    pub accepted: usize,
}

#[derive(Serialize)]
pub struct FleetResponse {
    pub schema_version: u32,
    pub simulated: bool,
    pub cursor: Cursor,
    pub projects: Vec<ProjectSummary>,
    pub tasks: Vec<TaskView>,
    pub coordination: CoordinationSummary,
    pub sqlite_version: String,
    pub native_execution_qualified: bool,
}

#[derive(Serialize)]
pub struct CoordinationSummary {
    pub queued_messages: usize,
    pub acknowledged_messages: usize,
    pub routine_model_turns: u32,
    pub measured_tokens: Option<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);
fn error(status: StatusCode, message: impl ToString) -> ApiError {
    (
        status,
        Json(serde_json::json!({"error":message.to_string()})),
    )
}

fn authorized(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    if state.config.simulated {
        return Ok(());
    }
    let expected = state.config.token.as_deref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "private server authentication not configured",
        )
    })?;
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    // Constant-time comparison: response timing must not reveal a prefix.
    if !provided.is_some_and(|p| telemetry::constant_time_eq(p.as_bytes(), expected.as_bytes())) {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "bearer authentication required",
        ));
    }
    Ok(())
}

fn fleet_from(
    projection: Projection,
    cursor: Cursor,
    sqlite_version: String,
    simulated: bool,
) -> FleetResponse {
    let schema_version = if projection.tasks.values().any(|t| t.lifecycle.is_some()) {
        2
    } else {
        1
    };
    let mut projects = BTreeMap::<String, ProjectSummary>::new();
    for task in projection.tasks.values() {
        let id = task.contract.project_id.to_string();
        let project = projects.entry(id.clone()).or_insert(ProjectSummary {
            id: id.clone(),
            name: id,
            total: 0,
            running: 0,
            awaiting_verification: 0,
            blocked: 0,
            accepted: 0,
        });
        project.total += 1;
        match task.state {
            SchedulingState::Running | SchedulingState::Claimed => project.running += 1,
            SchedulingState::AwaitingVerification => project.awaiting_verification += 1,
            SchedulingState::Blocked => project.blocked += 1,
            SchedulingState::Accepted => project.accepted += 1,
            _ => {}
        }
    }
    FleetResponse {
        schema_version,
        simulated,
        cursor,
        projects: projects.into_values().collect(),
        tasks: projection.tasks.into_values().collect(),
        coordination: CoordinationSummary {
            queued_messages: projection.messages.len(),
            acknowledged_messages: projection.acknowledgments.len(),
            routine_model_turns: 0,
            measured_tokens: None,
        },
        sqlite_version,
        native_execution_qualified: false,
    }
}

async fn fleet(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FleetResponse>, ApiError> {
    fleet_for_version(state, headers, 1).await
}

async fn fleet_v2(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FleetResponse>, ApiError> {
    fleet_for_version(state, headers, 2).await
}

async fn fleet_for_version(
    state: AppState,
    headers: HeaderMap,
    version: u32,
) -> Result<Json<FleetResponse>, ApiError> {
    authorized(&state, &headers)?;
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let store = state.store.clone();
    let snapshot = tokio::task::spawn_blocking(move || store.snapshot())
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if state.config.simulated
        && snapshot
            .projection
            .tasks
            .values()
            .any(|t| !t.contract.simulated)
    {
        return Err(error(
            StatusCode::CONFLICT,
            "demo mode cannot expose private task state",
        ));
    }
    if version == 1 && snapshot.schema_version != 1 {
        return Err(error(
            StatusCode::UPGRADE_REQUIRED,
            "native lifecycle requires /v2/fleet",
        ));
    }
    let mut response = fleet_from(
        snapshot.projection,
        snapshot.cursor,
        snapshot.sqlite_version,
        state.config.simulated,
    );
    response.schema_version = version;
    Ok(Json(response))
}

#[derive(Deserialize)]
struct EventQuery {
    after: Option<String>,
    generation: Option<String>,
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> Result<impl IntoResponse, ApiError> {
    events_for_version(state, headers, query, 1).await
}

async fn events_v2(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> Result<impl IntoResponse, ApiError> {
    events_for_version(state, headers, query, 2).await
}

async fn events_for_version(
    state: AppState,
    headers: HeaderMap,
    query: EventQuery,
    version: u32,
) -> Result<impl IntoResponse, ApiError> {
    authorized(&state, &headers)?;
    let after = query
        .after
        .as_deref()
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(|_| error(StatusCode::BAD_REQUEST, "after must be a decimal sequence"))?;
    let stream_permit = state.streams.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::TOO_MANY_REQUESTS,
            "event stream limit reached; close another stream before retrying",
        )
    })?;
    // Subscribe before the snapshot so an append between snapshot/query/wait
    // remains visible. Watch versions are wake hints, never event authority.
    let changes = state.store.subscribe_changes();
    let store = state.store.clone();
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let snapshot = tokio::task::spawn_blocking(move || store.snapshot())
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if state.config.simulated
        && snapshot
            .projection
            .tasks
            .values()
            .any(|t| !t.contract.simulated)
    {
        return Err(error(
            StatusCode::CONFLICT,
            "demo mode cannot expose private task state",
        ));
    }
    if query
        .generation
        .as_deref()
        .is_some_and(|g| g != snapshot.cursor.recovery_generation)
        || after > snapshot.cursor.event_seq.parse::<u64>().unwrap_or(0)
    {
        return Err(error(
            StatusCode::CONFLICT,
            "snapshot_required: recovery generation or event cursor changed",
        ));
    }
    if version == 1 && snapshot.schema_version != 1 {
        return Err(error(
            StatusCode::UPGRADE_REQUIRED,
            "native lifecycle requires /v2/events",
        ));
    }
    drop(_permit);
    let generation = snapshot.cursor.recovery_generation;
    let shutdown = state.shutdown.clone();
    let stream = futures_util::stream::unfold(
        (
            state,
            after,
            VecDeque::new(),
            generation,
            false,
            stream_permit,
            changes,
            shutdown,
        ),
        move |(
            state,
            mut after,
            mut pending,
            generation,
            closed,
            stream_permit,
            mut changes,
            mut shutdown,
        )| async move {
            if closed || *shutdown.borrow() {
                return None;
            }
            loop {
                if *shutdown.borrow() {
                    return None;
                }
                if let Some(record) = pending.pop_front() {
                    let record: admiralty_store::EventRecord = record;
                    after = record.seq.parse().unwrap_or(after);
                    let event = Event::default()
                        .event("domain")
                        .id(format!("{generation}:{after}"))
                        .json_data(&record)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialization failed")
                        });
                    return Some((
                        Ok::<_, Infallible>(event),
                        (
                            state,
                            after,
                            pending,
                            generation,
                            false,
                            stream_permit,
                            changes,
                            shutdown,
                        ),
                    ));
                }
                let permit = state.reads.clone().acquire_owned().await.ok()?;
                let store = state.store.clone();
                let cursor = Cursor {
                    recovery_generation: generation.clone(),
                    event_seq: after.to_string(),
                };
                let batch = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    store.events_since(&cursor, 100)
                })
                .await;
                match batch {
                    Ok(Ok(batch)) => {
                        if version == 1
                            && batch.events.iter().any(|r| {
                                matches!(
                                    &r.event,
                                    admiralty_core::DomainEvent::TaskLifecycle { .. }
                                )
                            })
                        {
                            let event = Event::default()
                                .event("upgrade_required")
                                .data("native lifecycle requires /v2/events");
                            return Some((
                                Ok(event),
                                (
                                    state,
                                    after,
                                    pending,
                                    generation,
                                    true,
                                    stream_permit,
                                    changes,
                                    shutdown,
                                ),
                            ));
                        }
                        if state.config.simulated && batch.events.iter().any(|r|matches!(&r.event,admiralty_core::DomainEvent::TaskSubmitted{contract} if !contract.simulated)) {
                        let event=Event::default().event("snapshot_required").data("demo scope changed; private state is unavailable");
                        return Some((Ok(event),(state,after,pending,generation,true,stream_permit,changes,shutdown)));
                    }
                        pending.extend(batch.events);
                    }
                    _ => {
                        let event = Event::default()
                            .event("snapshot_required")
                            .data("event replay unavailable or recovery generation changed");
                        return Some((
                            Ok(event),
                            (
                                state,
                                after,
                                pending,
                                generation,
                                true,
                                stream_permit,
                                changes,
                                shutdown,
                            ),
                        ));
                    }
                }
                if pending.is_empty() {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => {
                            return None;
                        }
                        changed = changes.changed() => {
                            if changed.is_err() {
                                let event = Event::default()
                                    .event("snapshot_required")
                                    .data("event notifications unavailable; fetch a fresh snapshot");
                                return Some((
                                    Ok(event),
                                    (
                                        state,
                                        after,
                                        pending,
                                        generation,
                                        true,
                                        stream_permit,
                                        changes,
                                        shutdown,
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("admiralty"),
    ))
}

fn model_config(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<admiralty_runtime::models::RegistryFile, ApiError> {
    authorized(state, headers)?;
    state
        .models
        .clone()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "model registry is not configured"))
}
// Run before JSON extraction: unavailable/demo endpoints and private auth must
// not depend on whether an untrusted caller supplied a valid request body.
async fn model_access_gate(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match model_config(&state, request.headers()) {
        Ok(_) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}

// Runs before the intake adapter reads a body or interprets its own credential.
// Reject ambiguous Authorization fields rather than choosing a proxy-dependent
// first value. No credential contents enter the response or diagnostics.
async fn intake_private_access_gate(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .count()
        > 1
    {
        return error(StatusCode::UNAUTHORIZED, "private authentication required").into_response();
    }
    match authorized(&state, request.headers()) {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}
async fn model_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let registry = model_config(&state, &headers)?;
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "read capacity unavailable"))?;
    models::list_models(registry).await
}
async fn model_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<models::ModelsUpdateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = model_config(&state, &headers)?;
    let _permit = state.reads.acquire().await.map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "model capacity unavailable",
        )
    })?;
    models::update_models(registry, request).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelectRequest {
    request: admiralty_runtime::models::SelectionRequest,
    capacity: admiralty_runtime::models::CapacitySnapshot,
}
async fn model_select(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ModelSelectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = model_config(&state, &headers)?;
    let _permit = state.reads.acquire().await.map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "model capacity unavailable",
        )
    })?;
    models::select_model(registry, body.request, body.capacity).await
}

fn legacy_config(state: &AppState, headers: &HeaderMap) -> Result<legacy::LegacyConfig, ApiError> {
    let config = state
        .config
        .legacy
        .clone()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "legacy bridge is not configured"))?;
    if !config.public_read {
        authorized(state, headers)?;
    }
    Ok(config)
}
async fn legacy_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    legacy_snapshot_view(state, headers, legacy::SnapshotView::Full).await
}
async fn legacy_detail_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    legacy_snapshot_view(state, headers, legacy::SnapshotView::Detail).await
}
async fn legacy_initial_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    legacy_snapshot_view(state, headers, legacy::SnapshotView::Initial).await
}
async fn legacy_brief_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    legacy_snapshot_view(state, headers, legacy::SnapshotView::Brief).await
}
async fn legacy_snapshot_view(
    state: AppState,
    headers: HeaderMap,
    view: legacy::SnapshotView,
) -> Result<Response, ApiError> {
    let started = std::time::Instant::now();
    let config = legacy_config(&state, &headers)?;
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "read capacity unavailable"))?;
    let encoded =
        legacy::snapshot_encoded(config, http_bytes::accepts_gzip(&headers), view).await?;
    let mut response = if http_bytes::not_modified(&headers, &encoded.etag) {
        (
            StatusCode::NOT_MODIFIED,
            [
                (header::VARY, "Accept-Encoding"),
                (header::CACHE_CONTROL, "no-store"),
            ],
        )
            .into_response()
    } else {
        http_bytes::encoded_response("application/json", encoded.body.clone(), encoded.gzip)
    };
    let headers = response.headers_mut();
    if let Ok(etag) = encoded.etag.parse() {
        headers.insert(header::ETAG, etag);
    }
    headers.insert(
        "x-admiralty-snapshot-cache",
        header::HeaderValue::from_static(if encoded.cache_hit { "hit" } else { "miss" }),
    );
    Ok(http_bytes::timed(response, "snapshot", started.elapsed()))
}
#[derive(Deserialize)]
struct BriefQuery {
    budget: Option<usize>,
    project: Option<String>,
    format: Option<String>,
}

/// Token-budgeted fleet brief for agents, derived from the same bounded
/// producer snapshot as the detail view. Same read scope as `/v1/legacy`.
async fn legacy_brief(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BriefQuery>,
) -> Result<Response, ApiError> {
    use admiralty_core::fleet_brief::{BriefOptions, brief};
    let started = std::time::Instant::now();
    let config = legacy_config(&state, &headers)?;
    let json = match query.format.as_deref() {
        None | Some("text") => false,
        Some("json") => true,
        Some(_) => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "format must be text or json",
            ));
        }
    };
    let project = match query.project {
        Some(p)
            if p.is_empty()
                || p.len() > 128
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) =>
        {
            return Err(error(StatusCode::BAD_REQUEST, "invalid project identifier"));
        }
        other => other,
    };
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "read capacity unavailable"))?;
    let snapshot = legacy::snapshot_value(config).await?;
    let result = brief(
        &snapshot,
        &BriefOptions {
            budget_bytes: query.budget.unwrap_or(2_000),
            project,
            now_utc: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        },
    );
    let omitted = result.omitted_lines.to_string();
    let mut response = if json {
        Json(&result).into_response()
    } else {
        (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            result.text,
        )
            .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    if let Ok(value) = omitted.parse() {
        headers.insert("x-admiralty-brief-omitted", value);
    }
    Ok(http_bytes::timed(response, "brief", started.elapsed()))
}

/// Status sources older than this are not ready (matches the dashboard's
/// stale threshold). A readiness pass is not a rendered-dashboard proof.
const READY_MAX_SOURCE_AGE_S: u64 = 180;

/// Readiness for supervisors and deploy checks. Unlike `/healthz` (process
/// up), this verifies the configured producer feed is present, bounded and
/// recently published, and that configured dashboard assets exist. It never
/// reads or reveals file contents or paths.
async fn readiness_sources(
    legacy: Option<legacy::LegacyConfig>,
    assets: Option<PathBuf>,
) -> (bool, serde_json::Map<String, serde_json::Value>) {
    let checks = tokio::task::spawn_blocking(move || {
        let mut checks = serde_json::Map::new();
        let mut ready = true;
        if let Some(legacy) = legacy {
            let metadata = std::fs::metadata(&legacy.status_file).ok();
            let age = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs());
            let ok = metadata
                .as_ref()
                .is_some_and(|m| m.is_file() && m.len() <= legacy::MAX_STATUS_BYTES)
                && age.is_some_and(|a| a <= READY_MAX_SOURCE_AGE_S);
            ready &= ok;
            checks.insert(
                "producer_feed".into(),
                serde_json::json!({"ok":ok,"age_s":age,"max_age_s":READY_MAX_SOURCE_AGE_S}),
            );
        }
        if let Some(assets) = assets {
            let ok = assets.join("index.html").is_file();
            ready &= ok;
            checks.insert("dashboard_assets".into(), serde_json::json!({"ok":ok}));
        }
        (ready, checks)
    })
    .await;
    checks.unwrap_or_else(|_| (false, serde_json::Map::new()))
}

async fn legacy_index(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let config = legacy_config(&state, &headers)?;
    let _permit = state
        .reads
        .acquire()
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "read capacity unavailable"))?;
    Ok(legacy::index(config).await)
}

async fn health() -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"status":"ok","version":env!("CARGO_PKG_VERSION"),"native_execution_qualified":false}),
    )
}

/// Readiness, unlike liveness, proves the Store writer answers a bounded
/// indexed query and the service is not draining. It never needs a token and
/// returns no task data, so load balancers and `systemd` checks can poll it.
async fn readiness(State(state): State<AppState>) -> Response {
    let draining = *state.shutdown.borrow();
    let store = state.store.clone();
    let started = std::time::Instant::now();
    let probe = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || store.events_after(i64::MAX as u64, 1).map(|_| ())),
    )
    .await;
    let store_check = match probe {
        Ok(Ok(Ok(()))) => "ok",
        Ok(Ok(Err(_))) | Ok(Err(_)) => "error",
        Err(_) => "timeout",
    };
    let (sources_ready, source_checks) =
        readiness_sources(state.config.legacy.clone(), state.config.assets.clone()).await;
    let ready = store_check == "ok" && !draining && sources_ready;
    if !ready {
        admiralty_telemetry::event(admiralty_telemetry::Level::Warn, "srv.ready", "ready.fail")
            .field("store", store_check)
            .field("draining", draining)
            .emit();
    }
    let mut checks = serde_json::json!({
        "store":store_check,"store_ms":started.elapsed().as_millis() as u64,
        "draining":draining,"read_permits_available":state.reads.available_permits(),
        "event_streams_available":state.streams.available_permits()
    });
    checks
        .as_object_mut()
        .expect("checks object")
        .extend(source_checks);
    let body = serde_json::json!({
        "ready": ready,
        "status": if ready { "ready" } else { "not_ready" },
        "version": env!("CARGO_PKG_VERSION"),
        "checks": checks,
        "native_execution_qualified": false,
    });
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

/// Prometheus text exposition. Private deployments require the bearer token;
/// the synthetic demo exposes it like its other reads.
async fn prometheus_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorized(&state, &headers)?;
    let text = telemetry::render_prometheus(&telemetry::Gauges {
        event_streams_active: MAX_EVENT_STREAMS.saturating_sub(state.streams.available_permits()),
        event_streams_limit: MAX_EVENT_STREAMS,
        read_permits_available: state.reads.available_permits(),
        simulated: state.config.simulated,
    });
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        text,
    )
        .into_response())
}

#[derive(Deserialize)]
struct DigestQuery {
    budget_tokens: Option<usize>,
    min_level: Option<String>,
    since_s: Option<u64>,
}

/// Maximum tokens a single digest request may ask for.
pub const MAX_DIGEST_TOKENS: usize = 8192;

/// Token-budgeted digest of this process's recent telemetry, for agents.
async fn log_digest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DigestQuery>,
) -> Result<Response, ApiError> {
    authorized(&state, &headers)?;
    let min_level = match query.min_level.as_deref() {
        None => admiralty_telemetry::Level::Trace,
        Some(text) => admiralty_telemetry::Level::parse(text)
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid min_level"))?,
    };
    let options = admiralty_telemetry::digest::Options {
        budget_tokens: query.budget_tokens.unwrap_or(600).min(MAX_DIGEST_TOKENS),
        since: query
            .since_s
            .map(|s| std::time::SystemTime::now() - Duration::from_secs(s.min(7 * 86_400))),
        min_level,
    };
    let logger = admiralty_telemetry::global();
    let text =
        admiralty_telemetry::digest::render(logger.component(), &logger.snapshot(), &options);
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        text,
    )
        .into_response())
}

async fn asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Option<RoutePath<String>>,
) -> Response {
    let started = std::time::Instant::now();
    let Ok(_permit) = state.reads.acquire().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let gzip = http_bytes::accepts_gzip(&headers);
    let requested = path.map(|p| p.0).unwrap_or_else(|| "index.html".into());
    if !admiralty_core::safe_relative_path(&requested) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(root) = state.config.assets else {
        // An explicit --assets directory always wins; otherwise serve the
        // dashboard compiled into this binary (release builds), if any.
        if let Some(response) = embedded::respond(&requested, &headers, gzip) {
            return http_bytes::timed(response, "asset", started.elapsed());
        }
        if embedded::embedded_dashboard().is_some() {
            return StatusCode::NOT_FOUND.into_response();
        }
        return (StatusCode::OK,[(header::CONTENT_TYPE,"text/html; charset=utf-8")],"<!doctype html><title>Admiralty</title><h1>Admiralty Rust foundation</h1><p>Static dashboard not built yet. Read-only JSON: <a href='/v1/fleet'>fleet</a>. Native execution remains disabled.</p>").into_response();
    };
    let data = tokio::task::spawn_blocking(move || {
        let root = root.canonicalize().ok()?;
        let path = root.join(&requested);
        let path = if path.is_file() {
            path
        } else if !requested.contains('.') {
            root.join("index.html")
        } else {
            return None;
        };
        let path = path.canonicalize().ok()?;
        if !path.starts_with(&root) || path.metadata().ok()?.len() > 16 * 1024 * 1024 {
            return None;
        }
        let mime = match path.extension().and_then(|p| p.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            Some("json") => "application/json",
            Some("woff2") => "font/woff2",
            Some("ico") => "image/x-icon",
            _ => "application/octet-stream",
        };
        let bytes = legacy::read_bounded(&path, 16 * 1024 * 1024).ok()?;
        let mut response = http_bytes::response(mime, bytes, gzip).ok()?;
        // Vite's eight-character URL-safe hash can itself contain '-'.
        let hashed_asset = embedded::is_hashed_asset(&requested);
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            if hashed_asset {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            }
            .parse()
            .expect("static asset cache policy"),
        );
        Some(response)
    })
    .await;
    let response = match data {
        Ok(Some(response)) => response,
        _ => StatusCode::NOT_FOUND.into_response(),
    };
    http_bytes::timed(response, "asset", started.elapsed())
}

pub fn app(
    store: Store,
    config: ServerConfig,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    app_with_native_routes(store, config, None)
}

/// Native command routes are constructed by protected startup configuration;
/// neither dashboard input nor demo mode can enable this surface.
pub fn app_with_native_routes(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    app_with_work_order_routes(store, config, native_routes, None)
}

/// Intake is an independently configured capability. The server credential
/// authenticates access to this private transport; it never grants intake scope.
/// The intake adapter separately authenticates its own opaque current actor.
pub fn app_with_work_order_routes(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
    intake: Option<services::work_order_routes::WorkOrderRouteState>,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    let (shutdown, _) = tokio::sync::watch::channel(false);
    app_with_work_order_routes_and_shutdown(
        store,
        config,
        native_routes,
        intake,
        shutdown,
        views::ViewsHub::new(),
    )
}

fn app_with_work_order_routes_and_shutdown(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
    intake: Option<services::work_order_routes::WorkOrderRouteState>,
    shutdown: tokio::sync::watch::Sender<bool>,
    views: Arc<views::ViewsHub>,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    if config.simulated && (native_routes.is_some() || intake.is_some()) {
        return Err("demo mode cannot install native command routes".into());
    }
    if !config.listen.ip().is_loopback() {
        return Err("remote bind is not qualified; use a loopback SSH tunnel".into());
    }
    if !config.simulated && config.token.as_deref().is_none_or(|t| t.len() < 32) {
        return Err(
            "private reads require a configured bearer token of at least 32 characters".into(),
        );
    }
    if config.simulated
        && store
            .snapshot()?
            .projection
            .tasks
            .values()
            .any(|t| !t.contract.simulated)
    {
        return Err("refusing to expose private task state in public demo mode".into());
    }
    if config.simulated && (config.legacy.is_some() || config.models_file.is_some()) {
        return Err("synthetic demo cannot expose real legacy or model configuration state".into());
    }
    let models = config
        .models_file
        .clone()
        .map(admiralty_runtime::models::RegistryFile::new)
        .transpose()?;
    let state = AppState {
        store,
        config,
        reads: Arc::new(Semaphore::new(8)),
        streams: Arc::new(Semaphore::new(MAX_EVENT_STREAMS)),
        models,
        views,
        shutdown: shutdown.subscribe(),
        _shutdown_lifetime: shutdown,
    };
    let model_routes = Router::new()
        .route("/v1/models", get(model_list).put(model_update))
        .route("/v1/models/select", axum::routing::post(model_select))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            model_access_gate,
        ));
    let intake_routes = services::work_order_routes::router(intake).route_layer(
        axum::middleware::from_fn_with_state(state.clone(), intake_private_access_gate),
    );
    let router = Router::new()
        .merge(model_routes)
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route("/metrics", get(prometheus_metrics))
        .route("/v1/logs/digest", get(log_digest))
        .route("/v1/fleet", get(fleet))
        .route("/v2/fleet", get(fleet_v2))
        .route("/v2/events", get(events_v2))
        .route("/v2/views", get(views::snapshot))
        .route("/v2/views/events", get(views::events))
        .route("/v2/{*path}", get(|| async { StatusCode::NOT_FOUND }))
        .route("/v1/legacy", get(legacy_snapshot))
        .route("/v1/legacy/detail", get(legacy_detail_snapshot))
        .route("/v1/legacy/detail/initial", get(legacy_initial_snapshot))
        .route("/v1/brief", get(legacy_brief))
        .route("/v1/legacy/brief", get(legacy_brief_snapshot))
        .route("/legacy/", get(legacy_index))
        .route("/legacy/index.html", get(legacy_index))
        .route("/legacy/status.json", get(legacy_snapshot))
        .route("/legacy/{*path}", get(|| async { StatusCode::NOT_FOUND }))
        .route("/v1/events", get(events))
        .route("/v1/{*path}", get(|| async { StatusCode::NOT_FOUND }))
        .route("/", get(asset))
        .route("/{*path}", get(asset))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(state)
        .nest("/v2/intake", intake_routes);
    Ok(match native_routes {
        Some(routes) => router.merge(routes),
        None => router,
    })
}

pub async fn serve(
    store: Store,
    config: ServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_with_native_routes(store, config, None).await
}

pub async fn serve_with_native_routes(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_with_work_order_routes(store, config, native_routes, None).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccessLogMode {
    Off,
    /// Default: failed (5xx) or slow (>= 1 s) requests only.
    Problems,
    All,
}

fn access_log_mode() -> AccessLogMode {
    static MODE: std::sync::OnceLock<AccessLogMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("ADMIRALTY_ACCESS_LOG").as_deref() {
        Ok("off") => AccessLogMode::Off,
        Ok("all") => AccessLogMode::All,
        _ => AccessLogMode::Problems,
    })
}

const SLOW_REQUEST_MS: u128 = 1_000;

/// Compact request events on stderr (logfmt). Only method, path, status,
/// duration and snapshot cache state are recorded: never query strings,
/// headers, bodies or tokens.
fn access_event(
    method: &str,
    path: &str,
    status: u16,
    ms: u128,
    cache: Option<&str>,
    mode: AccessLogMode,
) -> Option<admiralty_core::agent_log::Event> {
    use admiralty_core::agent_log::{Event, Level};
    let level = if status >= 500 {
        Level::Error
    } else if ms >= SLOW_REQUEST_MS {
        Level::Warn
    } else {
        Level::Info
    };
    if mode == AccessLogMode::Off || (mode == AccessLogMode::Problems && level == Level::Info) {
        return None;
    }
    let mut event = Event::new(
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        level,
        "http.request",
    )
    .field("method", method)
    .field("path", path)
    .field("status", status)
    .field("ms", u64::try_from(ms).unwrap_or(u64::MAX));
    if let Some(cache) = cache {
        event = event.field("cache", cache);
    }
    Some(event)
}

async fn access_log(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mode = access_log_mode();
    if mode == AccessLogMode::Off {
        return next.run(request).await;
    }
    let started = std::time::Instant::now();
    let method = request.method().as_str().to_owned();
    let path = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str())
        .unwrap_or("unmatched")
        .to_owned();
    let response = next.run(request).await;
    let cache = response
        .headers()
        .get("x-admiralty-snapshot-cache")
        .and_then(|v| v.to_str().ok());
    if let Some(event) = access_event(
        &method,
        &path,
        response.status().as_u16(),
        started.elapsed().as_millis(),
        cache,
        mode,
    ) {
        eprintln!(
            "{}",
            event.render(admiralty_core::agent_log::Format::Logfmt)
        );
    }
    response
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match &mut sigterm {
                    Some(s) => {
                        s.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub async fn serve_with_work_order_routes(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
    intake: Option<services::work_order_routes::WorkOrderRouteState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_with_operator_chat(store, config, native_routes, intake, None).await
}

/// Role inboxes are independently enrolled; mounting observation routes never
/// installs a native execution grant. Start their watcher only after binding.
pub async fn serve_with_operator_chat(
    store: Store,
    config: ServerConfig,
    native_routes: Option<Router>,
    intake: Option<services::work_order_routes::WorkOrderRouteState>,
    chat: Option<operator_chat::Configuration>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if config.simulated && chat.is_some() {
        return Err("demo mode cannot enroll live operator chat targets".into());
    }
    let listen = config.listen;
    let native_commands = native_routes.is_some() || intake.is_some();
    let chat_enabled = chat.is_some();
    let chat_source = (!config.simulated).then(|| {
        (
            config.token.clone().unwrap_or_default(),
            config.legacy.as_ref().map(|c| c.status_file.clone()),
        )
    });
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let views = views::ViewsHub::new();
    let app = app_with_work_order_routes_and_shutdown(
        store,
        config,
        native_routes,
        intake,
        shutdown_tx.clone(),
        views.clone(),
    )?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let app = match chat_source {
        Some((token, source)) => app.merge(operator_chat::router(token, chat, source)),
        None => app,
    };
    // Applied last so every merged and nested route is measured; MatchedPath
    // gives bounded route-template labels instead of raw request paths.
    let app = app
        .layer(axum::middleware::from_fn(access_log))
        .layer(axum::middleware::from_fn(telemetry::observe));
    let mode = if native_commands {
        "native commands enabled; execution requires admitted controller"
    } else if chat_enabled {
        "operator chat enabled; native execution disabled"
    } else {
        "read-only; native execution disabled"
    };
    let local = listener.local_addr()?;
    // Readiness contract for harnesses and operators: keep this exact prefix.
    eprintln!("Admiralty listening on http://{local} ({mode})");
    admiralty_telemetry::event(admiralty_telemetry::Level::Info, "srv", "srv.start")
        .field("listen", local.to_string())
        .field("mode", mode)
        .field("version", env!("CARGO_PKG_VERSION"))
        .volatile("pid", std::process::id())
        .emit();
    let summaries = tokio::spawn({
        let mut stop = shutdown_tx.subscribe();
        async move {
            let window = SUMMARY_INTERVAL;
            let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + window, window);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => telemetry::emit_summary(window),
                    changed = stop.changed() => if changed.is_err() || *stop.borrow() { break },
                }
            }
        }
    });
    let signal_tx = shutdown_tx.clone();
    let outcome = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            signal_tx.send_replace(true);
        })
        .await;
    // A notification is not a join. Release the updater's Store and any
    // in-flight blocking projection before the demo can remove its database.
    shutdown_tx.send_replace(true);
    let _ = summaries.await;
    views.shutdown().await?;
    telemetry::emit_summary(SUMMARY_INTERVAL);
    admiralty_telemetry::event(admiralty_telemetry::Level::Info, "srv", "srv.stop")
        .field("listen", local.to_string())
        .field("clean", outcome.is_ok())
        .emit();
    admiralty_telemetry::global().flush();
    outcome?;
    Ok(())
}

/// Interval between `http.summary` lines and log-burst flushes.
pub const SUMMARY_INTERVAL: Duration = Duration::from_secs(60);

/// Compatibility port implemented by the same native binary. Target and bind
/// remain loopback-only, and the original path/query is retained.
pub fn redirect_app(
    target: SocketAddr,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    if !target.ip().is_loopback() || target.port() == 0 {
        return Err("redirect target must be a concrete loopback listener".into());
    }
    let base = format!("http://{target}");
    Ok(Router::new().fallback(move |uri: axum::http::Uri| {
        let destination = format!(
            "{}{}",
            base,
            uri.path_and_query().map_or("/", |value| value.as_str())
        );
        async move { axum::response::Redirect::temporary(&destination) }
    }))
}

pub async fn serve_redirect(
    listen: SocketAddr,
    target: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !listen.ip().is_loopback() || listen == target {
        return Err("redirect listener must be a separate loopback address".into());
    }
    let app = redirect_app(target)?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod access_log_tests {
    use super::*;

    #[test]
    fn problems_mode_logs_only_failures_and_slow_requests() {
        let quiet = access_event(
            "GET",
            "/v1/legacy/detail",
            200,
            12,
            Some("hit"),
            AccessLogMode::Problems,
        );
        assert!(quiet.is_none());
        let slow = access_event(
            "GET",
            "/v1/legacy/detail",
            200,
            1_500,
            Some("miss"),
            AccessLogMode::Problems,
        )
        .expect("slow request")
        .render(admiralty_core::agent_log::Format::Logfmt);
        assert!(
            slow.contains(
                " warn http.request method=GET path=/v1/legacy/detail status=200 ms=1500 cache=miss"
            ),
            "{slow}"
        );
        let failed = access_event("GET", "/v1/legacy", 502, 3, None, AccessLogMode::Problems)
            .expect("failure");
        assert_eq!(failed.level, admiralty_core::agent_log::Level::Error);
        assert!(access_event("GET", "/", 200, 1, None, AccessLogMode::All).is_some());
        assert!(access_event("GET", "/", 500, 1, None, AccessLogMode::Off).is_none());
    }
}
