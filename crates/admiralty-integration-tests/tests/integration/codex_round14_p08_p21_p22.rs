use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::{agy, claude, codex, grok, supervision},
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{fs, path::PathBuf};

fn fixture<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("valid deterministic fixture: {error:?}"),
    }
}

const CODEX_TURN: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] =
    include_bytes!("../../../../tests/fixtures/providers/claude/continuity-turn.jsonl");
const AGY_CAPABILITIES: &str =
    include_str!("../../../../tests/fixtures/providers/agy/1.4.2/capabilities.json");
const GROK_CAPABILITIES: &str =
    include_str!("../../../../tests/fixtures/providers/grok/1.0.62/capabilities.json");

#[test]
fn codex_round14_p08_p21_p22() {
    // P08/P21: identical fixture replay has identical observations and terminal facts.
    let replay = || -> Result<_, codex::CodexError> {
        let mut parser =
            codex::CodexNdjsonStream::new("round14-model", codex::CodexEffort::Medium)?;
        let mut offset = 0_u64;
        let mut observations = Vec::new();
        for bytes in CODEX_TURN.chunks(9) {
            observations.extend(parser.push_chunk(&OutputChunk {
                offset,
                stream: OutputStream::Stdout,
                bytes: bytes.to_vec(),
            })?);
            offset += bytes.len() as u64;
        }
        Ok((observations, parser.finish(Some(0), true)))
    };
    let first = replay();
    let second = replay();
    assert!(first.is_ok() && second.is_ok());
    let (Ok(first), Ok(second)) = (first, second) else {
        return;
    };
    assert_eq!(first, second);
    assert!(!first.1.trusted_acceptance());
    assert!(!first.1.native_execution_qualified());

    // A journal gap poisons a Codex stream and cannot be repaired by replay.
    let broken = codex::CodexNdjsonStream::new("round14-model", codex::CodexEffort::Medium);
    assert!(broken.is_ok());
    let Ok(mut broken) = broken else { return };
    let gap = broken.push_chunk(&OutputChunk {
        offset: 1,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(gap, Err(codex::CodexError::JournalGap)));
    let retry = broken.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(retry, Err(codex::CodexError::Failed)));

    // P21/P22 provider parsing and policy reject malformed or unpinned evidence.
    let claude_turn = claude::parse_stream(CLAUDE_TURN, Some("fixture-session"));
    assert!(claude_turn.is_ok());
    if let Ok(turn) = claude_turn {
        assert_eq!(turn.usage.cache_read_input_tokens, Some(649));
        assert!(!turn.trusted_acceptance());
        assert!(!turn.native_execution_qualified());
    }
    assert!(claude::parse_stream(b"{\"type\":\"message_start\"}\n", None).is_err());
    assert!(agy::qualify("1.4.3", AGY_CAPABILITIES).is_err());
    assert!(grok::qualify("grok 1.0.63 (next) [stable]", GROK_CAPABILITIES).is_err());

    // P08 supervision replays the same observation deterministically and never authorizes.
    let tick = |n: u64| supervision::Tick::new(n.to_string());
    let tick100 = tick(100);
    let tick10 = tick(10);
    let tick5 = tick(5);
    assert!(tick100.is_ok() && tick10.is_ok() && tick5.is_ok());
    let (Ok(now), Ok(stall_after), Ok(cooldown)) = (tick100, tick10, tick5) else {
        return;
    };
    let scope = supervision::SupervisionScope {
        project_id: fixture(ProjectId::new("round14-project")),
        root_id: fixture(RootId::new("round14-root")),
        task_id: fixture(TaskId::new("round14-task")),
        attempt_id: fixture(AttemptId::new("round14-attempt")),
        owner: fixture(PrincipalId::new("round14-owner")),
        incarnation: "boot-1".into(),
        authority_ref: "grant:round14".into(),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        revision: 1,
    };
    let observation = supervision::SupervisionObservation {
        scope,
        clocks: supervision::SignalClocks {
            last_byte: Some(now.clone()),
            provider_event: Some(now.clone()),
            tool_heartbeat: Some(now.clone()),
            artifact_change: Some(now.clone()),
            distinct_command: Some(now.clone()),
            completed_check: Some(now.clone()),
            verified_milestone: Some(now.clone()),
        },
        process_state: supervision::ProcessState::Alive,
        tool_state: supervision::ToolState::Finished,
        provider_wait: supervision::ProviderWait::None,
        approval_observation: None,
    };
    let policy = supervision::DetectionPolicy {
        version: "round14-v1".into(),
        stall_after,
        hysteresis_samples: 2,
        cooldown,
    };
    let evaluated = supervision::evaluate_supervision(&policy, &observation, None, Some(&now));
    assert!(evaluated.is_ok());
    let Ok(proposal) = evaluated else { return };
    assert!(!proposal.may_signal());
    assert!(!proposal.may_send_input());
    assert!(!proposal.execution_granted());
    assert!(supervision::evaluate_supervision(&policy, &observation, None, None).is_err());

    // Workspace validation is replayable, and traversal cannot acquire ownership.
    let directory = fixture(tempfile::tempdir());
    let workspace = directory.path().join("workspace");
    fixture(fs::create_dir_all(workspace.join("src")));
    let canonical = fixture(fs::canonicalize(workspace));
    let spec = WorkspaceSpec {
        node_id: fixture(NodeRef::new("round14-node")),
        root_id: fixture(RootId::new("round14-root")),
        project_id: fixture(ProjectId::new("round14-project")),
        workspace: canonical,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private")],
    };
    let first = spec.validate();
    let second = spec.validate();
    assert!(first.is_ok() && second.is_ok());
    let (Ok(first), Ok(second)) = (first, second) else {
        return;
    };
    assert_eq!(first, second);
    assert_eq!(first.custody, Custody::RegisteredOnly);
    assert!(first.owns(std::path::Path::new("src/main.rs")));
    assert!(!first.owns(std::path::Path::new("src/private/key")));
    assert!(!first.owns(std::path::Path::new("../sibling/src/main.rs")));
    let mut traversal = spec;
    traversal.owned_paths = vec![PathBuf::from("../sibling")];
    assert!(traversal.validate().is_err());
}
