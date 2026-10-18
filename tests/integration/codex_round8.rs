//! Round 8 cross-packet qualification for recorded Codex and node telemetry.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeId, NodeIdentity, NodeRegistry, NodeScope,
    reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::{claude, codex, process},
    providers::codex::{CodexEffort, CodexNdjsonStream},
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{fs, path::PathBuf};

use admiralty_core::economics::{DecimalU128, ObservedAmount};
use admiralty_runtime::session::{LaunchSpec, validate_launch_envelope};

const TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CODEX_TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("p12-p23-project").expect("project fixture"),
        root_id: RootId::new("p12-p23-root").expect("root fixture"),
        task_id: TaskId::new("p12-p23-task").expect("task fixture"),
        authority: PrincipalId::new("p12-p23-captain").expect("authority fixture"),
    }
}

#[test]
fn p23_round8_p12_tui_node_telemetry_qualification() {
    // Codex usage telemetry is captured deterministically from fragmented TUI output.
    let mut parser = CodexNdjsonStream::new("fixture-model", CodexEffort::Medium)
        .expect("fixture model accepted");
    let mut offset = 0_u64;
    let mut usage = None;
    for chunk in TURN.chunks(5) {
        let events = parser
            .push_chunk(&OutputChunk {
                offset,
                stream: OutputStream::Stdout,
                bytes: chunk.to_vec(),
            })
            .expect("contiguous deterministic TUI output");
        offset += chunk.len() as u64;
        usage = events.into_iter().find_map(|event| event.usage).or(usage);
    }
    assert_eq!(
        usage.as_ref().and_then(|u| u.input_tokens.as_deref()),
        Some("100")
    );
    assert_eq!(
        usage
            .as_ref()
            .and_then(|u| u.cached_input_tokens.as_deref()),
        Some("90")
    );
    assert_eq!(
        usage.as_ref().and_then(|u| u.output_tokens.as_deref()),
        Some("3")
    );
    let codex_summary = parser.finish(Some(0), true);
    assert!(!codex_summary.trusted_acceptance());
    assert!(!codex_summary.native_execution_qualified());

    // P23's capacity telemetry is scoped to the enrolled node and explicit tick.
    let admission_scope = scope();
    let actor = admission_scope.authority.clone();
    let token = EnrollmentToken {
        token_id: "p12-p23-token".into(),
        token_digest: digest('a'),
        scope: admission_scope.clone(),
        expires_tick: DecimalU64::new(20),
        consumed_binding: None,
    };
    let issued = reduce_enrollment_for(
        &NodeRegistry::default(),
        &EnrollmentEvent::TokenIssued {
            event_id: "issue".into(),
            token,
        },
        &admission_scope,
        &actor,
        Some(1),
    )
    .expect("scoped token issue");
    let node_id = NodeId::new("telemetry-node").expect("node fixture");
    let enrolled = reduce_enrollment_for(
        &issued,
        &EnrollmentEvent::Enrolled {
            event_id: "enroll".into(),
            token_id: "p12-p23-token".into(),
            scope: admission_scope.clone(),
            identity: NodeIdentity {
                node_id: node_id.clone(),
                boot_id: "boot-1".into(),
                public_key_digest: digest('b'),
                generation: DecimalU64::new(1),
                capability_digest: digest('c'),
            },
        },
        &admission_scope,
        &actor,
        Some(2),
    )
    .expect("node enrolled");
    let capacity = EnrollmentEvent::CapacityObserved {
        event_id: "capacity".into(),
        node_id: node_id.clone(),
        boot_id: "boot-1".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(3)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('d'),
    };
    let observed = reduce_enrollment_for(&enrolled, &capacity, &admission_scope, &actor, Some(5))
        .expect("fresh node telemetry accepted");
    assert_eq!(
        observed.capacity[&node_id].observed_tick,
        DecimalU64::new(3)
    );
    assert_eq!(observed.capacity[&node_id].capacity_digest, digest('d'));
    assert!(
        reduce_enrollment_for(&enrolled, &capacity, &admission_scope, &actor, Some(5))
            .expect("exact telemetry replay")
            == observed
    );
}

