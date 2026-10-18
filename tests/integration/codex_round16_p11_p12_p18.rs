use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, FileChange, analyze_conflicts},
};
use admiralty_server::{ServerConfig, app};
use admiralty_store::Store;
use admiralty_testkit::{
    faults::{FaultEffect, FaultPlan, FaultRule},
    task_fixture,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "round-sixteen-cutover-stream-token-at-least-32-chars";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round16-project").expect("valid project id"),
        root_id: RootId::new("round16-root").expect("valid root id"),
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
    builder.prefix("admiralty-round16-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private cutover fixture")
}

fn append_task(store: &Store, sequence: &str, task_id: &str) -> admiralty_store::AppendReceipt {
    store
        .append(
            SourceIdentity {
                producer_id: "round16-cutover".into(),
                incarnation: "boot-1".into(),
                sequence: sequence.to_owned(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture(task_id),
            },
            "2026-10-18T18:13:00Z".into(),
        )
        .expect("durable cutover event")
}

#[tokio::test]
async fn round16_p11_p12_p18_cutover_contains_faults_and_preserves_receipt_boundaries() {
    let conflicted = [
        candidate("round16-candidate-a", "crates/shared.rs"),
        candidate("round16-candidate-b", "crates/shared.rs"),
        candidate("round16-candidate-c", "crates/independent.rs"),
    ];
    let generation = analyze_conflicts(&conflicted, "round16-cutover-policy")
        .expect("analyze cutover candidate generation");
    let replay = analyze_conflicts(
        &[
            conflicted[2].clone(),
            conflicted[1].clone(),
            conflicted[0].clone(),
        ],
        "round16-cutover-policy",
    )
    .expect("replay cutover candidate generation");
    assert_eq!(generation, replay, "conflict arbitration is replay-stable");
    assert_eq!(
        generation.generation.candidate_ids,
        [
            "round16-candidate-a",
            "round16-candidate-b",
            "round16-candidate-c"
        ]
    );
    assert!(generation.generation.combined_verification_required);
    assert_eq!(generation.edges.len(), 3);
    assert!(
        generation
            .edges
            .iter()
            .all(|edge| { edge.risks.contains(&ConflictRisk::UnknownWriteScope) })
    );

    let mut corrupted = candidate("round16-corrupted", "crates/shared.rs");
    corrupted.base_sha = "sha256:invalid".to_owned();
    assert!(matches!(
        analyze_conflicts(&[corrupted], "round16-cutover-policy"),
        Err(admiralty_core::conflicts::ConflictError::InvalidCandidate(
            _
        ))
    ));

    let directory = private_directory();
    let store = Store::open(directory.path().join("round16.sqlite")).expect("open event store");
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "cutover-append".into(),
        occurrence: 1,
        effect: FaultEffect::Fail,
    }])
    .expect("valid deterministic fault schedule");
    assert_eq!(
        faults.hit("cutover-append").expect("read injected fault"),
        Some(FaultEffect::Fail)
    );
    assert_eq!(
        store
            .snapshot()
            .expect("pre-retry snapshot")
            .cursor
            .event_seq,
        "0"
    );
    assert_eq!(
        faults
            .hit("cutover-append")
            .expect("read retry fault state"),
        None,
        "fault applies only at its configured occurrence"
    );
    let committed = append_task(&store, "1", "round16-before-cutover");
    assert_eq!(committed.event_seq, "1");
    assert!(!committed.duplicate);
    let boundary = store.snapshot().expect("cutover receipt boundary");
    let duplicate = append_task(&store, "1", "round16-before-cutover");
    assert!(
        duplicate.duplicate,
        "replayed cutover receipt is idempotent"
    );
    assert_eq!(duplicate.event_seq, committed.event_seq);
    assert_eq!(
        store.snapshot().expect("after replay").cursor.event_seq,
        "1"
    );
    append_task(&store, "2", "round16-after-cutover");

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
    .expect("private cutover API");

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1")
                .body(Body::empty())
                .expect("unauthenticated stream request"),
        )
        .await
        .expect("unauthorized stream response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let denied = to_bytes(unauthorized.into_body(), 4096)
        .await
        .expect("bounded denied response");
    assert!(!String::from_utf8_lossy(&denied).contains("round16-after-cutover"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=stale-round16")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale receipt boundary request"),
        )
        .await
        .expect("stale receipt boundary response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let uri = format!(
        "/v1/events?after={}&generation={}",
        boundary.cursor.event_seq, boundary.cursor.recovery_generation
    );
    let stream = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authenticated post-cutover replay request"),
        )
        .await
        .expect("authorized event stream");
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.into_body().into_data_stream().next(),
    )
    .await
    .expect("post-cutover event arrives within the fixture bound")
    .expect("stream yields the post-cutover event")
    .expect("valid SSE frame");
    let frame = String::from_utf8(frame.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round16-after-cutover"));
    assert!(!frame.contains("round16-before-cutover"));
    assert!(frame.contains("\"seq\":\"2\""));
}
