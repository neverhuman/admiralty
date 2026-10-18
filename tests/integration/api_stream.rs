use admiralty_core::{DomainEvent, SourceIdentity};
use admiralty_server::{ServerConfig, app};
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
    }
}

fn fixture(simulated: bool) -> HttpFixture {
    let directory = tempfile::tempdir().expect("isolated fixture directory");
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
    for uri in ["/v1/fleet", "/v1/events?after=0"] {
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
    let directory = tempfile::tempdir().expect("fixture directory");
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
async fn cursor_and_stream_sequences_above_javascript_precision_remain_decimal_strings() {
    let directory = tempfile::tempdir().expect("isolated precision fixture");
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
