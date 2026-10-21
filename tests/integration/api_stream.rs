use admiralty_core::{DomainEvent, SourceIdentity};
use admiralty_server::{MAX_EVENT_STREAMS, ServerConfig, app};
use admiralty_store::Store;
use admiralty_testkit::task_fixture;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "fixture-authentication-value-at-least-32-characters";

fn store_directory() -> TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-api-stream-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private API fixture directory")
}

struct HttpFixture {
    app: Router,
    store: Store,
    _directory: TempDir,
}

fn config(simulated: bool, assets: Option<PathBuf>) -> ServerConfig {
    ServerConfig {
        listen: "127.0.0.1:0"
            .parse::<SocketAddr>()
            .expect("loopback address"),
        simulated,
        token: Some(TOKEN.into()),
        assets,
        legacy: None,
        models_file: None,
    }
}

fn fixture(simulated: bool) -> HttpFixture {
    let directory = store_directory();
    let store = Store::open(directory.path().join("fleet.sqlite")).expect("fixture store");
    let app = app(store.clone(), config(simulated, None)).expect("qualified test app");
    HttpFixture {
        app,
        store,
        _directory: directory,
    }
}

fn append_task(store: &Store, sequence: u64, id: &str) {
    store
        .append(
            SourceIdentity {
                producer_id: "fixture-http".into(),
                incarnation: "boot-one".into(),
                sequence: sequence.to_string(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture(id),
            },
            "2026-10-18T12:00:00Z".into(),
        )
        .expect("durable fixture event");
}

fn append_real_task(store: &Store, sequence: u64, id: &str) {
    let mut contract = task_fixture(id);
    contract.simulated = false;
    contract.objective = "PRIVATE-TASK-DETAIL-MUST-NOT-APPEAR-IN-DEMO".into();
    store
        .append(
            SourceIdentity {
                producer_id: "fixture-http".into(),
                incarnation: "boot-one".into(),
                sequence: sequence.to_string(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-18T12:00:01Z".into(),
        )
        .expect("late real-task fixture event");
}

async fn request(app: &Router, method: &str, uri: &str, token: Option<&str>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("fixture HTTP request"))
        .await
        .expect("router response")
}

async fn json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded JSON body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn private_fleet_and_event_reads_require_correct_authentication() {
    let fixture = fixture(false);
    for uri in [
        "/v1/fleet",
        "/v1/events?after=0",
        "/v2/fleet",
        "/v2/events?after=0",
    ] {
        for token in [None, Some("wrong-token")] {
            let response = request(&fixture.app, "GET", uri, token).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "private read {uri}"
            );
            let error = json(response).await;
            assert!(
                error["error"]
                    .as_str()
                    .expect("API error")
                    .contains("authentication")
            );
            assert!(error.get("tasks").is_none());
        }
    }
    let response = request(&fixture.app, "GET", "/v1/fleet", Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["simulated"], false);
}

#[tokio::test]
async fn native_lifecycle_requires_explicit_v2_reads_and_ends_an_existing_v1_stream() {
    use admiralty_core::{
        LifecycleAction, LifecycleCommand, PrincipalId, TaskId, TaskView, content_digest,
    };
    use admiralty_store::LifecycleAuthority;
    use std::sync::Arc;
    struct FixtureAuthority;
    impl LifecycleAuthority for FixtureAuthority {
        fn authorize(
            &self,
            _: &SourceIdentity,
            _: &LifecycleCommand,
            _: &TaskView,
            effect: Option<&admiralty_store::LifecycleEffect<'_>>,
        ) -> bool {
            effect.is_none()
        }
    }
    let directory = store_directory();
    let store = Store::open_with_lifecycle_authority(
        directory.path().join("native.sqlite"),
        Arc::new(FixtureAuthority),
    )
    .expect("native HTTP fixture operation");
    append_task(&store, 1, "NATIVE");
    let router = app(store.clone(), config(false, None)).expect("native HTTP fixture operation");
    let mut old_stream = request(&router, "GET", "/v1/events?after=1", Some(TOKEN))
        .await
        .into_body()
        .into_data_stream();
    let task_id = TaskId::new("NATIVE").expect("native HTTP fixture operation");
    let task = &store
        .snapshot()
        .expect("native HTTP fixture operation")
        .projection
        .tasks[&task_id];
    store
        .execute_lifecycle(
            SourceIdentity {
                producer_id: "trusted-fixture".into(),
                incarnation: "one".into(),
                sequence: "1".into(),
            },
            LifecycleCommand {
                protocol_version: 2,
                task_id,
                expected_task_digest: content_digest(task).expect("native HTTP fixture operation"),
                actor: PrincipalId::new("owner").expect("native HTTP fixture operation"),
                action: LifecycleAction::CancelPending {
                    reason: "cancel fixture".into(),
                },
            },
            "2026-10-18T12:00:02Z".into(),
            None,
        )
        .expect("native HTTP fixture operation");
    for uri in ["/v1/fleet", "/v1/events?after=0"] {
        assert_eq!(
            request(&router, "GET", uri, Some(TOKEN)).await.status(),
            StatusCode::UPGRADE_REQUIRED
        );
    }
    let native = json(request(&router, "GET", "/v2/fleet", Some(TOKEN)).await).await;
    assert_eq!(native["schema_version"], 2);
    assert_eq!(native["tasks"][0]["state"], "cancelled");
    assert_eq!(native["native_execution_qualified"], false);
    let upgrade = tokio::time::timeout(Duration::from_secs(2), old_stream.next())
        .await
        .expect("native HTTP fixture operation")
        .expect("native HTTP fixture operation")
        .expect("native HTTP fixture operation");
    let upgrade = std::str::from_utf8(&upgrade).expect("native HTTP fixture operation");
    assert!(upgrade.contains("upgrade_required"));
    assert!(!upgrade.contains("cancel fixture"));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), old_stream.next())
            .await
            .expect("native HTTP fixture operation")
            .is_none()
    );
    let mut native_stream = request(&router, "GET", "/v2/events?after=1", Some(TOKEN))
        .await
        .into_body()
        .into_data_stream();
    let record = tokio::time::timeout(Duration::from_secs(2), native_stream.next())
        .await
        .expect("native HTTP fixture operation")
        .expect("native HTTP fixture operation")
        .expect("native HTTP fixture operation");
    let record = std::str::from_utf8(&record).expect("native HTTP fixture operation");
    assert!(record.contains("task_lifecycle") && record.contains("protocol_version"));
}

