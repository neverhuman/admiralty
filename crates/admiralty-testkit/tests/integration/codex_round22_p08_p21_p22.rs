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

const CLAUDE_SMOKE: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/claude/continuity-turn.jsonl");
const AGY_POLICY: &str =
    include_str!("../../../../tests/fixtures/providers/agy/1.4.2/capabilities.json");

fn checked<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("release canary fixture is valid: {error:?}"),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanaryReleaseRecord {
    schema_version: u32,
    qualification: String,
    provider_policy: Vec<String>,
    supervision_policy: String,
    workspace_scopes: Vec<String>,
    acceptance_granted: bool,
}

fn agy_help() -> String {
    let value: Value = checked(serde_json::from_str(AGY_POLICY));
    let Some(evidence) = value.get("help_evidence").and_then(Value::as_array) else {
        panic!("pinned provider fixture has help evidence");
    };
    let lines: Vec<&str> = evidence.iter().filter_map(Value::as_str).collect();
    assert_eq!(lines.len(), evidence.len());
    lines.join(" ")
}

#[test]
fn p08_p21_p22_round22_canary_smoke_release_package_qualification() {
    // P08 supervision produces deterministic observations without granting authority.
    let tick = |value: &str| checked(supervision::Tick::new(value));
    let observation = supervision::SupervisionObservation {
        scope: supervision::SupervisionScope {
            project_id: checked(ProjectId::new("round22-project")),
            root_id: checked(RootId::new("round22-root")),
            task_id: checked(TaskId::new("round22-task")),
            attempt_id: checked(AttemptId::new("round22-attempt")),
            owner: checked(PrincipalId::new("round22-owner")),
            incarnation: "canary-1".into(),
            authority_ref: "authority:round22".into(),
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            revision: 1,
        },
        clocks: supervision::SignalClocks {
            last_byte: Some(tick("300")),
            provider_event: Some(tick("300")),
            tool_heartbeat: Some(tick("300")),
            artifact_change: Some(tick("300")),
            distinct_command: Some(tick("300")),
            completed_check: Some(tick("300")),
            verified_milestone: Some(tick("300")),
        },
        process_state: supervision::ProcessState::Alive,
        tool_state: supervision::ToolState::Running,
        provider_wait: supervision::ProviderWait::None,
        approval_observation: None,
    };
    let policy = supervision::DetectionPolicy {
        version: "round22-canary-policy-v1".into(),
        stall_after: tick("20"),
        hysteresis_samples: 2,
        cooldown: tick("5"),
    };
    let now = tick("302");
    let first = checked(supervision::evaluate_supervision(
        &policy,
        &observation,
        None,
        Some(&now),
    ));
    let replay = checked(supervision::evaluate_supervision(
        &policy,
        &observation,
        None,
        Some(&now),
    ));
    assert_eq!(first, replay);
    assert!(!first.may_signal());
    assert!(!first.may_send_input());
    assert!(!first.execution_granted());

    // P21 pinned provider policy is repeatable and remains observation-only.
    let help = agy_help();
    let agy_first = checked(agy::qualify("1.4.2", &help));
    let agy_replay = checked(agy::qualify("1.4.2", &help));
    assert_eq!(agy_first, agy_replay);
    assert_eq!(agy_first.stream_json, agy::Capability::Advertised);
    assert!(!agy_first.native_execution_qualified());
    assert!(agy::qualify("1.4.3", &help).is_err());
    let claude_first = checked(claude::parse_stream(CLAUDE_SMOKE, Some("fixture-session")));
    let claude_replay = checked(claude::parse_stream(CLAUDE_SMOKE, Some("fixture-session")));
    assert_eq!(claude_first, claude_replay);
    assert!(!claude_first.trusted_acceptance());
    assert!(!claude_first.native_execution_qualified());

    // P22 canary workspace roots are temporary, distinct, and deny cross-root access.
    let temporary = checked(tempfile::tempdir());
    let root_a = temporary.path().join("canary/a");
    let root_b = temporary.path().join("canary/b");
    for (name, root) in [("a", &root_a), ("b", &root_b)] {
        checked(fs::create_dir_all(root.join("src")));
        checked(fs::create_dir_all(
            temporary.path().join(format!(".git/worktrees/{name}")),
        ));
        checked(fs::write(root.join("src/main.rs"), name.as_bytes()));
        checked(fs::write(
            root.join(".git"),
            format!("gitdir: ../../.git/worktrees/{name}\n"),
        ));
    }
    let make_spec = |name: &str, root_id: &str, root: &std::path::Path| WorkspaceSpec {
        node_id: checked(NodeRef::new(name)),
        root_id: checked(RootId::new(root_id)),
        project_id: checked(ProjectId::new("round22-project")),
        workspace: checked(fs::canonicalize(root)),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let spec_a = make_spec("canary-a", "root-a", &root_a);
    let spec_b = make_spec("canary-b", "root-b", &root_b);
    let validated_a = checked(spec_a.validate());
    let validated_b = checked(spec_b.validate());
    assert_eq!(validated_a.custody, Custody::RegisteredOnly);
    assert_eq!(validated_a.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(validated_b.git_metadata, GitMetadata::WorktreeFile);
    for _ in 0..12 {
        assert_eq!(checked(spec_a.validate()), validated_a);
        assert_eq!(checked(spec_b.validate()), validated_b);
        assert!(validated_a.owns(std::path::Path::new("src/main.rs")));
        assert!(validated_b.owns(std::path::Path::new("src/main.rs")));
        assert!(!validated_a.owns(std::path::Path::new("../b/src/main.rs")));
        assert!(!validated_b.owns(std::path::Path::new("../a/src/main.rs")));
        assert!(!validated_a.owns(std::path::Path::new("src/private/secret")));
    }

    // Canary release packaging schema is deterministic and cannot self-assert acceptance.
    let package = CanaryReleaseRecord {
        schema_version: 1,
        qualification: "P08-P21-P22-round22".into(),
        provider_policy: vec![
            "agy@1.4.2:observed".into(),
            "claude@fixture:observed".into(),
        ],
        supervision_policy: policy.version,
        workspace_scopes: vec!["root-a:src".into(), "root-b:src".into()],
        acceptance_granted: false,
    };
    let encoded = checked(serde_json::to_vec(&package));
    let decoded: CanaryReleaseRecord = checked(serde_json::from_slice(&encoded));
    assert_eq!(decoded, package);
    assert_eq!(
        Sha256::digest(&encoded),
        Sha256::digest(checked(serde_json::to_vec(&decoded)))
    );
    assert!(serde_json::from_slice::<CanaryReleaseRecord>(
        br#"{"schema_version":1,"qualification":"P08-P21-P22-round22","provider_policy":["agy@1.4.2:observed","claude@fixture:observed"],"supervision_policy":"round22-canary-policy-v1","workspace_scopes":["root-a:src","root-b:src"],"acceptance_granted":true,"self_attested":true}"#
    )
    .is_err());
}