// This cross-packet qualification keeps the cockpit read-only, preserves exact
// economics values, and rejects unsafe launch envelopes with fixed fixtures.
#[test]
fn p26_p31_round8_p12_hitl_cockpit_qualification() {
    let cockpit = include_str!("../../apps/dashboard/src/ui/App.tsx");
    assert!(cockpit.contains("READ ONLY · SOURCE:"));
    assert!(cockpit.contains("Candidate reports await independent verification"));
    assert!(cockpit.contains("Native execution remains disabled."));
    assert!(cockpit.contains("Sending, replying and changing task state are not available here."));

    let exact_cost = DecimalU128(u128::MAX);
    let encoded = serde_json::to_string(&ObservedAmount::Known { amount: exact_cost })
        .expect("serialize exact economics amount");
    let decoded: ObservedAmount = serde_json::from_str(&encoded).expect("decode exact amount");
    assert_eq!(
        decoded,
        ObservedAmount::Known {
            amount: DecimalU128(u128::MAX)
        }
    );

    let safe = LaunchSpec {
        run_id: "round8-qualification".into(),
        program: "/usr/bin/codex".into(),
        args: vec!["exec".into(), "--version".into()],
        cwd: "/workspace/project".into(),
        spool_dir: "/workspace/project/.spool".into(),
    };
    assert!(validate_launch_envelope(&safe).is_ok());

    let mut relative = safe.clone();
    relative.program = "codex".into();
    assert!(validate_launch_envelope(&relative).is_err());

    let mut oversized = safe;
    oversized.args = vec!["x".repeat(64 * 1024 + 1)];
    assert!(validate_launch_envelope(&oversized).is_err());

#[test]
fn p08_p21_p22_round8_worktree_isolation_receipt_qualification() {
    let temp = tempfile::tempdir().expect("temporary receipt workspace");
    let root = fs::canonicalize(temp.path()).expect("canonical root");
    let worktree = root.join("worktree");
    let sibling = root.join("sibling");
    fs::create_dir_all(worktree.join("src")).expect("worktree source");
    fs::create_dir_all(&sibling).expect("sibling workspace");
    fs::write(worktree.join("src/main.rs"), b"candidate").expect("candidate fixture");
    fs::write(sibling.join("private.receipt"), b"sibling sentinel").expect("sibling sentinel");

    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round8-node").expect("node ID"),
        root_id: RootId::new("round8-root").expect("root ID"),
        project_id: ProjectId::new("round8-project").expect("project ID"),
        workspace: worktree.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("private.receipt")],
    };
    let scope = spec.validate().expect("registered worktree scope");
    assert_eq!(scope.custody, Custody::RegisteredOnly);
    assert!(scope.owns(std::path::Path::new("src/main.rs")));
    assert!(!scope.owns(std::path::Path::new("../sibling/private.receipt")));
    assert_eq!(
        fs::read(sibling.join("private.receipt")).expect("sibling receipt remains intact"),
        b"sibling sentinel"
    );

    // P08 and P21 fixture parsing is repeatable and remains observational evidence.
    let codex = codex::CodexNdjsonStream::new("fixture-model", codex::CodexEffort::Medium)
        .expect("Codex parser configuration");
    let mut codex = codex;
    let codex_events = codex
        .push_chunk(&admiralty_runtime::session::OutputChunk {
            offset: 0,
            stream: admiralty_runtime::session::OutputStream::Stdout,
            bytes: CODEX_TURN.to_vec(),
        })
        .expect("Codex fixture parses");
    assert!(!codex_events.is_empty());
    assert!(!codex.finish(Some(0), true).trusted_acceptance());

    let claude =
        claude::parse_stream(CLAUDE_TURN, Some("fixture-session")).expect("Claude fixture parses");
    assert!(!claude.trusted_acceptance());

    // P22 legacy identity is only a hint and cannot turn a receipt into acceptance.
    let identity = process::classify_legacy(
        &["/opt/agy".into(), "--model".into(), "fixture-model".into()],
        None,
        None,
    )
    .expect("bounded legacy identity");
    assert!(identity.observed_only);
    assert_eq!(identity.observed_provider, None);

