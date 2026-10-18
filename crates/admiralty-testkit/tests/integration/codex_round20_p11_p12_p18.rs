#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictError, ConflictRisk, FileChange, analyze_conflicts},
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

const TOKEN: &str = "round-twenty-strict-audit-private-token-32-chars";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round20-project").expect("valid project id"),
        root_id: RootId::new("round20-root").expect("valid root id"),
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
    builder.prefix("admiralty-round20-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .expect("private release qualification fixture")
}

fn task_event(task_id: &str) -> DomainEvent {
    DomainEvent::TaskSubmitted {
        contract: task_fixture(task_id),
    }
}

#[tokio::test]
async fn round20_p11_p12_p18_strict_release_qualification_and_receipt_audit() {
    let candidates = [
        candidate("round20-p11", "crates/admiralty-runtime/src/lib.rs"),
        candidate("round20-p12", "crates/admiralty-server/src/lib.rs"),
        candidate("round20-p18", "crates/admiralty-core/src/conflicts.rs"),
    ];
    let forward = analyze_conflicts(&candidates, "round20-release-policy")
        .expect("bounded P11/P12/P18 arbitration");
    let reverse = analyze_conflicts(
        &[
            candidates[2].clone(),
            candidates[1].clone(),
            candidates[0].clone(),
        ],
        "round20-release-policy",
    )
    .expect("replay P11/P12/P18 arbitration");
    assert_eq!(forward, reverse, "release arbitration is order-independent");
    assert_eq!(
        forward.generation.candidate_ids,
        ["round20-p11", "round20-p12", "round20-p18"]
    );
    assert!(forward.generation.combined_verification_required);
    assert!(
        forward
            .edges
            .iter()
            .all(|edge| { edge.risks.contains(&ConflictRisk::UnknownWriteScope) })
    );

    let mut malformed = candidate("round20-malformed", "crates/shared.rs");
    malformed.head_sha = "sha256:bad".to_owned();
    assert!(matches!(
        analyze_conflicts(&[malformed], "round20-release-policy"),
        Err(ConflictError::InvalidCandidate(_))
    ));

    let directory = private_directory();
    let artifact_root = directory.path().join("release-receipts");
    std::fs::create_dir(&artifact_root).expect("create receipt root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700))
            .expect("restrict receipt root");
    }
    let artifacts = ArtifactStore::open(&artifact_root, 64 * 1024).expect("open receipt store");
    let receipt = serde_json::json!({
        "schema_version": 1,
        "packet_ids": ["P11", "P12", "P18"],
        "candidate_ids": forward.generation.candidate_ids,
        "verification_required": forward.generation.combined_verification_required,
    });
    let receipt_bytes = serde_json::to_vec(&receipt).expect("encode qualification receipt");
    let reference = artifacts
        .publish_bytes(&receipt_bytes)
        .expect("publish immutable receipt");
    assert_eq!(
        artifacts
            .publish_bytes(&receipt_bytes)
            .expect("replay immutable receipt publication"),
        reference,
        "receipt boundary is idempotent"
    );
    let verified = artifacts
        .read_verified(&reference)
        .expect("verify published receipt digest");
    assert_eq!(
        serde_json::from_slice::<Value>(&verified).expect("receipt schema"),
        receipt
    );
    let object_path = artifact_root.join(reference.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object_path, b"corrupted receipt").expect("inject receipt corruption");
    assert!(artifacts.read_verified(&reference).is_err());

    let store = Store::open(directory.path().join("release.sqlite")).expect("open release store");
    let source = SourceIdentity {
        producer_id: "round20-release-audit".into(),
        incarnation: "boot-1".into(),
        sequence: "1".into(),
    };
    let first = store
        .append(
            source.clone(),
            task_event("round20-before-boundary"),
            "2026-10-18T18:49:00Z".into(),
        )
        .expect("append event at opening receipt boundary");
    let replay = store
        .append(
            source,
            task_event("round20-before-boundary"),
            "2026-10-18T18:49:01Z".into(),
        )
        .expect("replay opening receipt event");
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, first.event_seq);
    store
        .append(
            SourceIdentity {
                producer_id: "round20-release-audit".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            task_event("round20-after-boundary"),
            "2026-10-18T18:49:02Z".into(),
        )
        .expect("append post-boundary event");
    let current = store.snapshot().expect("current release snapshot");
    let replayed = store.rebuild().expect("rebuild release snapshot");
    assert_eq!(current.cursor.event_seq, replayed.cursor.event_seq);
    assert_eq!(current.projection, replayed.projection);
    let generation = store
        .snapshot()
        .expect("current recovery generation after replay")
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
    .expect("authenticated release audit API");
    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1")
                .body(Body::empty())
                .expect("unauthenticated receipt stream request"),
        )
        .await
        .expect("unauthorized receipt stream response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let denied = to_bytes(unauthorized.into_body(), 4096)
        .await
        .expect("bounded authorization error");
    assert!(!String::from_utf8_lossy(&denied).contains("round20-after-boundary"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=round20-stale")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale receipt cursor request"),
        )
        .await
        .expect("stale receipt cursor response");
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/v1/events?after={}&generation={generation}",
                    first.event_seq
                ))
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authorized receipt-boundary stream request"),
        )
        .await
        .expect("authorized receipt-boundary stream response");
    assert_eq!(response.status(), StatusCode::OK);
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        response.into_body().into_data_stream().next(),
    )
    .await
    .expect("bounded post-receipt event");
    let bytes = frame
        .expect("stream yields post-boundary event")
        .expect("valid SSE frame");
    let frame = String::from_utf8(bytes.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round20-after-boundary"));
    assert!(!frame.contains("round20-before-boundary"));
    assert!(frame.contains("\"seq\":\"2\""));
}
