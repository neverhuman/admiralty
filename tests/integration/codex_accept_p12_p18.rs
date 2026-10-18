use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, analyze_conflicts},
};
use admiralty_runtime::providers::claude::{TurnOutcome, parse_stream};
use admiralty_server::{ServerConfig, app};
use admiralty_store::{Store, artifacts::ArtifactStore};
use admiralty_testkit::{clock::ClockError, clock::FakeClock, task_fixture};
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

#[test]
fn p12_p18_round9_item3_doctor_repair_idempotence_qualification() {
    let directory = private_directory();
    let artifact_root = directory.path().join("artifacts");
    std::fs::create_dir(&artifact_root).expect("create isolated artifact root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact_root, std::fs::Permissions::from_mode(0o700))
            .expect("restrict artifact root to current user");
    }
    let artifacts = ArtifactStore::open(&artifact_root, 64).expect("open immutable artifacts");
    let evidence = b"round-nine repair evidence";
    let reference = artifacts
        .publish_bytes(evidence)
        .expect("publish repair evidence");
    assert_eq!(
        artifacts
            .publish_bytes(evidence)
            .expect("replaying publication is idempotent"),
        reference
    );
    assert_eq!(
        artifacts
            .read_verified(&reference)
            .expect("verified artifact bytes"),
        evidence
    );
    std::fs::write(
        artifact_root.join(reference.sha256.trim_start_matches("sha256:")),
        b"tampered",
    )
    .expect("inject deterministic cross-boundary corruption");
    assert!(artifacts.read_verified(&reference).is_err());
    assert!(artifacts.publish_bytes(evidence).is_err());

    let database = directory.path().join("repair.sqlite");
    let store = Store::open(&database).expect("open durable repair fixture");
    let source = SourceIdentity {
        producer_id: "round9-doctor".into(),
        incarnation: "boot-1".into(),
        sequence: "1".into(),
    };
    let event = DomainEvent::TaskSubmitted {
        contract: task_fixture("round9-repair-task"),
    };
    let first = store
        .append(source.clone(), event.clone(), "2026-10-18T00:00:00Z".into())
        .expect("record repair observation");
    let replay = store
        .append(source.clone(), event, "2026-10-18T00:00:01Z".into())
        .expect("replay same observation");
    assert!(replay.duplicate);
    assert_eq!(first.event_seq, replay.event_seq);
    assert!(
        store
            .append(
                source,
                DomainEvent::TaskStarted {
                    task_id: TaskId::new("round9-repair-task").expect("valid task id"),
                    attempt_id: admiralty_core::AttemptId::new("changed-payload")
                        .expect("valid attempt id"),
                },
                "2026-10-18T00:00:02Z".into()
            )
            .is_err(),
        "same source key with changed payload fails closed"
    );
    let backup = directory.path().join("repair-backup.sqlite");
    let snapshot = store
        .backup(&backup)
        .expect("create consistent recovery backup");
    drop(store);
    let restored = Store::open(&backup).expect("reopen backup without repair side effects");
    assert_eq!(restored.snapshot().expect("restored snapshot"), snapshot);
    assert_eq!(
        restored
            .events_after(0, 10)
            .expect("replay durable events")
            .len(),
        1
    );

    let mut clock = FakeClock::new(u64::MAX - 1);
    let before = clock.clone();
    assert_eq!(
        clock.advance_ms(2),
        Err(ClockError::Overflow {
            now_ms: u64::MAX - 1,
            delta_ms: 2,
        })
    );
    assert_eq!(
        clock, before,
        "failed boundary repair leaves logical time unchanged"
    );

    let candidates = [candidate("repair-a"), candidate("repair-b")];
    let first_graph =
        analyze_conflicts(&candidates, "round9-policy").expect("analyze frozen generation");
    let replay_graph =
        analyze_conflicts(&candidates, "round9-policy").expect("replay same generation");
    assert_eq!(
        first_graph, replay_graph,
        "replayed conflict generation is byte-for-byte stable"
    );
    assert!(first_graph.generation.combined_verification_required);

    let resume = include_bytes!("../fixtures/providers/claude/turn-boundary-resume.jsonl");
    let provider_replay = parse_stream(resume, Some("fixture-session"))
        .expect("retained provider boundary fixture remains parseable");
    assert_eq!(provider_replay.outcome, TurnOutcome::Unknown);
    assert!(!provider_replay.trusted_acceptance());
    assert_eq!(
        parse_stream(resume, Some("different-session")),
        Err(admiralty_runtime::providers::claude::ClaudeError::ResumeMismatch),
        "replayed evidence bound to a different session cannot be accepted"
    );

    assert!(
        admiralty_core::placement::decode_input(br#"{"unexpected":"field"}"#).is_err(),
        "corrupted placement input with an unknown field must not be repaired into a proposal"
    );
}
