use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, analyze_conflicts},
};
use admiralty_server::{ServerConfig, app};
use admiralty_store::Store;
use admiralty_testkit::task_fixture;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

fn private_directory() -> TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round8-p12-p18-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private round-eight fixture")
}

fn candidate(id: &str) -> CandidateChange {
    CandidateChange {
        project_id: ProjectId::new("round8-project").expect("valid fixture project"),
        root_id: RootId::new("round8-root").expect("valid fixture root"),
        task_id: TaskId::new(id).expect("valid fixture task"),
        candidate_id: id.to_owned(),
        base_sha: format!("sha256:{}", "a".repeat(64)),
        head_sha: format!("sha256:{}", "b".repeat(64)),
        tree_sha: format!("sha256:{}", "c".repeat(64)),
        ancestry_digest: Some(format!("sha256:{}", "d".repeat(64))),
        files: Vec::new(),
        constraints: None,
    }
}

#[tokio::test]
async fn p12_p18_round8_p12_api_tui_doctor_qualification() {
    let directory = private_directory();
    let store = Store::open(directory.path().join("round8.sqlite")).expect("fixture store");
    store
        .append(
            SourceIdentity {
                producer_id: "round8-fixture".into(),
                incarnation: "round8-boot".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture("round8-p12-task"),
            },
            "2026-10-18T00:00:00Z".into(),
        )
        .expect("durable deterministic task fixture");

    let app = app(
        store,
        ServerConfig {
            listen: "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("loopback address"),
            simulated: true,
            token: None,
            assets: None,
            legacy: None,
            models_file: None,
        },
    )
    .expect("simulated read-only API");
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fleet")
                .body(Body::empty())
                .expect("deterministic fleet request"),
        )
        .await
        .expect("fleet router response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded dashboard response");
    let fleet: Value = serde_json::from_slice(&bytes).expect("fleet JSON contract");
    assert_eq!(fleet["schema_version"], 1);
    assert_eq!(fleet["simulated"], true);
    assert_eq!(fleet["native_execution_qualified"], false);
    assert_eq!(fleet["cursor"]["event_seq"], "1");
    assert_eq!(fleet["coordination"]["measured_tokens"], Value::Null);
    assert_eq!(fleet["tasks"][0]["contract"]["simulated"], true);

    let graph = analyze_conflicts(
        &[candidate("candidate-a"), candidate("candidate-b")],
        "policy-1",
    )
    .expect("conservative integration generation");
    assert!(graph.generation.combined_verification_required);
    assert_eq!(
        graph.generation.candidate_ids,
        ["candidate-a", "candidate-b"]
    );
    assert!(
        graph.edges[0]
            .risks
            .contains(&ConflictRisk::UnknownWriteScope)
    );
}
