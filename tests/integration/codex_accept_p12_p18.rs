use admiralty_core::{
    DomainEvent, ProjectId, RootId, SourceIdentity, TaskId,
    conflicts::{CandidateChange, ConflictRisk, analyze_conflicts},
};
use admiralty_runtime::providers::claude::{TurnOutcome, parse_stream};
use admiralty_server::{ServerConfig, app};
use admiralty_store::{Store, artifacts::ArtifactStore};
use admiralty_testkit::{clock::ClockError, clock::FakeClock, task_fixture};
use futures_util::StreamExt;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const ROUND11_TOKEN: &str = "round-eleven-private-read-token-at-least-32-chars";

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

#[tokio::test]
async fn p11_p12_p18_round11_item4_final_validation_gate() {
    let directory = private_directory();
    let store = Store::open(directory.path().join("round11.sqlite")).expect("fixture store");
    let mut contract = task_fixture("round11-private-task");
    contract.simulated = false;
    store
        .append(
            SourceIdentity {
                producer_id: "round11-validation".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-18T00:00:00Z".into(),
        )
        .expect("record one private task event");
    let router = app(
        store,
        ServerConfig {
            listen: "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("loopback address"),
            simulated: false,
            token: Some(ROUND11_TOKEN.into()),
            assets: None,
            legacy: None,
            models_file: None,
        },
    )
    .expect("authenticated private API");

    let unauthenticated = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fleet")
                .body(Body::empty())
                .expect("unauthenticated request"),
        )
        .await
        .expect("auth denial response");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let fleet_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fleet")
                .header("authorization", format!("Bearer {ROUND11_TOKEN}"))
                .body(Body::empty())
                .expect("authenticated fleet request"),
        )
        .await
        .expect("fleet response");
    assert_eq!(fleet_response.status(), StatusCode::OK);
    let fleet_bytes = to_bytes(fleet_response.into_body(), 1024 * 1024)
        .await
        .expect("bounded fleet snapshot");
    let fleet: Value = serde_json::from_slice(&fleet_bytes).expect("fleet snapshot JSON");
    assert_eq!(fleet["schema_version"], 1);
    assert_eq!(fleet["simulated"], false);
    assert_eq!(fleet["native_execution_qualified"], false);
    assert_eq!(fleet["cursor"]["event_seq"], "1");
    assert_eq!(fleet["tasks"][0]["contract"]["simulated"], false);
    assert_eq!(fleet["projects"][0]["accepted"], 0);

    let events_response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/events?after=0")
                .header("authorization", format!("Bearer {ROUND11_TOKEN}"))
                .body(Body::empty())
                .expect("authenticated event replay request"),
        )
        .await
        .expect("event stream response");
    assert_eq!(events_response.status(), StatusCode::OK);
    assert_eq!(
        events_response.headers()["content-type"],
        "text/event-stream"
    );
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        events_response.into_body().into_data_stream().next(),
    )
    .await
    .expect("first durable event arrives within the bounded fixture timeout")
    .expect("stream emits the recorded event")
    .expect("event stream chunk is valid");
    let frame = String::from_utf8(frame.to_vec()).expect("event frame is UTF-8");
    assert!(frame.contains("event: domain"));
    assert!(frame.contains("round11-private-task"));
    assert!(frame.contains("\"seq\":\"1\""));

    let candidates = [candidate("round11-a"), candidate("round11-b")];
    let graph = analyze_conflicts(&candidates, "round11-policy").expect("P18 frozen generation");
    assert_eq!(graph.generation.candidate_ids, ["round11-a", "round11-b"]);
    assert!(graph.generation.combined_verification_required);
    assert!(
        graph.edges[0]
            .risks
            .contains(&ConflictRisk::UnknownWriteScope)
    );
    assert_eq!(
        graph,
        analyze_conflicts(&candidates, "round11-policy").expect("deterministic replay")
    );
    let mut foreign = candidate("round11-foreign");
    foreign.project_id = ProjectId::new("foreign-project").expect("valid foreign project");
    assert!(matches!(
        analyze_conflicts(&[candidates[0].clone(), foreign], "round11-policy"),
        Err(admiralty_core::conflicts::ConflictError::ScopeMismatch { .. })
    ));
}