#[tokio::test]
async fn demo_response_marks_simulation_unknown_usage_and_unqualified_execution() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-DEMO");
    let response = request(&fixture.app, "GET", "/v1/fleet", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fleet = json(response).await;
    assert_eq!(fleet["schema_version"], 1);
    assert_eq!(fleet["simulated"], true);
    assert_eq!(fleet["native_execution_qualified"], false);
    assert_eq!(fleet["tasks"][0]["contract"]["simulated"], true);
    assert_eq!(fleet["projects"][0]["accepted"], 0);
    assert!(fleet["coordination"]["measured_tokens"].is_null());
    assert!(fleet["cursor"]["event_seq"].is_string());
    assert_eq!(fleet["cursor"]["event_seq"], "1");
}

#[test]
fn demo_mode_rejects_a_store_containing_real_task_contracts() {
    let directory = store_directory();
    let store = Store::open(directory.path().join("fleet.sqlite")).expect("fixture store");
    let mut contract = task_fixture("HTTP-REAL-TASK");
    contract.simulated = false;
    store
        .append(
            SourceIdentity {
                producer_id: "fixture-real".into(),
                incarnation: "boot-one".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-18T12:00:00Z".into(),
        )
        .expect("real-mode contract fixture");
    assert!(app(store, config(true, None)).is_err());
}

#[tokio::test]
async fn demo_reads_reject_real_task_appended_after_app_construction() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-INITIAL-DEMO");
    assert_eq!(
        request(&fixture.app, "GET", "/v1/fleet", None)
            .await
            .status(),
        StatusCode::OK
    );
    append_real_task(&fixture.store, 2, "HTTP-LATE-PRIVATE-TASK");
    for uri in ["/v1/fleet", "/v1/events?after=0"] {
        let response = request(&fixture.app, "GET", uri, None).await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "demo read {uri}");
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded demo-scope error");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("HTTP-LATE-PRIVATE-TASK"));
        assert!(!text.contains("PRIVATE-TASK-DETAIL-MUST-NOT-APPEAR-IN-DEMO"));
        assert!(text.contains("demo mode cannot expose private task state"));
    }
}

#[tokio::test]
async fn active_demo_stream_never_exposes_a_late_real_task() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-STREAM-INITIAL-DEMO");
    let snapshot = fixture.store.snapshot().expect("initial demo snapshot");
    let uri = format!(
        "/v1/events?after=1&generation={}",
        snapshot.cursor.recovery_generation
    );
    let response = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    append_real_task(&fixture.store, 2, "HTTP-LATE-STREAM-PRIVATE-TASK");
    let mut stream = response.into_body().into_data_stream();
    let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("demo scope-change signal must arrive")
        .expect("scope-change stream signal")
        .expect("SSE chunk");
    let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 scope-change event");
    assert!(frame.contains("event: snapshot_required"));
    assert!(!frame.contains("event: domain"));
    assert!(!frame.contains("HTTP-LATE-STREAM-PRIVATE-TASK"));
    assert!(!frame.contains("PRIVATE-TASK-DETAIL-MUST-NOT-APPEAR-IN-DEMO"));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("unsafe demo stream must terminate promptly")
            .is_none()
    );
}

#[tokio::test]
async fn read_only_router_has_no_mutation_endpoints() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-READ-ONLY");
    let before = fixture.store.snapshot().expect("before snapshot");
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        for uri in [
            "/v1/fleet",
            "/v1/tasks",
            "/v1/messages",
            "/v1/events",
            "/v1/terminal/input",
        ] {
            let response = request(&fixture.app, method, uri, Some(TOKEN)).await;
            assert!(
                matches!(
                    response.status(),
                    StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
                ),
                "unexpected mutation response {method} {uri}: {}",
                response.status()
            );
        }
    }
    assert_eq!(fixture.store.snapshot().expect("after snapshot"), before);
}

