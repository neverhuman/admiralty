#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictError, ConflictRisk, FileChange, analyze_conflicts},
};
use admiralty_server::{ServerConfig, app};
use admiralty_store::{Store, artifacts::ArtifactStore};
use admiralty_testkit::{
    faults::{FaultEffect, FaultPlan, FaultRule},
    task_fixture,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use serde_json::Value;
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const TOKEN: &str = "round-twenty-two-canary-release-token-32-plus";

fn candidate(id: &str, path: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round22-project").expect("valid project id"),
        root_id: RootId::new("round22-root").expect("valid root id"),
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
    builder.prefix("admiralty-round22-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private canary fixture")
}

fn task_event(id: &str) -> DomainEvent {
    DomainEvent::TaskSubmitted {
        contract: task_fixture(id),
    }
}

#[tokio::test]
async fn round22_p11_p12_p18_canary_smoke_and_release_package_qualification() {
    let candidates = [
        candidate("round22-p11", "crates/admiralty-runtime/src/lib.rs"),
        candidate("round22-p12", "crates/admiralty-server/src/lib.rs"),
        candidate("round22-p18", "crates/admiralty-core/src/conflicts.rs"),
    ];
    let analyzed = analyze_conflicts(&candidates, "round22-canary-policy")
        .expect("analyze canary candidate generation");
    let replay = analyze_conflicts(
        &[
            candidates[1].clone(),
            candidates[2].clone(),
            candidates[0].clone(),
        ],
        "round22-canary-policy",
    )
    .expect("replay canary candidate generation");
    assert_eq!(analyzed, replay, "release arbitration is deterministic");
    assert_eq!(
        analyzed.generation.candidate_ids,
        ["round22-p11", "round22-p12", "round22-p18"]
    );
    assert!(analyzed.generation.combined_verification_required);
    assert!(
        analyzed
            .edges
            .iter()
            .all(|edge| { edge.risks.contains(&ConflictRisk::UnknownWriteScope) })
    );
    let mut malformed = candidate("round22-malformed", "../outside");
    assert!(matches!(
        analyze_conflicts(std::slice::from_ref(&malformed), "round22-canary-policy"),
        Err(ConflictError::InvalidPath(_))
    ));
    malformed = candidate("round22-malformed-digest", "crates/valid.rs");
    malformed.base_sha = "sha256:broken".to_owned();
    assert!(matches!(
        analyze_conflicts(&[malformed], "round22-canary-policy"),
        Err(ConflictError::InvalidCandidate(_))
    ));

    let directory = private_directory();
    let artifact_root = directory.path().join("package");
    std::fs::create_dir(&artifact_root).expect("create private package artifact root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700))
            .expect("restrict package artifact root");
    }
    let artifacts = ArtifactStore::open(&artifact_root, 64 * 1024)
        .expect("open release package artifact store");
    let package_receipt = serde_json::json!({
        "schema_version": 1,
        "packet_ids": ["P11", "P12", "P18"],
        "candidate_ids": analyzed.generation.candidate_ids,
        "verification_required": analyzed.generation.combined_verification_required,
    });
    let package_bytes = serde_json::to_vec(&package_receipt).expect("encode package receipt");
    let reference = artifacts
        .publish_bytes(&package_bytes)
        .expect("publish immutable canary package receipt");
    assert_eq!(
        artifacts
            .publish_bytes(&package_bytes)
            .expect("replay package publication"),
        reference,
        "package publication is idempotent"
    );
    let verified = artifacts
        .read_verified(&reference)
        .expect("verify package receipt digest");
    assert_eq!(
        serde_json::from_slice::<Value>(&verified).expect("package receipt schema"),
        package_receipt
    );
    std::fs::write(
        artifact_root.join(reference.sha256.trim_start_matches("sha256:")),
        b"corrupted package receipt",
    )
    .expect("inject package receipt corruption");
    assert!(artifacts.read_verified(&reference).is_err());

    let store = Store::open(directory.path().join("canary.sqlite")).expect("open canary store");
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "canary-cutover".into(),
        occurrence: 1,
        effect: FaultEffect::Fail,
    }])
    .expect("valid canary fault plan");
    assert_eq!(
        faults.hit("canary-cutover").expect("read injected fault"),
        Some(FaultEffect::Fail)
    );
    assert_eq!(
        store
            .snapshot()
            .expect("post-fault snapshot")
            .cursor
            .event_seq,
        "0"
    );
    assert_eq!(
        faults
            .hit("canary-cutover")
            .expect("read retry fault state"),
        None
    );
    let source = SourceIdentity {
        producer_id: "round22-canary".into(),
        incarnation: "boot-1".into(),
        sequence: "1".into(),
    };
    let first = store
        .append(
            source.clone(),
            task_event("round22-before-boundary"),
            "2026-10-18T19:25:00Z".into(),
        )
        .expect("append canary receipt event");
    let replayed = store
        .append(
            source,
            task_event("round22-before-boundary"),
            "2026-10-18T19:25:01Z".into(),
        )
        .expect("replay canary receipt event");
    assert!(replayed.duplicate);
    assert_eq!(replayed.event_seq, first.event_seq);
    let _rebuilt = store.rebuild().expect("rebuild canary event receipt");
    let boundary = store.snapshot().expect("post-rebuild canary boundary");
    store
        .append(
            SourceIdentity {
                producer_id: "round22-canary".into(),
                incarnation: "boot-1".into(),
                sequence: "2".into(),
            },
            task_event("round22-after-boundary"),
            "2026-10-18T19:25:02Z".into(),
        )
        .expect("append post-boundary canary event");

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
    .expect("private canary API");
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1")
                .body(Body::empty())
                .expect("unauthenticated canary stream request"),
        )
        .await
        .expect("unauthorized canary stream response");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied_body = to_bytes(denied.into_body(), 4096)
        .await
        .expect("bounded canary denial body");
    assert!(!String::from_utf8_lossy(&denied_body).contains("round22-after-boundary"));

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=1&generation=stale-round22")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("stale canary cursor request"),
        )
        .await
        .expect("stale canary cursor response");
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
                .expect("authenticated canary replay request"),
        )
        .await
        .expect("authenticated canary replay response");
    assert_eq!(response.status(), StatusCode::OK);
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        response.into_body().into_data_stream().next(),
    )
    .await
    .expect("bounded canary stream frame")
    .expect("stream yields post-boundary event")
    .expect("valid canary event frame");
    let frame = String::from_utf8(frame.to_vec()).expect("SSE frame is UTF-8");
    assert!(frame.contains("round22-after-boundary"));
    assert!(!frame.contains("round22-before-boundary"));
    assert!(frame.contains("\"seq\":\"2\""));
}