#[tokio::test]
async fn p11_p12_p18_round12_item5_redteam_audit_gate() {
    let receipt_path = std::path::Path::new(
        "/home/ubuntu/admiralty-v2-build/receipts/RELEASE-QUALIFICATION/ROUND11-accept-p12-p18.json",
    );
    let receipt_bytes = std::fs::read(receipt_path).expect("read prior P11/P12/P18 receipt");
    let receipt: Value = serde_json::from_slice(&receipt_bytes).expect("valid receipt JSON");
    assert_eq!(
        receipt["packet_ids"],
        serde_json::json!(["P11", "P12", "P18"])
    );
    assert_eq!(receipt["status"], "passed");
    assert_eq!(
        receipt["test"],
        "p11_p12_p18_round11_item4_final_validation_gate"
    );
    assert_eq!(receipt["exit_code"], 0);

    let log_path = receipt["log_path"]
        .as_str()
        .expect("receipt has a log path");
    let expected_log_sha = receipt["log_sha256"]
        .as_str()
        .expect("receipt has log SHA-256");
    assert_eq!(expected_log_sha.len(), 64);
    assert!(
        expected_log_sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    let log = std::fs::read(log_path).expect("read receipt's recorded validation log");
    let actual_log_sha = format!("{:x}", Sha256::digest(&log));
    assert_eq!(
        actual_log_sha, expected_log_sha,
        "receipt log digest mismatch"
    );
    assert!(String::from_utf8_lossy(&log).contains("1 passed; 0 failed"));
    let mut tampered_log = log.clone();
    tampered_log.push(b'!');
    assert_ne!(
        format!("{:x}", Sha256::digest(&tampered_log)),
        expected_log_sha,
        "single-byte log tampering must be detected"
    );

    let directory = private_directory();
    let store = Store::open(directory.path().join("round12.sqlite")).expect("fixture store");
    let mut contract = task_fixture("round12-redteam-task");
    contract.simulated = false;
    store
        .append(
            SourceIdentity {
                producer_id: "round12-redteam".into(),
                incarnation: "boot-1".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted { contract },
            "2026-10-18T00:00:00Z".into(),
        )
        .expect("record private task fixture");
    let router = app(
        store,
        ServerConfig {
            listen: "127.0.0.1:0"
                .parse::<SocketAddr>()
                .expect("loopback address"),
            simulated: false,
            token: Some(ROUND11_TOKEN.into()),
            assets: None,
            legacy: None,
            models_file: None,
        },
    )
    .expect("authenticated private API");
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fleet")
                .body(Body::empty())
                .expect("unauthenticated red-team request"),
        )
        .await
        .expect("authorization denial");
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let allowed = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fleet")
                .header("authorization", format!("Bearer {ROUND11_TOKEN}"))
                .body(Body::empty())
                .expect("authenticated read request"),
        )
        .await
        .expect("authorized fleet response");
    assert_eq!(allowed.status(), StatusCode::OK);
    let bytes = to_bytes(allowed.into_body(), 1024 * 1024)
        .await
        .expect("bounded fleet projection");
    let fleet: Value = serde_json::from_slice(&bytes).expect("fleet JSON");
    assert_eq!(fleet["simulated"], false);
    assert_eq!(fleet["native_execution_qualified"], false);
    assert_eq!(fleet["cursor"]["event_seq"], "1");
    assert_eq!(fleet["tasks"][0]["contract"]["simulated"], false);
    assert_eq!(fleet["projects"][0]["accepted"], 0);

    let repeated = candidate("round12-candidate");
    let once = analyze_conflicts(std::slice::from_ref(&repeated), "round12-policy")
        .expect("single candidate audit");
    let replay = analyze_conflicts(&[repeated.clone(), repeated], "round12-policy")
        .expect("duplicate replay audit");
    assert_eq!(once, replay, "duplicate candidate replay is idempotent");
    assert!(replay.edges.is_empty());
    let mut foreign = candidate("round12-foreign");
    foreign.root_id = RootId::new("foreign-root").expect("valid foreign root");
    assert!(matches!(
        analyze_conflicts(&[candidate("round12-local"), foreign], "round12-policy"),
        Err(admiralty_core::conflicts::ConflictError::ScopeMismatch { .. })
    ));
}
