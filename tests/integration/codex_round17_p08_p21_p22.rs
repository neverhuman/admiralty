use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::{agy, claude, codex, grok, supervision},
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, GitMetadata, NodeRef, WorkspaceSpec},
};
use serde_json::Value;
use std::{fs, path::PathBuf};

const CODEX_TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
const AGY_CAPABILITIES: &str = include_str!("../fixtures/providers/agy/1.4.2/capabilities.json");
const GROK_CAPABILITIES: &str = include_str!("../fixtures/providers/grok/1.0.62/capabilities.json");

fn fixture<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("checked-in release fixture is valid: {error:?}"),
    }
}

fn help_evidence(source: &str) -> String {
    let value: Value = fixture(serde_json::from_str(source));
    let Some(entries) = value.get("help_evidence").and_then(Value::as_array) else {
        panic!("provider matrix fixture contains help evidence");
    };
    let evidence: Vec<&str> = entries.iter().filter_map(Value::as_str).collect();
    assert_eq!(evidence.len(), entries.len());
    evidence.join(" ")
}

#[test]
fn p08_p21_p22_round17_post_item5_release_audit_cross_packet_invariant_qualification() {
    // P08: the same recorded Codex turn is invariant across fragment partitions and replays.
    let parse_codex = |chunk_size: usize| {
        let mut parser = fixture(codex::CodexNdjsonStream::new(
            "round17-model",
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
    assert_eq!(parse_codex(1), codex_baseline);
    for chunk_size in [3, 8, 17, 29] {
        assert_eq!(parse_codex(chunk_size), codex_baseline);
    }
    assert!(!codex_baseline.1.trusted_acceptance());
    assert!(!codex_baseline.1.native_execution_qualified());

    let mut broken = fixture(codex::CodexNdjsonStream::new(
        "round17-model",
        codex::CodexEffort::Medium,
    ));
    assert!(matches!(
        broken.push_chunk(&OutputChunk {
            offset: 1,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(codex::CodexError::JournalGap)
    ));
    assert!(matches!(
        broken.push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(codex::CodexError::Failed)
    ));

    // P21: Claude's native usage replay is stable and malformed framing yields no turn.
    let claude_first = fixture(claude::parse_stream(CLAUDE_TURN, Some("fixture-session")));
    let claude_second = fixture(claude::parse_stream(CLAUDE_TURN, Some("fixture-session")));
    assert_eq!(claude_first, claude_second);
    assert_eq!(claude_first.usage.input_tokens, Some(2));
    assert_eq!(claude_first.usage.cache_read_input_tokens, Some(649));
    assert_eq!(claude_first.usage.cache_creation_input_tokens, Some(1991));
    assert!(!claude_first.trusted_acceptance());
    assert!(!claude_first.native_execution_qualified());
    assert!(claude::parse_stream(b"{\"type\":\"message_start\"}\n", None).is_err());

    // P22: pinned provider policy replays exactly; unsupported versions fail closed.
    let agy_evidence = help_evidence(AGY_CAPABILITIES);
    let agy_first = fixture(agy::qualify("1.4.2", &agy_evidence));
    let agy_second = fixture(agy::qualify("1.4.2", &agy_evidence));
    assert_eq!(agy_first, agy_second);
    assert_eq!(agy_first.stream_json, agy::Capability::Advertised);
    assert!(matches!(
        agy_first.auth_status,
        agy::Capability::Unsupported(_)
    ));
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

    // P08 supervision replay stays deterministic and proposes no external authority.
    let run_supervision = || {
        let tick = |value: u64| fixture(supervision::Tick::new(value.to_string()));
        let policy = supervision::DetectionPolicy {
            version: "round17-policy-v1".into(),
            stall_after: tick(12),
            hysteresis_samples: 3,
            cooldown: tick(5),
        };
        let mut previous = None;
        let mut proposals = Vec::new();
        for now_value in 500..524 {
            let now = tick(now_value);
            let signal = tick(now_value.saturating_sub(2));
            let observation = supervision::SupervisionObservation {
                scope: supervision::SupervisionScope {
                    project_id: fixture(ProjectId::new("round17-project")),
                    root_id: fixture(RootId::new("round17-root")),
                    task_id: fixture(TaskId::new("round17-task")),
                    attempt_id: fixture(AttemptId::new("round17-attempt")),
                    owner: fixture(PrincipalId::new("round17-owner")),
                    incarnation: "boot-1".into(),
                    authority_ref: "grant:round17".into(),
                    contract_digest: format!("sha256:{}", "c".repeat(64)),
                    revision: 1,
                },
                clocks: supervision::SignalClocks {
                    last_byte: Some(signal.clone()),
                    provider_event: Some(signal.clone()),
                    tool_heartbeat: Some(signal.clone()),
                    artifact_change: Some(signal.clone()),
                    distinct_command: Some(signal.clone()),
                    completed_check: Some(signal.clone()),
                    verified_milestone: Some(signal),
                },
                process_state: supervision::ProcessState::Alive,
                tool_state: supervision::ToolState::Running,
                provider_wait: supervision::ProviderWait::None,
                approval_observation: None,
            };
            let proposal = fixture(supervision::evaluate_supervision(
                &policy,
                &observation,
                previous.as_ref(),
                Some(&now),
            ));
            assert!(!proposal.may_signal());
            assert!(!proposal.may_send_input());
            assert!(!proposal.execution_granted());
            previous = Some(proposal.state.clone());
            proposals.push(proposal);
        }
        proposals
    };
    assert_eq!(run_supervision(), run_supervision());

    let mut bad_policy = supervision::DetectionPolicy {
        version: "round17-policy-v1".into(),
        stall_after: fixture(supervision::Tick::new("12")),
        hysteresis_samples: 3,
        cooldown: fixture(supervision::Tick::new("5")),
    };
    let scope = supervision::SupervisionScope {
        project_id: fixture(ProjectId::new("round17-project")),
        root_id: fixture(RootId::new("round17-root")),
        task_id: fixture(TaskId::new("round17-task")),
        attempt_id: fixture(AttemptId::new("round17-attempt")),
        owner: fixture(PrincipalId::new("round17-owner")),
        incarnation: "boot-1".into(),
        authority_ref: "grant:round17".into(),
        contract_digest: format!("sha256:{}", "c".repeat(64)),
        revision: 1,
    };
    let tick = fixture(supervision::Tick::new("500"));
    let observed = fixture(supervision::Tick::new("498"));
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
    let first = fixture(supervision::evaluate_supervision(
        &bad_policy,
        &observation,
        None,
        Some(&tick),
    ));
    bad_policy.version = "round17-policy-v2".into();
    assert_eq!(
        supervision::evaluate_supervision(
            &bad_policy,
            &observation,
            Some(&first.state),
            Some(&fixture(supervision::Tick::new("501"))),
        ),
        Err(supervision::SupervisionError::PolicyDrift)
    );

    // Workspace custody is replayable, path-scoped, and cannot include sibling/private paths.
    let directory = fixture(tempfile::tempdir());
    let root_a = directory.path().join("worktrees/worker-a");
    let root_b = directory.path().join("worktrees/worker-b");
    for (worker, root) in [("worker-a", &root_a), ("worker-b", &root_b)] {
        fixture(fs::create_dir_all(root.join("src")));
        fixture(fs::write(root.join("src/state.rs"), worker.as_bytes()));
        fixture(fs::create_dir_all(
            directory.path().join(format!(".git/worktrees/{worker}")),
        ));
        fixture(fs::write(
            root.join(".git"),
            format!("gitdir: ../../.git/worktrees/{worker}\n"),
        ));
    }
    let make_spec = |node: &str, root_id: &str, root: &std::path::Path| WorkspaceSpec {
        node_id: fixture(NodeRef::new(node)),
        root_id: fixture(RootId::new(root_id)),
        project_id: fixture(ProjectId::new("round17-project")),
        workspace: fixture(fs::canonicalize(root)),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let spec_a = make_spec("worker-a", "root-a", &root_a);
    let spec_b = make_spec("worker-b", "root-b", &root_b);
    let valid_a = fixture(spec_a.validate());
    let valid_b = fixture(spec_b.validate());
    assert_eq!(valid_a.custody, Custody::RegisteredOnly);
    assert_eq!(valid_a.git_metadata, GitMetadata::WorktreeFile);
    assert_eq!(valid_b.git_metadata, GitMetadata::WorktreeFile);
    for _ in 0..24 {
        assert_eq!(fixture(spec_a.validate()), valid_a);
        assert_eq!(fixture(spec_b.validate()), valid_b);
        assert!(valid_a.owns(std::path::Path::new("src/state.rs")));
        assert!(valid_b.owns(std::path::Path::new("src/state.rs")));
        assert!(!valid_a.owns(std::path::Path::new("../worker-b/src/state.rs")));
        assert!(!valid_b.owns(std::path::Path::new("../worker-a/src/state.rs")));
        assert!(!valid_a.owns(std::path::Path::new("src/private/key")));
    }
    let mut traversal = spec_a;
    traversal.owned_paths = vec![PathBuf::from("../worker-b")];
    assert!(traversal.validate().is_err());
}
