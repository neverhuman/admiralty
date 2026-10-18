#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, FileChange, analyze_conflicts},
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

const TOKEN: &str = "round-twenty-one-hermetic-release-token-over-32";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round21-project").expect("valid project id"),
        root_id: RootId::new("round21-root").expect("valid root id"),
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
    builder.prefix("admiralty-round21-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private hermetic fixture")
}

fn event(task_id: &str) -> DomainEvent {
    DomainEvent::TaskSubmitted {
        contract: task_fixture(task_id),
    }
}

#[tokio::test]
async fn round21_p11_p12_p18_hermetic_release_audit_and_cross_packet_invariants() {
    let candidates = (0..6)
        .map(|index| candidate(&format!("round21-candidate-{index}"), "crates/shared.rs"))
        .collect::<Vec<_>>();
    let forward = analyze_conflicts(&candidates, "round21-hermetic-policy")
        .expect("analyze bounded cross-packet candidates");
    let mut reversed = candidates.clone();
    reversed.reverse();
    let replay = analyze_conflicts(&reversed, "round21-hermetic-policy")
        .expect("replay cross-packet candidates");
    assert_eq!(
        forward, replay,
        "candidate permutation leaves receipt stable"
    );
    assert_eq!(forward.generation.candidate_ids.len(), 6);
    assert!(forward.generation.combined_verification_required);
    assert_eq!(forward.edges.len(), 15);
    assert!(
        forward
            .edges
            .iter()
            .all(|edge| { edge.risks.contains(&ConflictRisk::UnknownWriteScope) })
    );

    let directory = private_directory();
    let artifacts_root = directory.path().join("artifacts");
    std::fs::create_dir(&artifacts_root).expect("create local artifact root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifacts_root, std::fs::Permissions::from_mode(0o700))
            .expect("restrict local artifact root");
    }
    let artifacts =
        ArtifactStore::open(&artifacts_root, 64 * 1024).expect("open hermetic artifacts");
    let receipt = serde_json::json!({
        "schema_version": 1,
        "packet_ids": ["P11", "P12", "P18"],
        "candidate_ids": forward.generation.candidate_ids,
        "combined_verification_required": forward.generation.combined_verification_required,
    });
    let receipt_bytes = serde_json::to_vec(&receipt).expect("encode receipt fixture");
    let reference = artifacts
        .publish_bytes(&receipt_bytes)
        .expect("publish local qualification receipt");
    assert_eq!(
        artifacts
            .publish_bytes(&receipt_bytes)
            .expect("replay local receipt publication"),
        reference
    );
    let verified = artifacts
        .read_verified(&reference)
        .expect("verify content-addressed receipt");
    assert_eq!(
        serde_json::from_slice::<Value>(&verified).expect("decode receipt schema"),
        receipt
    );
    std::fs::write(
        artifacts_root.join(reference.sha256.trim_start_matches("sha256:")),
        b"mutated receipt",
    )
    .expect("inject local receipt corruption");
    assert!(artifacts.read_verified(&reference).is_err());

    let store =
        Store::open(directory.path().join("events.sqlite")).expect("open local event store");
    let first_event = event("round21-before-boundary");
    let event_json = serde_json::to_value(&first_event).expect("encode event schema");
    assert_eq!(
        serde_json::from_value::<DomainEvent>(event_json).expect("decode event schema"),
        first_event,
        "event serialization is schema-preserving"
    );
    let first = store
        .append(
            SourceIdentity {
                producer_id: "round21-hermetic".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            first_event,
            "2026-10-18T19:05:00Z".into(),
        )
        .expect("append event at receipt boundary");
    let duplicate = store
        .append(
            SourceIdentity {
                producer_id: "round21-hermetic".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            event("round21-before-boundary"),
            "2026-10-18T19:05:01Z".into(),
        )
        .expect("replay event receipt");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.event_seq, first.event_seq);
    let _rebuilt = store
        .rebuild()
        .expect("rebuild deterministic event projection");
    let boundary = store.snapshot().expect("post-rebuild receipt boundary");
    assert_eq!(boundary.cursor.event_seq, "1");
    store
        .append(
            SourceIdentity {
                producer_id: "round21-hermetic".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            event("round21-after-boundary"),
            "2026-10-18T19:05:02Z".into(),
        )
        .expect("append event beyond receipt boundary");

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
    .expect("private hermetic event API");
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
        .expect("unauthorized stream result");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied_body = to_bytes(denied.into_body(), 4096)
        .await
        .expect("bounded denial payload");
    assert!(!String::from_utf8_lossy(&denied_body).contains("round21-after-boundary"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=round21-stale")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale generation request"),
        )
        .await
        .expect("stale generation response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let uri = format!(
        "/v1/events?after={}&generation={}",
        boundary.cursor.event_seq, boundary.cursor.recovery_generation
    );
    let response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authorized receipt-boundary replay request"),
        )
        .await
        .expect("authorized receipt-boundary response");
    assert_eq!(response.status(), StatusCode::OK);
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        response.into_body().into_data_stream().next(),
    )
    .await
    .expect("bounded release event replay")
    .expect("post-boundary event is streamed")
    .expect("valid SSE payload");
    let frame = String::from_utf8(frame.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round21-after-boundary"));
    assert!(!frame.contains("round21-before-boundary"));
    assert!(frame.contains("\"seq\":\"2\""));
}
