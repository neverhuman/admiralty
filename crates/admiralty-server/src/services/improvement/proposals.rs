//! Bounded local incident/proposal HTTP surface; no issue/PR publisher exists here.
use super::excursions::ExcursionPolicy;
use admiralty_core::excursions::ExcursionScope;
use admiralty_store::{
    Store, StoreError,
    excursions::{ExcursionCommand, ExcursionStoreError, MAX_COMMAND_BYTES, MAX_SNAPSHOT_BYTES},
};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::de::DeserializeOwned;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct RouteState {
    store: Store,
    policy: Arc<ExcursionPolicy>,
    slots: Arc<Semaphore>,
}
fn error(status: StatusCode, message: &'static str) -> Response {
    (status, axum::Json(serde_json::json!({"error":message}))).into_response()
}
fn failure(error_value: StoreError) -> Response {
    let (status, message) = match error_value {
        StoreError::Excursion(ExcursionStoreError::Denied) => {
            (StatusCode::FORBIDDEN, "incident_scope_denied")
        }
        StoreError::Excursion(ExcursionStoreError::Stale | ExcursionStoreError::Collision) => (
            StatusCode::CONFLICT,
            "incident_revision_or_identity_conflict",
        ),
        StoreError::Excursion(ExcursionStoreError::Capacity) => (
            StatusCode::INSUFFICIENT_STORAGE,
            "incident_capacity_exhausted",
        ),
        StoreError::Excursion(ExcursionStoreError::Invalid | ExcursionStoreError::Reduction(_)) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "incident_input_rejected")
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "incident_store_unavailable",
        ),
    };
    error(status, message)
}
fn json<T: serde::Serialize>(value: &T) -> Response {
    let Ok(bytes) = serde_json::to_vec(value) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "incident_response_unavailable",
        );
    };
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "incident_response_exceeds_bound",
        );
    }
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}
async fn admit<T: DeserializeOwned>(
    state: &RouteState,
    request: Request,
) -> Result<
    (
        T,
        admiralty_store::excursions::ExcursionProof,
        tokio::sync::OwnedSemaphorePermit,
    ),
    Response,
> {
    let permit = state
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "incident_capacity_busy"))?;
    let h = request.headers();
    if h.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "incident_authentication_required",
        ));
    }
    let proof = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| state.policy.authenticate(t))
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "incident_authentication_required"))?;
    if h.get_all(header::CONTENT_TYPE).iter().count() != 1
        || h.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) != Some("application/json")
    {
        return Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "incident_json_required",
        ));
    }
    if h.contains_key(header::ORIGIN) {
        let origin = h
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("http://"));
        let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
        let local = origin.is_some_and(|a| {
            a.parse::<SocketAddr>().is_ok_and(|s| s.ip().is_loopback())
                || a.strip_prefix("localhost:")
                    .is_some_and(|p| p.parse::<u16>().is_ok_and(|p| p > 0))
        });
        if h.get_all(header::ORIGIN).iter().count() != 1
            || h.get_all(header::HOST).iter().count() != 1
            || !local
            || origin != host
            || h.get("x-admiralty-requested-with")
                .and_then(|v| v.to_str().ok())
                != Some("incidents")
        {
            return Err(error(StatusCode::FORBIDDEN, "incident_origin_denied"));
        }
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), MAX_COMMAND_BYTES),
    )
    .await
    .map_err(|_| error(StatusCode::REQUEST_TIMEOUT, "incident_body_deadline"))?
    .map_err(|_| error(StatusCode::PAYLOAD_TOO_LARGE, "incident_body_exceeds_bound"))?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "incident_invalid_json"))?;
    Ok((value, proof, permit))
}
async fn read(State(state): State<RouteState>, request: Request) -> Response {
    let (scope, proof, permit) = match admit::<ExcursionScope>(&state, request).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.store.read_excursions(scope, proof)
    })
    .await
    {
        Ok(Ok(snapshot)) => json(&snapshot),
        Ok(Err(e)) => failure(e),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "incident_worker_unavailable",
        ),
    }
}
async fn command(State(state): State<RouteState>, request: Request) -> Response {
    let (command, proof, permit) = match admit::<ExcursionCommand>(&state, request).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.store.record_excursion(command, proof)
    })
    .await
    {
        Ok(Ok(receipt)) => json(&receipt),
        Ok(Err(e)) => failure(e),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "incident_worker_unavailable",
        ),
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeQuery {}
async fn scopes(State(state): State<RouteState>, request: Request) -> Response {
    let (_, proof, _permit) = match admit::<ScopeQuery>(&state, request).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    json(&state.policy.scopes(proof.as_ref()))
}
pub fn router(store: Store, policy: Arc<ExcursionPolicy>) -> Router {
    Router::new()
        .route("/v2/incidents/scopes", post(scopes))
        .route("/v2/incidents/read", post(read))
        .route("/v2/incidents/commands", post(command))
        .with_state(RouteState {
            store,
            policy,
            slots: Arc::new(Semaphore::new(8)),
        })
}

#[cfg(test)]
mod tests {
    use super::super::excursions::ExcursionGrant;
    use super::*;
    use admiralty_core::{ProjectId, RootId, TaskId};
    use admiralty_store::{
        StoreAuthorities,
        excursions::{ExcursionAction, ExcursionOperation},
    };
    use axum::body::Body;