#[tokio::test]
async fn unknown_api_path_does_not_become_a_successful_spa_page() {
    let fixture = fixture(true);
    assert_eq!(
        request(&fixture.app, "GET", "/v1/unknown-route", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn generation_mismatch_and_future_cursor_require_a_new_snapshot() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-CURSOR");
    let snapshot = fixture.store.snapshot().expect("snapshot");
    for uri in [
        "/v1/events?after=0&generation=stale-generation".to_owned(),
        format!(
            "/v1/events?after=2&generation={}",
            snapshot.cursor.recovery_generation
        ),
    ] {
        let response = request(&fixture.app, "GET", &uri, None).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            json(response).await["error"]
                .as_str()
                .expect("resnapshot error")
                .contains("snapshot_required")
        );
    }
    let response = request(&fixture.app, "GET", "/v1/events?after=not-a-number", None).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn event_stream_replays_only_events_after_the_snapshot_cursor() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-BEFORE-SNAPSHOT");
    let snapshot = fixture.store.snapshot().expect("snapshot");
    append_task(&fixture.store, 2, "HTTP-AFTER-SNAPSHOT");
    let uri = format!(
        "/v1/events?after={}&generation={}",
        snapshot.cursor.event_seq, snapshot.cursor.recovery_generation
    );
    let response = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let mut stream = response.into_body().into_data_stream();
    let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("event must arrive")
        .expect("stream remains open")
        .expect("SSE chunk");
    let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 event");
    assert!(frame.contains("event: domain"));
    assert!(frame.contains("HTTP-AFTER-SNAPSHOT"));
    assert!(!frame.contains("HTTP-BEFORE-SNAPSHOT"));
    assert!(frame.contains("\"seq\":\"2\""));
    assert!(frame.contains(&format!("id: {}:2", snapshot.cursor.recovery_generation)));
}

#[tokio::test]
async fn queued_append_wakes_a_pending_stream_without_routine_model_turns() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-NOTIFY-INITIAL");
    let snapshot = fixture.store.snapshot().expect("notification snapshot");
    let uri = format!(
        "/v1/events?after=1&generation={}",
        snapshot.cursor.recovery_generation
    );
    let response = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let (pending_sender, pending_receiver) = tokio::sync::oneshot::channel();
    let waiting = tokio::spawn(async move {
        let mut pending_sender = Some(pending_sender);
        let next = futures_util::future::poll_fn(|context| {
            let result = stream.poll_next_unpin(context);
            if result.is_pending()
                && let Some(sender) = pending_sender.take()
            {
                let _ = sender.send(());
            }
            result
        });
        tokio::time::timeout(Duration::from_secs(2), next)
            .await
            .expect("durable append must wake the pending stream")
            .expect("stream receives the appended event")
            .expect("SSE append chunk")
    });
    tokio::time::timeout(Duration::from_secs(2), pending_receiver)
        .await
        .expect("stream reaches a pending read or change wait")
        .expect("pending stream handshake");
    append_task(&fixture.store, 2, "HTTP-NOTIFY-APPENDED");
    let bytes = waiting.await.expect("pending stream task joins");
    let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 notified event");
    assert!(frame.contains("event: domain"));
    assert!(frame.contains("HTTP-NOTIFY-APPENDED"));
    assert!(frame.contains("\"seq\":\"2\""));
    assert!(!frame.contains("HTTP-NOTIFY-INITIAL"));
    let fleet = json(request(&fixture.app, "GET", "/v1/fleet", None).await).await;
    assert_eq!(fleet["coordination"]["routine_model_turns"], 0);
    assert!(fleet["coordination"]["measured_tokens"].is_null());
}

#[tokio::test]
async fn cursor_and_stream_sequences_above_javascript_precision_remain_decimal_strings() {
    let directory = store_directory();
    let path = directory.path().join("fleet.sqlite");
    let store = Store::open(&path).expect("fixture store");
    append_task(&store, 1, "HTTP-PRECISION-INITIAL");
    drop(store);
    // Seed only this closed disposable database's next event identity. Native
    // producer sequence remains contiguous; no production database is accessed.
    let connection = rusqlite::Connection::open(&path).expect("isolated fixture SQL");
    assert_eq!(
        connection
            .execute(
                "UPDATE sqlite_sequence SET seq=?1 WHERE name='domain_events'",
                [9_007_199_254_740_992_i64],
            )
            .expect("seed large journal identity"),
        1
    );
    drop(connection);
    let store = Store::open(&path).expect("reopen fixture store");
    append_task(&store, 2, "HTTP-PRECISION-LARGE");
    let app = app(store, config(true, None)).expect("precision app");
    let fleet = json(request(&app, "GET", "/v1/fleet", None).await).await;
    assert_eq!(
        fleet["cursor"]["event_seq"].as_str(),
        Some("9007199254740993")
    );
    let generation = fleet["cursor"]["recovery_generation"]
        .as_str()
        .expect("generation string");
    let uri = format!("/v1/events?after=9007199254740992&generation={generation}");
    let response = request(&app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("large event arrives")
        .expect("stream event")
        .expect("SSE chunk");
    let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 event");
    assert!(frame.contains(&format!("id: {generation}:9007199254740993")));
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("SSE JSON data");
    let record: Value = serde_json::from_str(data).expect("event JSON");
    assert_eq!(record["seq"].as_str(), Some("9007199254740993"));
}

#[tokio::test]
async fn active_event_stream_requests_resnapshot_after_recovery_generation_changes() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-BEFORE-RECOVERY");
    let old = fixture.store.snapshot().expect("old snapshot");
    let uri = format!(
        "/v1/events?after=1&generation={}",
        old.cursor.recovery_generation
    );
    let response = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let recovered = fixture
        .store
        .rebuild()
        .expect("explicit recovery generation reset");
    assert_ne!(
        old.cursor.recovery_generation,
        recovered.cursor.recovery_generation
    );
    append_task(&fixture.store, 2, "HTTP-AFTER-RECOVERY");
    let mut stream = response.into_body().into_data_stream();
    let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("recovery signal must arrive")
        .expect("stream signal")
        .expect("SSE chunk");
    let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 event");
    assert!(
        frame.contains("event: snapshot_required"),
        "stale stream emitted {frame}"
    );
    assert!(!frame.contains("HTTP-AFTER-RECOVERY"));
}

