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

const TOKEN: &str = "round-fifteen-private-stream-token-over-32-chars";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round15-project").expect("valid project id"),
        root_id: RootId::new("round15-root").expect("valid root id"),
        task_id: TaskId::new(id).expect("valid task id"),
        candidate_id: id.to_owned(),
        base_sha: format!("sha256:{}", "a".repeat(64)),
        head_sha: format!("sha256:{}", "b".repeat(64)),
        tree_sha: format!("sha256:{}", "c".repeat(64)),
        ancestry_digest: Some(format!("sha256:{}", "d".repeat(64))),
        files: vec![FileChange {
            path: path.to_owned(),
            hunks: None,
        }],
        constraints: None,
    }
}

fn private_directory() -> TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round15-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private release-smoke fixture")
}

fn append_task(store: &Store, sequence: u8, task_id: &str) {
    store
        .append(
            SourceIdentity {
                producer_id: "round15-release-smoke".into(),
                incarnation: "boot-1".into(),
                sequence: sequence.to_string(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture(task_id),
            },
            format!("2026-10-18T18:01:{sequence:02}Z"),
        )
        .expect("append deterministic release-smoke event");
}

#[tokio::test]
async fn codex_round15_p11_p12_p18_release_smoke_and_multistep_stress() {
    let mut candidates = (0..12)
        .map(|index| candidate(&format!("round15-candidate-{index:02}"), "crates/shared.rs"))
        .collect::<Vec<_>>();
    let forward = analyze_conflicts(&candidates, "round15-arbitration")
        .expect("bounded multi-candidate arbitration");
    candidates.reverse();
    let reverse = analyze_conflicts(&candidates, "round15-arbitration")
        .expect("replayed multi-candidate arbitration");
    assert_eq!(forward, reverse, "arrival order cannot alter replay output");
    assert_eq!(forward.generation.candidate_ids.len(), 12);
    assert!(forward.generation.combined_verification_required);
    assert_eq!(
        forward.edges.len(),
        66,
        "all overlapping pairs are retained"
    );
    assert!(
        forward
            .edges
            .iter()
            .all(|edge| { edge.risks.contains(&ConflictRisk::UnknownWriteScope) })
    );

    let mut corrupted = candidate("round15-corrupted", "crates/shared.rs");
    corrupted.tree_sha = "sha256:not-a-valid-digest".to_owned();
    assert!(matches!(
        analyze_conflicts(&[corrupted], "round15-arbitration"),
        Err(admiralty_core::conflicts::ConflictError::InvalidCandidate(
            _
        ))
    ));

    let directory = private_directory();
    let database = directory.path().join("round15.sqlite");
    let store = Store::open(&database).expect("open release-smoke event store");
    append_task(&store, 1, "round15-before-snapshot-1");
    append_task(&store, 2, "round15-before-snapshot-2");
    let receipt_boundary = store.snapshot().expect("capture durable receipt boundary");
    append_task(&store, 3, "round15-after-snapshot-1");
    append_task(&store, 4, "round15-after-snapshot-2");

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
    .expect("private streaming API");

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=2")
                .body(Body::empty())
                .expect("unauthorized replay request"),
        )
        .await
        .expect("unauthorized replay response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let denied_bytes = to_bytes(unauthorized.into_body(), 4096)
        .await
        .expect("bounded authorization error");
    assert!(!String::from_utf8_lossy(&denied_bytes).contains("round15-after-snapshot"));

    let stale_uri = format!(
        "/v1/events?after=2&generation=stale-round15-{}",
        receipt_boundary.cursor.recovery_generation
    );
    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(stale_uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale boundary request"),
        )
        .await
        .expect("stale boundary response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let uri = format!(
        "/v1/events?after={}&generation={}",
        receipt_boundary.cursor.event_seq, receipt_boundary.cursor.recovery_generation
    );
    let stream = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authorized receipt-boundary replay request"),
        )
        .await
        .expect("authorized replay stream");
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    let mut frames = stream.into_body().into_data_stream();
    for (sequence, expected_task) in [
        ("3", "round15-after-snapshot-1"),
        ("4", "round15-after-snapshot-2"),
    ] {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
            .await
            .expect("bounded replay frame timeout")
            .expect("replay stream remains open")
            .expect("replay frame is valid");
        let frame = String::from_utf8(bytes.to_vec()).expect("UTF-8 event frame");
        assert!(frame.contains(expected_task));
        assert!(frame.contains(&format!("\"seq\":\"{sequence}\"")));
        assert!(!frame.contains("round15-before-snapshot"));
    }
}
