#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::{agy, claude, supervision},
    workspace::registry::{Custody, GitMetadata, NodeRef, WorkspaceSpec},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const CLAUDE_TRANSCRIPT: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/claude/continuity-turn.jsonl");
const AGY_CAPABILITIES: &str =
    include_str!("../../../../tests/fixtures/providers/agy/1.4.2/capabilities.json");

fn fixture<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    match value {
        Ok(value) => value,
        Err(error) => panic!("checked-in hermetic fixture is valid: {error:?}"),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseRecord {
    schema_version: u32,
    packet: String,
    provider_policy: String,
    supervision_policy: String,
    workspace_roots: Vec<String>,
    native_execution_qualified: bool,
}

fn provider_help() -> String {
    let value: Value = fixture(serde_json::from_str(AGY_CAPABILITIES));
    let Some(evidence) = value.get("help_evidence").and_then(Value::as_array) else {
        panic!("pinned AGY fixture contains help evidence");
    };
    let lines: Vec<&str> = evidence.iter().filter_map(Value::as_str).collect();
    assert_eq!(lines.len(), evidence.len());
    lines.join(" ")
}

#[test]
fn p08_p21_p22_round21_post_item6_hermetic_ci_cross_packet_release_qualification() {
    // P08: typed supervision evidence survives a JSON round trip and replays identically.
    let tick = |value: &str| fixture(supervision::Tick::new(value));
    let scope = supervision::SupervisionScope {
        project_id: fixture(ProjectId::new("round21-project")),
        root_id: fixture(RootId::new("round21-root")),
        task_id: fixture(TaskId::new("round21-task")),
        attempt_id: fixture(AttemptId::new("round21-attempt")),
        owner: fixture(PrincipalId::new("round21-owner")),
        incarnation: "ci-run-1".into(),
        authority_ref: "authority:round21".into(),
        contract_digest: format!("sha256:{}", "f".repeat(64)),
        revision: 1,
    };
    let observation = supervision::SupervisionObservation {
        scope,
        clocks: supervision::SignalClocks {
            last_byte: Some(tick("200")),
            provider_event: Some(tick("200")),
            tool_heartbeat: Some(tick("200")),
            artifact_change: Some(tick("200")),
            distinct_command: Some(tick("200")),
            completed_check: Some(tick("200")),
            verified_milestone: Some(tick("200")),
        },
        process_state: supervision::ProcessState::Alive,
        tool_state: supervision::ToolState::Running,
        provider_wait: supervision::ProviderWait::None,
        approval_observation: None,
    };
    let replayed: supervision::SupervisionObservation = fixture(serde_json::from_slice(&fixture(
        serde_json::to_vec(&observation),
    )));
    assert_eq!(replayed, observation);
    let policy = supervision::DetectionPolicy {
        version: "round21-policy-v1".into(),
        stall_after: tick("15"),
        hysteresis_samples: 2,
        cooldown: tick("5"),
    };
    let now = tick("202");
    let proposal = fixture(supervision::evaluate_supervision(
        &policy,
        &observation,
        None,
        Some(&now),
    ));
    assert_eq!(
        proposal,
        fixture(supervision::evaluate_supervision(
            &policy,
            &replayed,
            None,
            Some(&now),
        ))
    );
    assert!(!proposal.may_signal());
    assert!(!proposal.may_send_input());
    assert!(!proposal.execution_granted());

    // P21: checked provider evidence is reproducible and unsupported drift is rejected.
    let help = provider_help();
    let agy_a = fixture(agy::qualify("1.4.2", &help));
    let agy_b = fixture(agy::qualify("1.4.2", &help));
    assert_eq!(agy_a, agy_b);
    assert_eq!(agy_a.stream_json, agy::Capability::Advertised);
    assert!(!agy_a.native_execution_qualified());
    assert!(agy::qualify("1.4.3", &help).is_err());
    let claude_a = fixture(claude::parse_stream(
        CLAUDE_TRANSCRIPT,
        Some("fixture-session"),
    ));
    let claude_b = fixture(claude::parse_stream(
        CLAUDE_TRANSCRIPT,
        Some("fixture-session"),
    ));
    assert_eq!(claude_a, claude_b);
    assert!(!claude_a.trusted_acceptance());
    assert!(!claude_a.native_execution_qualified());

    // P22: generated worktree roots validate repeatedly, without sibling or private ownership.
    let temporary = fixture(tempfile::tempdir());
    let root_a = temporary.path().join("lanes/a");
    let root_b = temporary.path().join("lanes/b");
    for (lane, root) in [("a", &root_a), ("b", &root_b)] {
        fixture(fs::create_dir_all(root.join("src")));
        fixture(fs::create_dir_all(
            temporary.path().join(format!(".git/worktrees/{lane}")),
        ));
        fixture(fs::write(root.join("src/lib.rs"), lane.as_bytes()));
        fixture(fs::write(
            root.join(".git"),
            format!("gitdir: ../../.git/worktrees/{lane}\n"),
        ));
    }
    let make_spec = |lane: &str, root_name: &str, root: &std::path::Path| WorkspaceSpec {
        node_id: fixture(NodeRef::new(lane)),
        root_id: fixture(RootId::new(root_name)),
        project_id: fixture(ProjectId::new("round21-project")),
        workspace: fixture(fs::canonicalize(root)),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let spec_a = make_spec("lane-a", "root-a", &root_a);
    let spec_b = make_spec("lane-b", "root-b", &root_b);
    let validated_a = fixture(spec_a.validate());
    let validated_b = fixture(spec_b.validate());
    assert_eq!(validated_a.custody, Custody::RegisteredOnly);
    assert_eq!(validated_a.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(validated_b.git_metadata, GitMetadata::WorktreeFile);
    for _ in 0..20 {
        assert_eq!(fixture(spec_a.validate()), validated_a);
        assert_eq!(fixture(spec_b.validate()), validated_b);
        assert!(validated_a.owns(std::path::Path::new("src/lib.rs")));
        assert!(validated_b.owns(std::path::Path::new("src/lib.rs")));
        assert!(!validated_a.owns(std::path::Path::new("../b/src/lib.rs")));
        assert!(!validated_b.owns(std::path::Path::new("../a/src/lib.rs")));
        assert!(!validated_a.owns(std::path::Path::new("src/private/key")));
    }

    // Cross-packet release evidence remains schema-closed and hashes identically on replay.
    let record = ReleaseRecord {
        schema_version: 1,
        packet: "P08-P21-P22".into(),
        provider_policy: "agy@1.4.2;claude@fixture-session".into(),
        supervision_policy: policy.version,
        workspace_roots: vec!["root-a:src".into(), "root-b:src".into()],
        native_execution_qualified: false,
    };
    let bytes = fixture(serde_json::to_vec(&record));
    let decoded: ReleaseRecord = fixture(serde_json::from_slice(&bytes));
    assert_eq!(decoded, record);
    let digest = Sha256::digest(&bytes);
    assert_eq!(
        digest,
        Sha256::digest(fixture(serde_json::to_vec(&decoded)))
    );
    assert!(serde_json::from_slice::<ReleaseRecord>(
        br#"{"schema_version":1,"packet":"P08-P21-P22","provider_policy":"agy@1.4.2;claude@fixture-session","supervision_policy":"round21-policy-v1","workspace_roots":["root-a:src","root-b:src"],"native_execution_qualified":false,"accepted":true}"#
    )
    .is_err());
}
