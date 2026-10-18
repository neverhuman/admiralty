use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::{agy, claude, codex, grok, process, supervision},
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, GitMetadata, NodeRef, WorkspaceSpec},
};
use serde_json::Value;
use std::{fs, path::PathBuf};

const CODEX_TURN: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/claude/continuity-turn.jsonl");
const AGY_CAPABILITIES: &str =
    include_str!("../../../../tests/fixtures/providers/agy/1.4.2/capabilities.json");
const GROK_CAPABILITIES: &str =
    include_str!("../../../../tests/fixtures/providers/grok/1.0.62/capabilities.json");

fn fixture<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("checked-in deterministic fixture is valid: {error:?}"),
    }
}

fn help_evidence(source: &str) -> String {
    let value: Value = fixture(serde_json::from_str(source));
    let Some(entries) = value.get("help_evidence").and_then(Value::as_array) else {
        panic!("provider fixture contains help evidence");
    };
    let evidence: Vec<&str> = entries.iter().filter_map(Value::as_str).collect();
    assert_eq!(evidence.len(), entries.len());
    evidence.join(" ")
}

#[test]
fn p08_p21_p22_round15_post_item4_release_smoke_multistep_stress_qualification() {
    // P08: fragmented replay is independent of chunk boundaries and remains untrusted.
    let parse_codex = |chunk_size: usize| {
        let mut parser = fixture(codex::CodexNdjsonStream::new(
            "round15-model",
            codex::CodexEffort::Medium,
        ));
        let mut offset = 0_u64;
        let mut observations = Vec::new();
        for bytes in CODEX_TURN.chunks(chunk_size) {
            observations.extend(fixture(parser.push_chunk(&OutputChunk {
                offset,
                stream: OutputStream::Stdout,
                bytes: bytes.to_vec(),
            })));
            offset += bytes.len() as u64;
        }
        let summary = parser.finish(Some(0), true);
        (observations, summary)
    };
    let codex_baseline = parse_codex(1);
    for chunk_size in [2, 5, 13, 31] {
        assert_eq!(parse_codex(chunk_size), codex_baseline);
    }
    assert!(!codex_baseline.1.trusted_acceptance());
    assert!(!codex_baseline.1.native_execution_qualified());

    // P21: successful Claude usage replay is stable; malformed framing fails closed.
    let claude_first = fixture(claude::parse_stream(CLAUDE_TURN, Some("fixture-session")));
    let claude_second = fixture(claude::parse_stream(CLAUDE_TURN, Some("fixture-session")));
    assert_eq!(claude_first, claude_second);
    assert_eq!(claude_first.usage.cache_read_input_tokens, Some(649));
    assert_eq!(claude_first.usage.cache_creation_input_tokens, Some(1991));
    assert!(!claude_first.trusted_acceptance());
    assert!(!claude_first.native_execution_qualified());
    assert!(claude::parse_stream(b"{\"type\":\"message_start\"}\n", None).is_err());

    // P22: pinned advertised capability rows replay, while drift and malformed versions deny.
    let agy_evidence = help_evidence(AGY_CAPABILITIES);
    let agy_first = fixture(agy::qualify("1.4.2", &agy_evidence));
    let agy_second = fixture(agy::qualify("1.4.2", &agy_evidence));
    assert_eq!(agy_first, agy_second);
    assert_eq!(agy_first.stream_json, agy::Capability::Advertised);
    assert!(!agy_first.native_execution_qualified());
    assert!(agy::qualify("1.4.3", &agy_evidence).is_err());

    let grok_evidence = help_evidence(GROK_CAPABILITIES);
    let grok_first = fixture(grok::qualify(
        "grok 1.0.62 (1474f9cbdd91) [stable]",
        &grok_evidence,
    ));
    let grok_second = fixture(grok::qualify(
        "grok 1.0.62 (1474f9cbdd91) [stable]",
        &grok_evidence,
    ));
    assert_eq!(grok_first, grok_second);
    assert_eq!(grok_first.acp_stream, grok::Capability::Advertised);
    assert!(!grok_first.native_execution_qualified());
    assert!(grok::qualify("grok 1.0.63 (next) [stable]", &grok_evidence).is_err());

    // Legacy process hints remain observation only across repeated classification.
    let argv = vec![
        "/opt/agy".to_owned(),
        "--model".to_owned(),
        "fixture".to_owned(),
    ];
    let legacy_first = fixture(process::classify_legacy(
        &argv,
        Some("recycled-session"),
        Some("untrusted provider transcript"),
    ));
    let legacy_second = fixture(process::classify_legacy(
        &argv,
        Some("recycled-session"),
        Some("untrusted provider transcript"),
    ));
    assert_eq!(legacy_first, legacy_second);
    assert!(legacy_first.observed_only);
    assert_eq!(legacy_first.observed_provider, None);
    assert_eq!(legacy_first.observed_model, None);

    // P08 supervision stress: replay each multi-step sample sequence byte-for-byte.
    let run_supervision = || {
        let make_tick = |value: u64| fixture(supervision::Tick::new(value.to_string()));
        let policy = supervision::DetectionPolicy {
            version: "round15-policy-v1".into(),
            stall_after: make_tick(8),
            hysteresis_samples: 2,
            cooldown: make_tick(4),
        };
        let mut prior = None;
        let mut proposals = Vec::new();
        for now_value in 100..124 {
            let now = make_tick(now_value);
            let observed = make_tick(now_value.saturating_sub(2));
            let scope = supervision::SupervisionScope {
                project_id: fixture(ProjectId::new("round15-project")),
                root_id: fixture(RootId::new("round15-root")),
                task_id: fixture(TaskId::new("round15-task")),
                attempt_id: fixture(AttemptId::new("round15-attempt")),
                owner: fixture(PrincipalId::new("round15-owner")),
                incarnation: "boot-1".into(),
                authority_ref: "grant:round15".into(),
                contract_digest: format!("sha256:{}", "b".repeat(64)),
                revision: 1,
            };
            let observation = supervision::SupervisionObservation {
                scope,
                clocks: supervision::SignalClocks {
                    last_byte: Some(observed.clone()),
                    provider_event: Some(observed.clone()),
                    tool_heartbeat: Some(observed.clone()),
                    artifact_change: Some(observed.clone()),
                    distinct_command: Some(observed.clone()),
                    completed_check: Some(observed.clone()),
                    verified_milestone: Some(observed),
                },
                process_state: supervision::ProcessState::Alive,
                tool_state: supervision::ToolState::Running,
                provider_wait: supervision::ProviderWait::None,
                approval_observation: None,
            };
            let proposal = fixture(supervision::evaluate_supervision(
                &policy,
                &observation,
                prior.as_ref(),
                Some(&now),
            ));
            assert!(!proposal.may_signal());
            assert!(!proposal.may_send_input());
            assert!(!proposal.execution_granted());
            prior = Some(proposal.state.clone());
            proposals.push(proposal);
        }
        proposals
    };
    assert_eq!(run_supervision(), run_supervision());
    let unknown_clock = supervision::evaluate_supervision(
        &supervision::DetectionPolicy {
            version: "round15-policy-v1".into(),
            stall_after: fixture(supervision::Tick::new("8")),
            hysteresis_samples: 2,
            cooldown: fixture(supervision::Tick::new("4")),
        },
        &supervision::SupervisionObservation {
            scope: supervision::SupervisionScope {
                project_id: fixture(ProjectId::new("round15-project")),
                root_id: fixture(RootId::new("round15-root")),
                task_id: fixture(TaskId::new("round15-task")),
                attempt_id: fixture(AttemptId::new("round15-attempt")),
                owner: fixture(PrincipalId::new("round15-owner")),
                incarnation: "boot-1".into(),
                authority_ref: "grant:round15".into(),
                contract_digest: format!("sha256:{}", "b".repeat(64)),
                revision: 1,
            },
            clocks: supervision::SignalClocks::default(),
            process_state: supervision::ProcessState::Unknown,
            tool_state: supervision::ToolState::Unknown,
            provider_wait: supervision::ProviderWait::None,
            approval_observation: None,
        },
        None,
        None,
    );
    assert_eq!(
        unknown_clock,
        Err(supervision::SupervisionError::UnknownClock)
    );

    // Workspace smoke and stress: two registered roots never acquire sibling ownership.
    let directory = fixture(tempfile::tempdir());
    let root_a = directory.path().join("worktrees/worker-a");
    let root_b = directory.path().join("worktrees/worker-b");
    fixture(fs::create_dir_all(
        directory.path().join(".git/worktrees/worker-a"),
    ));
    fixture(fs::create_dir_all(
        directory.path().join(".git/worktrees/worker-b"),
    ));
    fixture(fs::create_dir_all(root_a.join("src")));
    fixture(fs::create_dir_all(root_b.join("src")));
    fixture(fs::write(root_a.join("src/state.rs"), b"a"));
    fixture(fs::write(root_b.join("src/state.rs"), b"b"));
    fixture(fs::write(
        root_a.join(".git"),
        b"gitdir: ../../.git/worktrees/worker-a\n",
    ));
    fixture(fs::write(
        root_b.join(".git"),
        b"gitdir: ../../.git/worktrees/worker-b\n",
    ));
    let make_spec = |node: &str, root_id: &str, root: &std::path::Path| WorkspaceSpec {
        node_id: fixture(NodeRef::new(node)),
        root_id: fixture(RootId::new(root_id)),
        project_id: fixture(ProjectId::new("round15-project")),
        workspace: fixture(fs::canonicalize(root)),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let spec_a = make_spec("worker-a", "root-a", &root_a);
    let spec_b = make_spec("worker-b", "root-b", &root_b);
    let validated_a = fixture(spec_a.validate());
    let validated_b = fixture(spec_b.validate());
    assert_eq!(validated_a.custody, Custody::RegisteredOnly);
    assert_eq!(validated_a.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(validated_b.git_metadata, GitMetadata::WorktreeFile);
    for _ in 0..32 {
        assert_eq!(fixture(spec_a.validate()), validated_a);
        assert_eq!(fixture(spec_b.validate()), validated_b);
        assert!(validated_a.owns(std::path::Path::new("src/state.rs")));
        assert!(validated_b.owns(std::path::Path::new("src/state.rs")));
        assert!(!validated_a.owns(std::path::Path::new("../worker-b/src/state.rs")));
        assert!(!validated_b.owns(std::path::Path::new("../worker-a/src/state.rs")));
        assert!(!validated_a.owns(std::path::Path::new("src/private/key")));
    }
    assert!(fs::read(root_a.join("src/state.rs")).is_ok());
    assert!(fs::read(root_b.join("src/state.rs")).is_ok());
}
