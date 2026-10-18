use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, FileChange, analyze_conflicts},
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

const TOKEN: &str = "round-seventeen-release-audit-stream-token-32-plus";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round17-project").expect("valid project id"),
        root_id: RootId::new("round17-root").expect("valid root id"),
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
    builder.prefix("admiralty-round17-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private release-audit fixture")
}

fn task_event(task_id: &str) -> DomainEvent {
    DomainEvent::TaskSubmitted {
        contract: task_fixture(task_id),
    }
}

#[tokio::test]
async fn round17_p11_p12_p18_release_audit_preserves_cross_packet_invariants() {
    let candidates = [
        candidate("round17-p11", "crates/admiralty-runtime/src/lib.rs"),
        candidate("round17-p12", "crates/admiralty-server/src/lib.rs"),
        candidate("round17-p18", "crates/admiralty-core/src/conflicts.rs"),
        candidate("round17-control", "crates/admiralty-store/src/lib.rs"),
    ];
    let baseline = analyze_conflicts(&candidates, "round17-release-policy")
        .expect("analyze cross-packet release candidates");
    let mut permutation = candidates.clone();
    permutation.reverse();
    let replay = analyze_conflicts(&permutation, "round17-release-policy")
        .expect("replay cross-packet release candidates");
    assert_eq!(
        baseline, replay,
        "permutation preserves arbitration receipt"
    );
    assert_eq!(
        baseline.generation.candidate_ids,
        [
            "round17-control",
            "round17-p11",
            "round17-p12",
            "round17-p18"
        ]
    );
    assert!(baseline.generation.combined_verification_required);

    let directory = private_directory();
    let store = Store::open(directory.path().join("round17.sqlite")).expect("open audit store");
    let source = SourceIdentity {
        producer_id: "round17-release-audit".into(),
        incarnation: "boot-1".into(),
        sequence: "1".into(),
    };
    let first = store
        .append(
            source.clone(),
            task_event("round17-before-boundary"),
            "2026-10-18T18:25:00Z".into(),
        )
        .expect("append opening audit receipt");
    let receipt_boundary_seq = first.event_seq.clone();
    let duplicate = store
        .append(
            source,
            task_event("round17-before-boundary"),
            "2026-10-18T18:25:01Z".into(),
        )
        .expect("replay opening audit receipt");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.event_seq, first.event_seq);
    let next = store
        .append(
            SourceIdentity {
                producer_id: "round17-release-audit".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            task_event("round17-after-boundary"),
            "2026-10-18T18:25:02Z".into(),
        )
        .expect("append event after receipt boundary");
    assert_eq!(next.event_seq, "2");
    let current = store.snapshot().expect("current audit snapshot");
    let rebuilt = store.rebuild().expect("rebuild durable audit projection");
    assert_eq!(current.cursor.event_seq, rebuilt.cursor.event_seq);
    assert_eq!(current.projection, rebuilt.projection);
    let active_generation = store
        .snapshot()
        .expect("capture generation after replay rebuild")
        .cursor
        .recovery_generation;

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
    .expect("private release audit API");

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1")
                .body(Body::empty())
                .expect("unauthenticated release stream request"),
        )
        .await
        .expect("unauthorized release stream response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let denied_body = to_bytes(unauthorized.into_body(), 4096)
        .await
        .expect("bounded authorization failure");
    assert!(!String::from_utf8_lossy(&denied_body).contains("round17-after-boundary"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=round17-stale")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale release cursor request"),
        )
        .await
        .expect("stale release cursor response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let uri = format!(
        "/v1/events?after={}&generation={}",
        receipt_boundary_seq, active_generation
    );
    let stream = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authenticated audit-boundary replay request"),
        )
        .await
        .expect("authorized audit stream");
    assert_eq!(stream.status(), StatusCode::OK);
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.into_body().into_data_stream().next(),
    )
    .await
    .expect("bounded cross-packet replay response")
    .expect("event stream remains open")
    .expect("event frame is valid");
    let frame = String::from_utf8(frame.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round17-after-boundary"));
    assert!(!frame.contains("round17-before-boundary"));
    assert!(frame.contains("\"seq\":\"2\""));
}