    fn scope() -> ExcursionScope {
        ExcursionScope {
            project: ProjectId::new("p").expect("p"),
            root: RootId::new("r").expect("r"),
            task: TaskId::new("t").expect("t"),
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
        }
    }
    fn request(value: serde_json::Value, token: &str) -> Request {
        Request::builder()
            .method("POST")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .expect("request")
    }
    fn setup(actions: Vec<ExcursionAction>) -> (tempfile::TempDir, RouteState) {
        #[cfg(unix)]
        let dir = {
            use std::os::unix::fs::PermissionsExt;
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .expect("private scratch")
        };
        #[cfg(not(unix))]
        let dir = tempfile::tempdir().expect("scratch");
        let policy = Arc::new(
            ExcursionPolicy::new(
                &"x".repeat(32),
                vec![ExcursionGrant {
                    scope: scope(),
                    actions,
                    evidence_digests: vec![],
                }],
            )
            .expect("policy"),
        );
        let store = Store::open_with_startup_authorities(
            dir.path().join("incidents.db"),
            StoreAuthorities {
                excursions: Some(policy.clone()),
                ..Default::default()
            },
        )
        .expect("store");
        (
            dir,
            RouteState {
                store,
                policy,
                slots: Arc::new(Semaphore::new(8)),
            },
        )
    }
    #[tokio::test]
    async fn incident_routes_authenticate_before_decode_and_enforce_scope_origin_and_revocation() {
        let (_dir, state) = setup(vec![ExcursionAction::Read]);
        let mut bad = request(serde_json::json!({}), "wrong");
        *bad.body_mut() = Body::from("not json");
        assert_eq!(
            read(State(state.clone()), bad).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let ok = request(
            serde_json::to_value(scope()).expect("scope"),
            &"x".repeat(32),
        );
        let response = read(State(state.clone()), ok).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let mut foreign = scope();
        foreign.task = TaskId::new("foreign").expect("task");
        assert_eq!(
            read(
                State(state.clone()),
                request(
                    serde_json::to_value(foreign).expect("scope"),
                    &"x".repeat(32)
                )
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let mut origin = request(
            serde_json::to_value(scope()).expect("scope"),
            &"x".repeat(32),
        );
        origin.headers_mut().insert(
            header::ORIGIN,
            "http://external.invalid".parse().expect("origin"),
        );
        origin
            .headers_mut()
            .insert(header::HOST, "external.invalid".parse().expect("host"));
        assert_eq!(
            read(State(state.clone()), origin).await.status(),
            StatusCode::FORBIDDEN
        );
        let scopes_response = scopes(
            State(state.clone()),
            request(serde_json::json!({}), &"x".repeat(32)),
        )
        .await;
        let bytes = to_bytes(scopes_response.into_body(), MAX_SNAPSHOT_BYTES)
            .await
            .expect("body");
        assert_eq!(
            serde_json::from_slice::<Vec<ExcursionScope>>(&bytes).expect("scopes"),
            vec![scope()]
        );
        state.policy.revoke();
        assert_eq!(
            read(
                State(state),
                request(
                    serde_json::to_value(scope()).expect("scope"),
                    &"x".repeat(32)
                )
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn evidence_and_write_grants_are_independent_of_read_access() {
        let (_dir, state) = setup(vec![ExcursionAction::Read, ExcursionAction::Observe]);
        let mut command_value = ExcursionCommand {
            scope: scope(),
            command_id: "observe".into(),
            expected_revision: 0,
            operation: ExcursionOperation::Observe {
                incidents: vec![admiralty_core::excursions::ScopedIncident {
                    scope: scope(),
                    event: admiralty_core::excursions::IncidentEvent {
                        event_id: "failure".into(),
                        family_signature: "unknown".into(),
                        cause_parent: None,
                        kind: "failure".into(),
                        evidence_digests: vec![format!("sha256:{}", "c".repeat(64))],
                        observed_tick: None,
                    },
                }],
            },
        };
        let token = "x".repeat(32);
        assert_eq!(
            command(
                State(state.clone()),
                request(
                    serde_json::to_value(&command_value).expect("command"),
                    &token
                )
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        if let ExcursionOperation::Observe { incidents } = &mut command_value.operation {
            incidents[0].event.evidence_digests.clear();
        }
        assert_eq!(
            command(
                State(state.clone()),
                request(
                    serde_json::to_value(&command_value).expect("command"),
                    &token
                )
            )
            .await
            .status(),
            StatusCode::OK
        );
        let result = state
            .store
            .read_excursions(scope(), state.policy.authenticate(&token).expect("proof"))
            .expect("snapshot");
        let proposal = ExcursionCommand {
            scope: scope(),
            command_id: "propose".into(),
            expected_revision: 1,
            operation: ExcursionOperation::Propose {
                episode_id: result.state.expect("state").episodes[0].episode_id.clone(),
                reproduction: admiralty_core::excursions::ReproductionContract {
                    fields: Default::default(),
                },
            },
        };
        assert_eq!(
            command(
                State(state),
                request(serde_json::to_value(proposal).expect("proposal"), &token)
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
}
