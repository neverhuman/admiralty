//! Round 8 cross-packet qualification for recorded Codex and node telemetry.

use admiralty_core::conflicts::{CandidateChange, FileChange, analyze_conflicts};
use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeId, NodeIdentity, NodeRegistry, NodeScope,
    decode_event, reduce_enrollment_for,
};
use admiralty_core::placement::{
    BudgetRequirement, Capacity, DecimalU64 as PlacementTick, Known, NodeId as PlacementNodeId,
    NodeOffer, Observation, PlacementPolicy, PlacementRequest, propose_placement,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::MANAGED_EXECUTION_QUALIFIED;
use admiralty_runtime::{
    providers::codex::{CodexEffort, CodexError, CodexNdjsonStream},
    providers::{claude, codex, process},
    recovery::{Blocker, plan_recovery},
    session::{LaunchSpec, OutputChunk, OutputStream, validate_launch_envelope},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use admiralty_store::artifacts::ArtifactStore;
#[path = "support/recovery.rs"]
mod recovery_fixture;
use std::{fs, path::PathBuf};

use admiralty_core::economics::{DecimalU128, ObservedAmount};

const TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CODEX_TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");
const CLAUDE_TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn issued_token(id: &str, expires: u64, now: u64) -> NodeRegistry {
    let event_scope = scope();
    reduce_enrollment_for(
        &NodeRegistry::default(),
        &EnrollmentEvent::TokenIssued {
            event_id: format!("issue-{id}"),
            token: EnrollmentToken {
                token_id: id.into(),
                token_digest: digest('a'),
                scope: event_scope.clone(),
                expires_tick: DecimalU64::new(expires),
                consumed_binding: None,
            },
        },
        &event_scope,
        &event_scope.authority,
        Some(now),
    )
    .expect("issue scoped fixture token")
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("p12-p23-project").expect("project fixture"),
        root_id: RootId::new("p12-p23-root").expect("root fixture"),
        task_id: TaskId::new("p12-p23-task").expect("task fixture"),
        authority: PrincipalId::new("p12-p23-captain").expect("authority fixture"),
    }
}

fn placement_fixture() -> (PlacementRequest, NodeOffer, PlacementPolicy) {
    let tick = |value| PlacementTick::new(value);
    let request = PlacementRequest {
        project: ProjectId::new("round9-project").expect("project"),
        root: RootId::new("round9-root").expect("root"),
        task: TaskId::new("round9-task").expect("task"),
        contract_digest: digest('a'),
        authority_ref: "round9-grant".into(),
        required_capabilities: vec!["ingest".into()],
        locality_refs: vec!["payload".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "pool".into(),
            amount: tick(1),
        },
    };
    let offer = NodeOffer {
        node_id: PlacementNodeId::new("arbitrary-node").expect("node"),
        boot_id: "boot-1".into(),
        authority_generation: tick(1),
        visible_scopes: Observation {
            value: vec!["round9-project".into()],
            observed_at: tick(10),
        },
        capabilities: Observation {
            value: vec!["ingest".into()],
            observed_at: tick(10),
        },
        account_pool: Observation {
            value: "pool".into(),
            observed_at: tick(10),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: tick(1),
                    budget_remaining: tick(2),
                },
            },
            observed_at: tick(10),
        },
        current_pressure: Observation {
            value: Known::Known { value: tick(0) },
            observed_at: tick(10),
        },
        spool_free: Observation {
            value: Known::Known { value: tick(4) },
            observed_at: tick(10),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["payload".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(tick(10)),
        ownership_revision: Some(tick(1)),
    };
    let policy = PlacementPolicy {
        max_observation_age: tick(10),
        maximum_pressure: tick(5),
        minimum_spool_reserve: tick(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(tick(1)),
    };
    (request, offer, policy)
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
    // The console's sentence-case copy (21 October 2026 redesign) still states
    // that the cockpit is read-only and names its source.
    assert!(
        cockpit.contains(
            "`Read only. Source: ${legacyView ? 'producer snapshot' : 'native fleet API'}`"
        )
    );
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
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}

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
}

#[test]
fn p23_p25_round9_item3_webhook_node_ingest_qualification() {
    // P04: content-addressed body storage makes exact webhook retries stable.
    let temp = tempfile::tempdir().expect("temporary artifact fixture");
    let object_root = temp.path().join("objects");
    fs::create_dir(&object_root).expect("artifact root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&object_root, fs::Permissions::from_mode(0o700))
            .expect("restrict artifact root");
    }
    let artifacts = ArtifactStore::open(&object_root, 1024).expect("artifact store");
    let body = b"deterministic node webhook fixture";
    let reference = artifacts.publish_bytes(body).expect("publish webhook body");
    assert_eq!(
        artifacts.publish_bytes(body).expect("idempotent retry"),
        reference
    );
    assert_eq!(
        artifacts.read_verified(&reference).expect("verified body"),
        body
    );

    // P07/P21: a retry cannot bridge a journal gap or promote malformed bytes.
    let mut parser =
        CodexNdjsonStream::new("fixture-model", CodexEffort::Low).expect("fixture model");
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: 1,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::JournalGap)
    ));
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::Failed)
    ));

    // P23 webhook node ingest is strict, scoped and replay deterministic.
    // Transport authentication is not implemented, so the event remains data.
    let admission_scope = scope();
    let actor = admission_scope.authority.clone();
    let issued = issued_token("round9-ingest", 20, 1);
    let event = EnrollmentEvent::Enrolled {
        event_id: "round9-enroll".into(),
        token_id: "round9-ingest".into(),
        scope: admission_scope.clone(),
        identity: NodeIdentity {
            node_id: NodeId::new("round9-node").expect("node fixture"),
            boot_id: "boot-1".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let payload = serde_json::to_vec(&event).expect("serialize stable webhook payload");
    let decoded = decode_event(&payload).expect("strict node event decode");
    let enrolled = reduce_enrollment_for(&issued, &decoded, &admission_scope, &actor, Some(2))
        .expect("scoped enrollment");
    assert_eq!(
        reduce_enrollment_for(&enrolled, &decoded, &admission_scope, &actor, Some(2))
            .expect("exact webhook replay"),
        enrolled
    );
    let foreign = PrincipalId::new("foreign-webhook-actor").expect("foreign actor");
    assert!(reduce_enrollment_for(&issued, &decoded, &admission_scope, &foreign, Some(2)).is_err());
    let mut unknown = serde_json::to_value(&event).expect("event object");
    unknown
        .as_object_mut()
        .expect("object")
        .insert("signature_valid".into(), serde_json::json!(true));
    assert!(decode_event(&serde_json::to_vec(&unknown).expect("encode unknown field")).is_err());

    // P18: a changed ancestry cannot reuse an otherwise identical conflict proof.
    let candidate = |ancestry: char| CandidateChange {
        project_id: ProjectId::new("round9-project").expect("project"),
        root_id: RootId::new("round9-root").expect("root"),
        task_id: TaskId::new("round9-task").expect("task"),
        candidate_id: "round9-candidate".into(),
        base_sha: digest('d'),
        head_sha: digest('e'),
        tree_sha: digest('f'),
        ancestry_digest: Some(digest(ancestry)),
        files: vec![FileChange {
            path: "src/node.rs".into(),
            hunks: None,
        }],
        constraints: None,
    };
    let a = candidate('a');
    let b = candidate('b');
    let first = analyze_conflicts(std::slice::from_ref(&a), "round9-policy").expect("analysis");
    let changed = analyze_conflicts(std::slice::from_ref(&b), "round9-policy").expect("analysis");
    assert_ne!(first.source_digest, changed.source_digest);

    // P24: a disconnected node cannot start ordinary ingest work.
    let (request, mut offer, policy) = placement_fixture();
    offer.controller_connected = false;
    let placement = propose_placement(&request, &[offer], &policy, 10).expect("placement proposal");
    assert!(placement.candidates.is_empty());
    assert!(!placement.execution_granted && !placement.publication_granted);

    // P25: recovery cannot propose a resume without the external executor fence.
    let (manifest, observation, _fence) = recovery_fixture::valid_recovery_evidence();
    let recovery = plan_recovery(&manifest, &observation, None).expect("bounded recovery plan");
    assert!(recovery.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!recovery.resume_proposed);
    assert!(!recovery.effects_authorized());

    // P26: missing webhook cost remains unknown rather than zero.
    let cost = serde_json::to_value(ObservedAmount::Unknown {
        reason: "node webhook did not report cost".into(),
    })
    .expect("serialize unknown economics");
    assert_eq!(cost["kind"], "unknown");
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}
