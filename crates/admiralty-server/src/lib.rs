//! Read-only fleet projection and durable event replay. Native provider control remains disabled.
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
}

#[derive(Clone)]
struct AppState {
    store: Store,
    config: ServerConfig,
    reads: Arc<Semaphore>,
    streams: Arc<Semaphore>,
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
    if provided != Some(expected) {
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
            _ => {}
        }
    }
    FleetResponse {
        schema_version: 1,
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
    Ok(Json(fleet_from(
        snapshot.projection,
        snapshot.cursor,
        snapshot.sqlite_version,
        state.config.simulated,
    )))
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
    drop(_permit);
    let generation = snapshot.cursor.recovery_generation;
    let stream = futures_util::stream::unfold(
        (
            state,
            after,
            VecDeque::new(),
            generation,
            false,
            stream_permit,
            changes,
        ),
        |(state, mut after, mut pending, generation, closed, stream_permit, mut changes)| async move {
            if closed {
                return None;
            }
            loop {
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
                        if state.config.simulated && batch.events.iter().any(|r|matches!(&r.event,admiralty_core::DomainEvent::TaskSubmitted{contract} if !contract.simulated)) {
                        let event=Event::default().event("snapshot_required").data("demo scope changed; private state is unavailable");
                        return Some((Ok(event),(state,after,pending,generation,true,stream_permit,changes)));
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
                            ),
                        ));
                    }
                }
                if pending.is_empty() && changes.changed().await.is_err() {
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
                        ),
                    ));
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

async fn health() -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"status":"ok","version":env!("CARGO_PKG_VERSION"),"native_execution_qualified":false}),
    )
}

async fn asset(State(state): State<AppState>, path: Option<RoutePath<String>>) -> Response {
    let requested = path.map(|p| p.0).unwrap_or_else(|| "index.html".into());
    if !admiralty_core::safe_relative_path(&requested) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(root) = state.config.assets else {
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
            Some("json") => "application/json",
            _ => "application/octet-stream",
        };
        Some((mime, std::fs::read(path).ok()?))
    })
    .await;
    match data {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn app(
    store: Store,
    config: ServerConfig,
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
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
    let state = AppState {
        store,
        config,
        reads: Arc::new(Semaphore::new(8)),
        streams: Arc::new(Semaphore::new(MAX_EVENT_STREAMS)),
    };
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/v1/fleet", get(fleet))
        .route("/v1/events", get(events))
        .route("/v1/{*path}", get(|| async { StatusCode::NOT_FOUND }))
        .route("/", get(asset))
        .route("/{*path}", get(asset))
        .with_state(state))
}

pub async fn serve(
    store: Store,
    config: ServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listen = config.listen;
    let app = app(store, config)?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!(
        "Admiralty listening on http://{} (read-only; native execution disabled)",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
