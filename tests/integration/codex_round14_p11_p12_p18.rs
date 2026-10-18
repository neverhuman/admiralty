use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, FileChange, analyze_conflicts},
};
use admiralty_server::{ServerConfig, app};
use admiralty_store::Store;
use admiralty_testkit::task_fixture;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "round14-stream-token-with-at-least-thirty-two-chars";

fn candidate(id: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round14-project").expect("valid project id"),
        root_id: RootId::new("round14-root").expect("valid root id"),
        task_id: TaskId::new(id).expect("valid task id"),
        candidate_id: id.to_owned(),
        base_sha: format!("sha256:{}", "a".repeat(64)),
        head_sha: format!("sha256:{}", "b".repeat(64)),
        tree_sha: format!("sha256:{}", "c".repeat(64)),
        ancestry_digest: Some(format!("sha256:{}", "d".repeat(64))),
        files: vec![FileChange {
            path: "crates/admiralty-runtime/src/lib.rs".to_owned(),
            hunks: None,
        }],
        constraints: None,
    }
}

fn private_directory() -> TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round14-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private deterministic fixture")
}

#[tokio::test]
async fn codex_round14_p11_p12_p18() {
    let candidates = [candidate("round14-a"), candidate("round14-b")];
    let first = analyze_conflicts(&candidates, "round14-policy").expect("conflict generation");
    let replay = analyze_conflicts(
        &[candidates[1].clone(), candidates[0].clone()],
        "round14-policy",
    )
    .expect("replayed conflict generation");
    assert_eq!(
        first, replay,
        "candidate arrival order cannot change arbitration"
    );
    assert_eq!(first.generation.candidate_ids, ["round14-a", "round14-b"]);
    assert!(first.generation.combined_verification_required);
    assert!(
        first.edges[0]
            .risks
            .contains(&ConflictRisk::UnknownWriteScope)
    );

    let mut invalid = candidate("round14-invalid");
    invalid.files[0].path = "../outside-boundary".to_owned();
    assert!(matches!(
        analyze_conflicts(&[invalid], "round14-policy"),
        Err(admiralty_core::conflicts::ConflictError::InvalidPath(_))
    ));

    let directory = private_directory();
    let store = Store::open(directory.path().join("round14.sqlite")).expect("fixture store");
    store
        .append(
            SourceIdentity {
                producer_id: "round14-fixture".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture("round14-before-cursor"),
            },
            "2026-10-18T17:50:00Z".into(),
        )
        .expect("first durable receipt boundary");
    let snapshot = store.snapshot().expect("receipt boundary snapshot");
    store
        .append(
            SourceIdentity {
                producer_id: "round14-fixture".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture("round14-after-cursor"),
            },
            "2026-10-18T17:50:01Z".into(),
        )
        .expect("second durable receipt boundary");
    let router = app(
        store,
        ServerConfig {
            listen: "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("loopback address"),
            simulated: false,
            token: Some(TOKEN.to_owned()),
            assets: None,
            legacy: None,
            models_file: None,
        },
    )
    .expect("authenticated simulated API");

    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1")
                .body(Body::empty())
                .expect("unauthenticated stream request"),
        )
        .await
        .expect("denied stream response");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied_body = to_bytes(denied.into_body(), 4096)
        .await
        .expect("bounded denial response");
    assert!(!String::from_utf8_lossy(&denied_body).contains("round14-after-cursor"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=stale")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale receipt cursor request"),
        )
        .await
        .expect("fail-closed stale cursor response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let uri = format!(
        "/v1/events?after={}&generation={}",
        snapshot.cursor.event_seq, snapshot.cursor.recovery_generation
    );
    let stream = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authorized bounded replay request"),
        )
        .await
        .expect("authorized event stream response");
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.into_body().into_data_stream().next(),
    )
    .await
    .expect("replay frame arrives within fixture timeout")
    .expect("stream yields a frame")
    .expect("valid event frame");
    let frame = String::from_utf8(frame.to_vec()).expect("event frame is UTF-8");
    assert!(frame.contains("round14-after-cursor"));
    assert!(!frame.contains("round14-before-cursor"));
    assert!(frame.contains("\"seq\":\"2\""));
}
