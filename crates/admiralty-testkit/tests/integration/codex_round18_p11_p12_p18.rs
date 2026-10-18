#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, FileChange, analyze_conflicts},
};
use admiralty_server::{ServerConfig, app};
use admiralty_store::{Store, artifacts::ArtifactStore};
use admiralty_testkit::task_fixture;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use serde_json::Value;
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "round-eighteen-release-bundle-token-32-chars-min";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round18-project").expect("valid project id"),
        root_id: RootId::new("round18-root").expect("valid root id"),
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
    builder.prefix("admiralty-round18-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private release-bundle fixture")
}

fn task_event(task_id: &str) -> DomainEvent {
    DomainEvent::TaskSubmitted {
        contract: task_fixture(task_id),
    }
}

#[tokio::test]
async fn round18_p11_p12_p18_release_bundle_schema_and_stream_parity() {
    let candidates = [
        candidate("round18-p11", "crates/admiralty-runtime/src/lib.rs"),
        candidate("round18-p12", "crates/admiralty-server/src/lib.rs"),
        candidate("round18-p18", "crates/admiralty-core/src/conflicts.rs"),
    ];
    let arbitration = analyze_conflicts(&candidates, "round18-release-policy")
        .expect("analyze release bundle candidates");
    let replay = analyze_conflicts(
        &[
            candidates[2].clone(),
            candidates[0].clone(),
            candidates[1].clone(),
        ],
        "round18-release-policy",
    )
    .expect("replay release bundle candidates");
    assert_eq!(
        arbitration, replay,
        "candidate permutation preserves bundle receipt"
    );
    assert_eq!(
        arbitration.generation.candidate_ids,
        ["round18-p11", "round18-p12", "round18-p18"]
    );

    let directory = private_directory();
    let artifact_root = directory.path().join("bundle");
    std::fs::create_dir(&artifact_root).expect("create isolated bundle root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700))
            .expect("restrict bundle root to current user");
    }
    let artifacts = ArtifactStore::open(&artifact_root, 64 * 1024).expect("open bundle store");
    let bundle = serde_json::json!({
        "schema_version": 1,
        "packet_ids": ["P11", "P12", "P18"],
        "conflict_candidate_ids": arbitration.generation.candidate_ids,
        "combined_verification_required": arbitration.generation.combined_verification_required,
    });
    let bundle_bytes = serde_json::to_vec(&bundle).expect("encode release bundle schema");
    let artifact = artifacts
        .publish_bytes(&bundle_bytes)
        .expect("publish immutable release bundle");
    assert_eq!(
        artifacts
            .publish_bytes(&bundle_bytes)
            .expect("replay bundle publication"),
        artifact,
        "release bundle publication is idempotent"
    );
    let verified_bytes = artifacts
        .read_verified(&artifact)
        .expect("verify release bundle digest");
    assert_eq!(verified_bytes, bundle_bytes);
    let decoded_bundle: Value = serde_json::from_slice(&verified_bytes).expect("bundle JSON");
    assert_eq!(decoded_bundle, bundle);

    let store = Store::open(directory.path().join("round18.sqlite")).expect("open audit store");
    let event = task_event("round18-before-boundary");
    let event_schema = serde_json::to_value(&event).expect("encode durable event schema");
    assert_eq!(
        serde_json::from_value::<DomainEvent>(event_schema.clone()).expect("decode durable schema"),
        event,
        "event wire schema round-trips without field loss"
    );
    let first = store
        .append(
            SourceIdentity {
                producer_id: "round18-release-bundle".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            event,
            "2026-10-18T18:37:00Z".into(),
        )
        .expect("append schema-checked event");
    let duplicate = store
        .append(
            SourceIdentity {
                producer_id: "round18-release-bundle".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            task_event("round18-before-boundary"),
            "2026-10-18T18:37:01Z".into(),
        )
        .expect("replay schema-checked event");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.event_seq, first.event_seq);
    let boundary_seq = first.event_seq;
    store
        .append(
            SourceIdentity {
                producer_id: "round18-release-bundle".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            task_event("round18-after-boundary"),
            "2026-10-18T18:37:02Z".into(),
        )
        .expect("append event after bundle boundary");
    let current = store.snapshot().expect("current bundle snapshot");
    let rebuilt = store.rebuild().expect("rebuild bundle event projection");
    assert_eq!(current.cursor.event_seq, rebuilt.cursor.event_seq);
    assert_eq!(current.projection, rebuilt.projection);
    let active_generation = store
        .snapshot()
        .expect("post-replay recovery cursor")
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
    .expect("authenticated release bundle API");
    let denied = router
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
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied_bytes = to_bytes(denied.into_body(), 4096)
        .await
        .expect("bounded unauthorized body");
    assert!(!String::from_utf8_lossy(&denied_bytes).contains("round18-after-boundary"));

    let uri = format!("/v1/events?after={boundary_seq}&generation={active_generation}");
    let response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authenticated bundle-boundary replay request"),
        )
        .await
        .expect("authenticated bundle-boundary response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        response.into_body().into_data_stream().next(),
    )
    .await
    .expect("bounded release replay response")
    .expect("stream yields the post-boundary event")
    .expect("SSE frame is valid");
    let frame = String::from_utf8(bytes.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round18-after-boundary"));
    assert!(!frame.contains("round18-before-boundary"));
    assert!(frame.contains("\"seq\":\"2\""));
}