#[tokio::test]
async fn concurrent_event_stream_admission_is_bounded_and_drop_releases_its_slot() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-STREAM-LIMIT");
    let snapshot = fixture.store.snapshot().expect("stream-limit snapshot");
    let uri = format!(
        "/v1/events?after=1&generation={}",
        snapshot.cursor.recovery_generation
    );
    let mut admitted = Vec::new();
    for _ in 0..MAX_EVENT_STREAMS {
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            request(&fixture.app, "GET", &uri, None),
        )
        .await
        .expect("admission must not retain a shared snapshot read permit");
        assert_eq!(response.status(), StatusCode::OK);
        // An unpolled response still owns a live stream body and consumes one slot.
        admitted.push(response);
    }
    let saturated = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        json(saturated).await["error"]
            .as_str()
            .expect("saturation error")
            .contains("event stream limit")
    );
    let fleet = tokio::time::timeout(
        Duration::from_secs(2),
        request(&fixture.app, "GET", "/v1/fleet", None),
    )
    .await
    .expect("full stream capacity cannot block ordinary snapshot reads");
    assert_eq!(fleet.status(), StatusCode::OK);
    drop(admitted.pop().expect("admitted response body"));
    let replacement = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(
        replacement.status(),
        StatusCode::OK,
        "dropping body must immediately release stream capacity"
    );
    admitted.push(replacement);
    assert_eq!(
        request(&fixture.app, "GET", &uri, None).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn terminal_recovery_signal_releases_stream_capacity_without_dropping_body() {
    let fixture = fixture(true);
    append_task(&fixture.store, 1, "HTTP-TERMINAL-STREAM-LIMIT");
    let snapshot = fixture.store.snapshot().expect("terminal stream snapshot");
    let uri = format!(
        "/v1/events?after=1&generation={}",
        snapshot.cursor.recovery_generation
    );
    let response = request(&fixture.app, "GET", &uri, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut held = Vec::new();
    for _ in 1..MAX_EVENT_STREAMS {
        let response = request(&fixture.app, "GET", &uri, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    assert_eq!(
        request(&fixture.app, "GET", &uri, None).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let recovered = fixture
        .store
        .rebuild()
        .expect("recovery invalidates active streams");
    let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("recovery signal arrives")
        .expect("recovery event")
        .expect("SSE chunk");
    assert!(String::from_utf8_lossy(&bytes).contains("event: snapshot_required"));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("terminal stream closes")
            .is_none()
    );
    let fresh = format!(
        "/v1/events?after=1&generation={}",
        recovered.cursor.recovery_generation
    );
    assert_eq!(
        request(&fixture.app, "GET", &fresh, None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        held.len(),
        MAX_EVENT_STREAMS - 1,
        "other bodies remain live during replacement admission"
    );
}

#[tokio::test]
async fn static_assets_reject_traversal_and_missing_files_but_allow_spa_routes() {
    let fixture = fixture(true);
    let assets = fixture._directory.path().join("assets");
    std::fs::create_dir(&assets).expect("asset directory");
    std::fs::write(
        assets.join("index.html"),
        "<title>Fixture dashboard</title>",
    )
    .expect("index asset");
    std::fs::write(assets.join("app.js"), "console.log('fixture');").expect("JS asset");
    std::fs::write(
        fixture._directory.path().join("outside.txt"),
        "OUTSIDE-ASSET-ROOT",
    )
    .expect("outside fixture");
    let app = app(fixture.store.clone(), config(true, Some(assets))).expect("asset router");
    let response = request(&app, "GET", "/app.js", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    for uri in [
        "/../outside.txt",
        "/%2e%2e/outside.txt",
        "/..%2foutside.txt",
        "/%2e%2e%5coutside.txt",
        "/missing.js",
        "/v1/missing-api",
    ] {
        let response = request(&app, "GET", uri, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "path {uri}");
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded error body");
        assert!(!String::from_utf8_lossy(&bytes).contains("OUTSIDE-ASSET-ROOT"));
    }
    let response = request(&app, "GET", "/projects/example", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("SPA body");
    assert!(String::from_utf8_lossy(&bytes).contains("Fixture dashboard"));
}

#[cfg(unix)]
#[tokio::test]
async fn static_assets_cannot_follow_file_or_directory_symlinks_outside_root() {
    use std::os::unix::fs::symlink;
    let fixture = fixture(true);
    let assets = fixture._directory.path().join("assets");
    let outside = fixture._directory.path().join("outside");
    std::fs::create_dir(&assets).expect("assets");
    std::fs::create_dir(&outside).expect("outside directory");
    std::fs::write(assets.join("index.html"), "<title>Fixture</title>").expect("index");
    std::fs::write(outside.join("private.txt"), "OUTSIDE-SYMLINK-CONTENT").expect("outside file");
    symlink(outside.join("private.txt"), assets.join("escape.txt")).expect("file symlink");
    symlink(&outside, assets.join("external")).expect("directory symlink");
    let app = app(fixture.store.clone(), config(true, Some(assets))).expect("asset router");
    for uri in ["/escape.txt", "/external/private.txt"] {
        let response = request(&app, "GET", uri, None).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "symlink escape {uri}"
        );
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("error body");
        assert!(!String::from_utf8_lossy(&bytes).contains("OUTSIDE-SYMLINK-CONTENT"));
    }
}

#[test]
fn app_rejects_remote_binding_and_missing_private_authentication() {
    let fixture = fixture(true);
    let mut remote = config(true, None);
    remote.listen = "0.0.0.0:0".parse().expect("remote address");
    assert!(app(fixture.store.clone(), remote).is_err());
    for token in [None, Some("short".into())] {
        let mut private = config(false, None);
        private.token = token;
        assert!(app(fixture.store.clone(), private).is_err());
    }
}

#[tokio::test]
async fn legacy_bridge_keeps_real_facts_out_of_demo_and_preserves_native_auth() {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    let index = directory.path().join("index.html");
    std::fs::write(&status, br#"{"generated_utc":"2026-10-18T12:00:00Z","order":["example"],"projects":{"example":{"name":"Example","done":3}},"rollout":{"project":{"id":"build"}}}"#).expect("legacy facts");
    std::fs::write(
        &index,
        b"<!doctype html><title>Original detailed view</title>",
    )
    .expect("legacy page");
    let store = Store::open(directory.path().join("native.sqlite")).expect("store");
    let mut configured = config(false, None);
    configured.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status.clone(),
        index_file: Some(index),
        public_read: true,
    });
    let router = app(store.clone(), configured.clone()).expect("explicit public legacy scope");
    let response = request(&router, "GET", "/v1/legacy", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["projects"]["example"]["done"], 3);
    assert_eq!(
        request(&router, "GET", "/v1/fleet", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&router, "GET", "/legacy/", None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&router, "GET", "/legacy/status.json", None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&router, "GET", "/legacy/secrets", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    configured.simulated = true;
    assert!(app(store.clone(), configured.clone()).is_err());
    configured.simulated = false;
    configured
        .legacy
        .as_mut()
        .expect("legacy configured")
        .public_read = false;
    let private = app(store, configured).expect("private legacy scope");
    assert_eq!(
        request(&private, "GET", "/v1/legacy", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&private, "GET", "/v1/legacy", Some(TOKEN))
            .await
            .status(),
        StatusCode::OK
    );
    std::fs::write(&status, b"broken JSON").expect("inject malformed source");
    assert_eq!(
        request(&private, "GET", "/v1/legacy", Some(TOKEN))
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn legacy_route_without_retained_page_redirects_to_react_detail_view_under_same_scope() {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    std::fs::write(
        &status,
        br#"{"generated_utc":"2026-10-18T12:00:00Z","order":[],"projects":{}}"#,
    )
    .expect("legacy facts");
    let store = Store::open(directory.path().join("native.sqlite")).expect("store");
    let mut configured = config(false, None);
    configured.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status,
        index_file: None,
        public_read: false,
    });
    let router = app(store, configured).expect("status-only legacy bridge");
    // The redirect never leaks private facts and keeps the same access scope.
    assert_eq!(
        request(&router, "GET", "/legacy/", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = request(&router, "GET", "/legacy/", Some(TOKEN)).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some(admiralty_server::legacy::REACT_DETAILED_VIEW)
    );
    assert_eq!(
        request(&router, "GET", "/v1/legacy", Some(TOKEN))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn dashboard_compression_is_negotiated_without_bypassing_auth_or_source_validation() {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    let assets = directory.path().join("assets");
    std::fs::create_dir(&assets).expect("assets");
    let source = serde_json::json!({"generated_utc":"2026-10-19T12:00:00Z",
        "order":["real"],"projects":{"real":{"name":"real source", "log":"Unicode 🧭\n".repeat(10_000)}}}).to_string();
    std::fs::write(&status, &source).expect("source");
    let script = "console.log('dashboard asset');\n".repeat(1_000);
    std::fs::write(assets.join("index.js"), &script).expect("asset");
    std::fs::create_dir(assets.join("assets")).expect("hashed assets");
    std::fs::write(assets.join("assets/index-12345678.js"), &script).expect("hashed asset");
    let image = include_bytes!("../../apps/dashboard/src/assets/admiralty-mark.webp");
    std::fs::write(assets.join("assets/admiralty-mark-abc_12--.webp"), image)
        .expect("hashed image");
    std::fs::write(assets.join("admiralty-mark.webp"), image).expect("unversioned image");
    let store = Store::open(directory.path().join("native.sqlite")).expect("store");
    let mut settings = config(false, Some(assets));
    settings.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status.clone(),
        index_file: Some(directory.path().join("original.html")),
        public_read: true,
    });
    let router = app(store.clone(), settings.clone()).expect("router");
    let encoded = |router: Router, path: &'static str, encoding: &'static str| async move {
        router
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::ACCEPT_ENCODING, encoding)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response")
    };
    let cached = encoded(router.clone(), "/assets/index-12345678.js", "gzip").await;
    assert_eq!(
        cached.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let image_response = encoded(
        router.clone(),
        "/assets/admiralty-mark-abc_12--.webp",
        "gzip",
    )
    .await;
    assert_eq!(image_response.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(
        image_response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert!(
        !image_response
            .headers()
            .contains_key(header::CONTENT_ENCODING)
    );
    assert_eq!(
        to_bytes(image_response.into_body(), image.len())
            .await
            .expect("image body")
            .as_ref(),
        image
    );
    assert_eq!(
        encoded(router.clone(), "/admiralty-mark.webp", "gzip")
            .await
            .headers()[header::CACHE_CONTROL],
        "no-cache"
    );
    let initial =
        json(encoded(router.clone(), "/v1/legacy/detail/initial", "identity").await).await;
    assert_eq!(initial["admiralty_delivery"], "initial");
    assert_eq!(initial["generated_utc"], "2026-10-19T12:00:00Z");
    assert_eq!(initial["projects"]["real"]["name"], "real source");
    assert!(initial["projects"]["real"].get("log").is_none());
    let detail = json(encoded(router.clone(), "/v1/legacy/detail", "identity").await).await;
    assert_eq!(detail["admiralty_delivery"], "complete");
    assert_eq!(
        detail["projects"]["real"]["log"],
        serde_json::from_str::<Value>(&source).expect("source")["projects"]["real"]["log"]
    );
    for (path, original) in [
        ("/v1/legacy", source.as_bytes()),
        ("/legacy/status.json", source.as_bytes()),
        ("/index.js", script.as_bytes()),
    ] {
        let response = encoded(router.clone(), path, "gzip, br").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        let body = to_bytes(response.into_body(), original.len())
            .await
            .expect("compressed body");
        assert_eq!(&body[..2], &[0x1f, 0x8b]);
        assert!(
            body.len() < original.len() / 4,
            "bounded transfer reduction"
        );
        let identity = encoded(router.clone(), path, "gzip;q=0, *;q=1").await;
        assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(
            to_bytes(identity.into_body(), original.len())
                .await
                .expect("identity body")
                .as_ref(),
            original
        );
    }
    settings.legacy.as_mut().expect("legacy").public_read = false;
    let private = app(store, settings).expect("private router");
    for path in ["/v1/legacy/detail", "/v1/legacy/detail/initial"] {
        assert_eq!(
            encoded(private.clone(), path, "gzip").await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let denied = encoded(private.clone(), "/v1/legacy", "gzip").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert!(!denied.headers().contains_key(header::CONTENT_ENCODING));
    std::fs::write(&status, "malformed").expect("broken source");
    let response = private
        .oneshot(
            Request::builder()
                .uri("/v1/legacy")
                .header(header::ACCEPT_ENCODING, "gzip")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
}

#[tokio::test]
async fn model_configuration_requires_auth_and_exact_revision_and_never_exposes_connection_refs() {
    let directory = store_directory();
    let private = directory.path().join("private-models");
    std::fs::create_dir(&private).expect("private model root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
            .expect("private mode");
    }
    let registry_path = private.join("models.json");
    let registry =
        admiralty_runtime::models::RegistryFile::new(&registry_path).expect("private registry");
    let desired: admiralty_runtime::models::ModelRegistry =
        serde_json::from_str(include_str!("../../configs/examples/models.json"))
            .expect("model example");
    registry
        .replace("0", desired.models)
        .expect("seed model registry");
    let store = Store::open(directory.path().join("native.sqlite")).expect("native store");
    let mut configured = config(false, None);
    configured.models_file = Some(registry_path);
    let router = app(store, configured).expect("model API app");
    assert_eq!(
        request(&router, "GET", "/v1/models", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let public = json(request(&router, "GET", "/v1/models", Some(TOKEN)).await).await;
    assert_eq!(public["revision"], "1");
    let rows = public["models"].as_array().expect("public models");
    assert!(
        rows.iter()
            .all(|row| row.get("endpoint").is_none() && row.get("credential_file_ref").is_none())
    );
    let before = registry.read().expect("before update");
    let mut models = public["models"].clone();
    for row in models.as_array_mut().expect("rows") {
        row.as_object_mut().expect("model").remove("usage_cost");
    }
    let payload = serde_json::json!({"expected_revision":"1","models":models});
    let update = |token: Option<&str>, body: Value| {
        let mut builder = Request::builder()
            .method("PUT")
            .uri("/v1/models")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder
            .body(Body::from(body.to_string()))
            .expect("model update")
    };
    let denied = router
        .clone()
        .oneshot(update(None, payload.clone()))
        .await
        .expect("unauthenticated update");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let saved = router
        .clone()
        .oneshot(update(Some(TOKEN), payload.clone()))
        .await
        .expect("save public rows");
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        registry.read().expect("preserved connection config").models,
        before.models
    );
    let stale = router
        .clone()
        .oneshot(update(Some(TOKEN), payload))
        .await
        .expect("stale update");
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn visual_feed_is_private_atomic_and_reconnects_to_current_projection() {
    let f = fixture(false);
    for path in ["/v2/views", "/v2/views/events"] {
        assert_eq!(
            request(&f.app, "GET", path, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let first = json(request(&f.app, "GET", "/v2/views", Some(TOKEN)).await).await;
    assert_eq!(first["schema_version"], 1);
    let response = request(&f.app, "GET", "/v2/views/events", Some(TOKEN)).await;
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let mut stream = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("initial cut")
        .expect("frame")
        .expect("bytes");
    let frame = String::from_utf8(frame.to_vec()).expect("utf8");
    assert!(frame.contains("event: views"));
    assert!(frame.contains(first["revision"].as_str().expect("revision")));
    append_real_task(&f.store, 1, "visual-project");
    let changed = tokio::time::timeout(Duration::from_secs(4), stream.next())
        .await
        .expect("changed cut")
        .expect("frame")
        .expect("bytes");
    let changed = String::from_utf8(changed.to_vec()).expect("utf8");
    assert!(changed.contains("project"));
    assert!(!changed.contains(first["revision"].as_str().expect("old revision")));
    drop(stream);
    let latest = json(request(&f.app, "GET", "/v2/views", Some(TOKEN)).await).await;
    assert_ne!(latest["revision"], first["revision"]);
    assert_eq!(latest["projects"].as_array().expect("projects").len(), 1);
}

#[tokio::test]
async fn visual_producer_failure_keeps_last_known_inventory_and_public_legacy_flag_never_grants_access()
 {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    let source = serde_json::json!({"generated_utc":"2026-10-18T12:00:00Z","host":"node","order":["demo-harbor"],"projects":{},"project_catalog":[{"id":"demo-harbor","name":"Producer project"}],
        "hierarchy":{"captains":[{"key":"demo-harbor","screen":"77.manager","host":"node","agent_running":true}]},
        "agents":[{"id":"12.worker","screen":"worker","host":"node","demo-harbor":"demo-harbor","model":null},{"id":"77.manager","screen":"manager","host":"node","demo-harbor":"demo-harbor","model":null}],
        "git":{"demo-harbor":{"instance_id":"retained-instance","node":"node","observed_utc":"2026-10-18T11:00:00Z","error":null,"shallow":false,"truncated":false,"commits":[],"refs":[],"checkouts":[]}}});
    std::fs::write(&status, source.to_string()).expect("source");
    let store = Store::open(directory.path().join("fleet.sqlite")).expect("store");
    append_real_task(&store, 1, "native-and-git");
    let mut settings = config(false, None);
    settings.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status.clone(),
        index_file: Some(directory.path().join("index.html")),
        public_read: true,
    });
    let router = app(store, settings).expect("router");
    assert_eq!(
        request(&router, "GET", "/v2/views/events", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let first = json(request(&router, "GET", "/v2/views", Some(TOKEN)).await).await;
    std::fs::write(&status, "malformed").expect("failure");
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let stale = json(request(&router, "GET", "/v2/views", Some(TOKEN)).await).await;
    assert_eq!(stale["projects"][0]["id"], "demo-harbor");
    assert_eq!(
        stale["projects"][0]["git"]["instance_id"],
        "retained-instance"
    );
    assert_eq!(
        stale["projects"][0]["git"]["observed_utc"],
        "2026-10-18T11:00:00Z"
    );
    let managers = stale["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .filter(|a| a["role"] == "commodore")
        .collect::<Vec<_>>();
    assert_eq!(managers.len(), 1);
    assert_eq!(managers[0]["presence"], "observed");
    assert_eq!(managers[0]["id"], "screen:node:77.manager");
    let original = first["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|a| a["role"] == "worker")
        .expect("worker");
    let retained = stale["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|a| a["id"] == original["id"])
        .expect("retained");
    assert_eq!(retained["observed_utc"], original["observed_utc"]);
    assert!(
        stale["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|s| s["state"] == "error")
    );
}

#[tokio::test]
async fn snapshot_projection_cache_revalidates_with_etag_and_tracks_source_replacement() {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    let write = |generated: &str| {
        let source = serde_json::json!({"generated_utc":generated,"order":["real"],
            "projects":{"real":{"name":"real source","log":"x".repeat(4_096)}},
            "git":{"real":{"commits":[]}}})
        .to_string();
        // Atomic replacement, exactly like the observer's publisher.
        let next = directory.path().join(".status.tmp");
        std::fs::write(&next, &source).expect("source");
        std::fs::rename(&next, &status).expect("publish");
    };
    write("2026-10-21T00:00:00Z");
    let store = Store::open(directory.path().join("native.sqlite")).expect("store");
    let mut settings = config(false, None);
    settings.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status.clone(),
        index_file: Some(directory.path().join("original.html")),
        public_read: true,
    });
    let router = app(store, settings).expect("router");
    let get = |router: Router, etag: Option<String>| async move {
        let mut builder = Request::builder()
            .uri("/v1/legacy/detail")
            .header(header::ACCEPT_ENCODING, "gzip");
        if let Some(etag) = etag {
            builder = builder.header(header::IF_NONE_MATCH, etag);
        }
        router
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response")
    };
    let first = get(router.clone(), None).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["x-admiralty-snapshot-cache"], "miss");
    assert_eq!(first.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
    let etag = first.headers()[header::ETAG]
        .to_str()
        .expect("etag")
        .to_owned();
    assert!(etag.starts_with('"') && etag.ends_with("-gz\""), "{etag}");
    let first_body = to_bytes(first.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let second = get(router.clone(), None).await;
    assert_eq!(second.headers()["x-admiralty-snapshot-cache"], "hit");
    assert_eq!(second.headers()[header::ETAG], etag.as_str());
    assert_eq!(
        to_bytes(second.into_body(), 1024 * 1024)
            .await
            .expect("cached body"),
        first_body,
        "a cache hit must be byte-identical to the original projection"
    );
    let unchanged = get(router.clone(), Some(etag.clone())).await;
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(unchanged.headers()[header::ETAG], etag.as_str());
    assert!(
        to_bytes(unchanged.into_body(), 16)
            .await
            .expect("empty")
            .is_empty()
    );
    // A newly published snapshot is a new identity: fresh projection and tag.
    write("2026-10-21T00:00:15Z");
    let replaced = get(router.clone(), Some(etag.clone())).await;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(replaced.headers()["x-admiralty-snapshot-cache"], "miss");
    assert_ne!(replaced.headers()[header::ETAG], etag.as_str());
    // Identity clients get a different representation tag, never gzip bytes.
    let identity = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/legacy/detail")
                .header(header::ACCEPT_ENCODING, "identity")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
    assert!(
        !identity.headers()[header::ETAG]
            .to_str()
            .expect("tag")
            .ends_with("-gz\"")
    );
    let detail = json(identity).await;
    assert_eq!(detail["generated_utc"], "2026-10-21T00:00:15Z");
    assert!(detail.get("git").is_none());
    // Malformed replacement still fails closed instead of serving the cache.
    std::fs::write(directory.path().join(".bad"), b"malformed").expect("bad");
    std::fs::rename(directory.path().join(".bad"), &status).expect("publish bad");
    assert_eq!(get(router, None).await.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn agent_brief_and_readiness_use_the_bounded_producer_feed_without_leaking_paths() {
    let directory = store_directory();
    let status = directory.path().join("status.json");
    let generated = utc_now_seconds();
    std::fs::write(
        &status,
        serde_json::json!({"generated_utc":generated,"host":"node","order":["p"],
            "collector":{"duration_ms":12,"model_turns":0},
            "projects":{"p":{"done":3,"total":4,"counts":{"in_progress":1,"blocked":0}}},
            "git_health":{"p":{"state":"attention","actionable":true,"fingerprint":"sha256:abcdef0123",
                "issues":[{"code":"merge_conflicts"}]}}})
        .to_string(),
    )
    .expect("source");
    let assets = directory.path().join("assets");
    std::fs::create_dir(&assets).expect("assets");
    let store = Store::open(directory.path().join("native.sqlite")).expect("store");
    let mut settings = config(false, Some(assets.clone()));
    settings.legacy = Some(admiralty_server::legacy::LegacyConfig {
        status_file: status.clone(),
        index_file: Some(directory.path().join("original.html")),
        public_read: true,
    });
    let router = app(store.clone(), settings.clone()).expect("router");
    let brief = request(&router, "GET", "/v1/brief?budget=400", None).await;
    assert_eq!(brief.status(), StatusCode::OK);
    assert_eq!(
        brief.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    assert_eq!(brief.headers()["x-admiralty-brief-omitted"], "0");
    let text = String::from_utf8(
        to_bytes(brief.into_body(), 4096)
            .await
            .expect("brief")
            .to_vec(),
    )
    .expect("utf-8");
    assert!(
        text.starts_with(&format!("# fleet gen={generated} ")),
        "{text}"
    );
    assert!(
        text.contains("\n! git p attention merge_conflicts fp=abcdef01"),
        "{text}"
    );
    assert!(text.contains("proj p done=3/4 ip=1 blk=0"), "{text}");
    assert!(text.len() <= 400);
    let structured =
        json(request(&router, "GET", "/v1/brief?format=json&project=p", None).await).await;
    assert_eq!(structured["omitted_lines"], 0);
    assert_eq!(structured["stale"], false);
    for bad in ["/v1/brief?format=yaml", "/v1/brief?project=../etc"] {
        assert_eq!(
            request(&router, "GET", bad, None).await.status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    // Readiness: assets missing index.html -> 503 with reasons, never paths.
    let not_ready = request(&router, "GET", "/readyz", None).await;
    assert_eq!(not_ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json(not_ready).await;
    assert_eq!(body["ready"], false);
    assert_eq!(body["checks"]["producer_feed"]["ok"], true);
    assert_eq!(body["checks"]["dashboard_assets"]["ok"], false);
    assert!(
        !body
            .to_string()
            .contains(directory.path().to_str().expect("path"))
    );
    std::fs::write(assets.join("index.html"), "<!doctype html>").expect("index");
    let ready = request(&router, "GET", "/readyz", None).await;
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(json(ready).await["ready"], true);
    // Private scope still applies to the brief.
    settings.legacy.as_mut().expect("legacy").public_read = false;
    let private = app(store, settings).expect("private");
    assert_eq!(
        request(&private, "GET", "/v1/brief", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&private, "GET", "/v1/brief", Some(TOKEN))
            .await
            .status(),
        StatusCode::OK
    );
}

/// RFC 3339 UTC "now" without adding a date dependency to the test kit.
fn utc_now_seconds() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}
