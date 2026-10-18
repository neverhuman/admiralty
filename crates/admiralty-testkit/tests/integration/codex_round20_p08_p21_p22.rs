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

const CLAUDE_FIXTURE: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/claude/continuity-turn.jsonl");
const AGY_POLICY: &str =
    include_str!("../../../../tests/fixtures/providers/agy/1.4.2/capabilities.json");

fn checked<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    match value {
        Ok(value) => value,
        Err(error) => panic!("checked-in release evidence is valid: {error:?}"),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PacketEvidence {
    schema_version: u32,
    packet: String,
    provider: String,
    provider_version: String,
    observed_only: bool,
    workspace_root: String,
    supervision_policy: String,
}

fn evidence_text() -> String {
    let parsed: Value = checked(serde_json::from_str(AGY_POLICY));
    let Some(rows) = parsed.get("help_evidence").and_then(Value::as_array) else {
        panic!("pinned provider fixture must contain help evidence");
    };
    let text: Vec<&str> = rows.iter().filter_map(Value::as_str).collect();
    assert_eq!(text.len(), rows.len());
    text.join(" ")
}

fn scope() -> supervision::SupervisionScope {
    supervision::SupervisionScope {
        project_id: checked(ProjectId::new("round20-project")),
        root_id: checked(RootId::new("round20-root")),
        task_id: checked(TaskId::new("round20-task")),
        attempt_id: checked(AttemptId::new("round20-attempt")),
        owner: checked(PrincipalId::new("round20-owner")),
        incarnation: "release-audit-1".into(),
        authority_ref: "authority:round20".into(),
        contract_digest: format!("sha256:{}", "e".repeat(64)),
        revision: 1,
    }
}

#[test]
fn p08_p21_p22_round20_post_pr57_release_qualification_strict_clippy_audit() {
    // P08 supervision is stable across serialization and policy drift fails closed.
    let tick = |value: &str| checked(supervision::Tick::new(value));
    let policy = supervision::DetectionPolicy {
        version: "round20-policy-v1".into(),
        stall_after: tick("10"),
        hysteresis_samples: 2,
        cooldown: tick("4"),
    };
    let observation = supervision::SupervisionObservation {
        scope: scope(),
        clocks: supervision::SignalClocks {
            last_byte: Some(tick("100")),
            provider_event: Some(tick("100")),
            tool_heartbeat: Some(tick("100")),
            artifact_change: Some(tick("100")),
            distinct_command: Some(tick("100")),
            completed_check: Some(tick("100")),
            verified_milestone: Some(tick("100")),
        },
        process_state: supervision::ProcessState::Alive,
        tool_state: supervision::ToolState::Running,
        provider_wait: supervision::ProviderWait::None,
        approval_observation: None,
    };
    let serialized = checked(serde_json::to_vec(&observation));
    let replayed: supervision::SupervisionObservation =
        checked(serde_json::from_slice(&serialized));
    assert_eq!(replayed, observation);
    let now = tick("102");
    let first = checked(supervision::evaluate_supervision(
        &policy,
        &observation,
        None,
        Some(&now),
    ));
    let replay = checked(supervision::evaluate_supervision(
        &policy,
        &replayed,
        None,
        Some(&now),
    ));
    assert_eq!(first, replay);
    assert!(!first.may_signal());
    assert!(!first.may_send_input());
    assert!(!first.execution_granted());
    let changed_policy = supervision::DetectionPolicy {
        version: "round20-policy-v2".into(),
        ..policy
    };
    assert_eq!(
        supervision::evaluate_supervision(
            &changed_policy,
            &observation,
            Some(&first.state),
            Some(&tick("103")),
        ),
        Err(supervision::SupervisionError::PolicyDrift)
    );

    // P21 provider observations use the pinned policy and cannot qualify native execution.
    let capabilities = checked(agy::qualify("1.4.2", &evidence_text()));
    assert_eq!(capabilities.stream_json, agy::Capability::Advertised);
    assert!(!capabilities.native_execution_qualified());
    assert!(agy::qualify("1.4.3", &evidence_text()).is_err());
    let claude_first = checked(claude::parse_stream(
        CLAUDE_FIXTURE,
        Some("fixture-session"),
    ));
    let claude_replay = checked(claude::parse_stream(
        CLAUDE_FIXTURE,
        Some("fixture-session"),
    ));
    assert_eq!(claude_first, claude_replay);
    assert!(!claude_first.trusted_acceptance());
    assert!(!claude_first.native_execution_qualified());
    assert!(claude::parse_stream(b"{\"type\":\"message_start\"}\n", None).is_err());

    // P22 path custody stays rooted, and both release packet rows validate identically.
    let temp = checked(tempfile::tempdir());
    let root_a = temp.path().join("worktrees/a");
    let root_b = temp.path().join("worktrees/b");
    for (name, root) in [("a", &root_a), ("b", &root_b)] {
        checked(fs::create_dir_all(root.join("src")));
        checked(fs::create_dir_all(
            temp.path().join(format!(".git/worktrees/{name}")),
        ));
        checked(fs::write(root.join("src/lib.rs"), name.as_bytes()));
        checked(fs::write(
            root.join(".git"),
            format!("gitdir: ../../.git/worktrees/{name}\n"),
        ));
    }
    let make_spec = |name: &str, root_id: &str, root: &std::path::Path| WorkspaceSpec {
        node_id: checked(NodeRef::new(name)),
        root_id: checked(RootId::new(root_id)),
        project_id: checked(ProjectId::new("round20-project")),
        workspace: checked(fs::canonicalize(root)),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let spec_a = make_spec("worker-a", "root-a", &root_a);
    let spec_b = make_spec("worker-b", "root-b", &root_b);
    let valid_a = checked(spec_a.validate());
    let valid_b = checked(spec_b.validate());
    assert_eq!(valid_a.custody, Custody::RegisteredOnly);
    assert_eq!(valid_a.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(valid_b.git_metadata, GitMetadata::WorktreeFile);
    for _ in 0..16 {
        assert_eq!(checked(spec_a.validate()), valid_a);
        assert_eq!(checked(spec_b.validate()), valid_b);
        assert!(valid_a.owns(std::path::Path::new("src/lib.rs")));
        assert!(!valid_a.owns(std::path::Path::new("../b/src/lib.rs")));
        assert!(!valid_b.owns(std::path::Path::new("../a/src/lib.rs")));
        assert!(!valid_a.owns(std::path::Path::new("src/private/key")));
    }

    let packet = PacketEvidence {
        schema_version: 1,
        packet: "P08-P21-P22".into(),
        provider: "agy".into(),
        provider_version: "1.4.2".into(),
        observed_only: true,
        workspace_root: "root-a:src".into(),
        supervision_policy: "round20-policy-v1".into(),
    };
    let packet_bytes = checked(serde_json::to_vec(&packet));
    let decoded: PacketEvidence = checked(serde_json::from_slice(&packet_bytes));
    assert_eq!(decoded, packet);
    let digest = Sha256::digest(&packet_bytes);
    assert_eq!(
        digest,
        Sha256::digest(checked(serde_json::to_vec(&decoded)))
    );
    assert!(serde_json::from_slice::<PacketEvidence>(
        br#"{"schema_version":1,"packet":"P08-P21-P22","provider":"agy","provider_version":"1.4.2","observed_only":true,"workspace_root":"root-a:src","supervision_policy":"round20-policy-v1","acceptance_granted":true}"#
    )
    .is_err());
}
