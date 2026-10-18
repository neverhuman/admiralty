use admiralty_core::{
    ProjectId, RootId, TaskId,
    economics::{DecimalU128, ObservedAmount},
    placement::{
        BudgetRequirement, Capacity, DecimalU64, Known, NodeId, NodeOffer, Observation,
        PlacementPolicy, PlacementRequest, propose_placement,
    },
};
use admiralty_runtime::{
    providers::codex::{
        CodexEffort, CodexError, CodexNdjsonStream, CodexTermination, MAX_STREAM_BYTES,
    },
    recovery::plan_recovery,
    session::{OutputChunk, OutputStream},
    workspace::registry::{Custody, GitMetadata, NodeRef, WorkspaceSpec},
};
use admiralty_testkit::{
    clock::FakeClock,
    faults::{FaultEffect, FaultPlan, FaultRule},
    forge::{Fault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator},
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[path = "support/recovery.rs"]
mod recovery_support;

const TURN: &[u8] = include_bytes!("../fixtures/providers/codex/fragmented-turn.jsonl");

#[test]
fn fragmented_codex_turn_preserves_usage_and_never_claims_acceptance() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Medium)
        .expect("explicit model accepted");
    let mut observations = Vec::new();
    let mut offset = 0_u64;

    for bytes in TURN.chunks(7) {
        observations.extend(
            parser
                .push_chunk(&OutputChunk {
                    offset,
                    stream: OutputStream::Stdout,
                    bytes: bytes.to_vec(),
                })
                .expect("fragment is accepted into the contiguous journal"),
        );
        offset += bytes.len() as u64;
    }

    let completed = observations
        .iter()
        .find(|observation| observation.event_type == "turn.completed")
        .expect("completion event observed");
    let usage = completed
        .usage
        .as_ref()
        .expect("native turn usage observed");
    assert_eq!(usage.input_tokens.as_deref(), Some("100"));
    assert_eq!(usage.cached_input_tokens.as_deref(), Some("90"));
    assert_eq!(usage.output_tokens.as_deref(), Some("3"));
    assert!(observations.iter().all(|event| !event.trusted_acceptance()));

    let summary = parser.finish(Some(0), true);
    assert_eq!(summary.termination, CodexTermination::ProviderTurnCompleted);
    assert_eq!(summary.thread_id.as_deref(), Some("fixture-thread"));
    assert_eq!(summary.raw_bytes, TURN.len().to_string());
    assert!(!summary.trusted_acceptance());
    assert!(!summary.native_execution_qualified());
}

#[test]
fn incomplete_codex_capture_stays_unknown() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{\"type\":\"thread.started\",\"thread_id\":\"fixture-thread\"}\n".to_vec(),
        })
        .expect("start event parses");

    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn journal_gap_poisoning_requires_reconciliation() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let gap = parser.push_chunk(&OutputChunk {
        offset: 1,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(gap, Err(CodexError::JournalGap)));

    let retry = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{}\n".to_vec(),
    });
    assert!(matches!(retry, Err(CodexError::Failed)));
}

#[test]
fn malformed_codex_json_yields_no_observation_and_poisoning_is_sticky() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let malformed = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{broken}\n".to_vec(),
    });
    assert!(matches!(malformed, Err(CodexError::Json(_))));
    assert!(parser.observations().is_empty());
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: b"{}\n".to_vec(),
        }),
        Err(CodexError::Failed)
    ));
}

#[test]
fn codex_stream_limit_rejects_first_byte_over_boundary() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let result = parser.push_chunk(&OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: vec![b' '; MAX_STREAM_BYTES + 1],
    });
    assert!(matches!(result, Err(CodexError::Bounds)));
    assert!(parser.observations().is_empty());
}

#[test]
fn replaying_a_codex_journal_chunk_cannot_duplicate_observations() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let chunk = OutputChunk {
        offset: 0,
        stream: OutputStream::Stdout,
        bytes: b"{\"type\":\"thread.started\",\"thread_id\":\"fixture-thread\"}\n".to_vec(),
    };
    let first = parser.push_chunk(&chunk).expect("initial journal chunk");
    assert_eq!(first.len(), 1);
    assert!(matches!(
        parser.push_chunk(&chunk),
        Err(CodexError::JournalGap)
    ));
    assert_eq!(parser.observations().len(), 1);
}

#[test]
fn invalid_utf8_in_a_fragmented_codex_event_is_rejected_without_observation() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let corrupted = b"{\"type\":\"thread.started\",\"thread_id\":\"\xc3(\"}\n";
    let split = corrupted
        .windows(2)
        .position(|pair| pair == b"\xc3(")
        .expect("corrupt UTF-8 sequence");
    assert!(
        parser
            .push_chunk(&OutputChunk {
                offset: 0,
                stream: OutputStream::Stdout,
                bytes: corrupted[..split + 1].to_vec(),
            })
            .is_ok()
    );
    assert!(matches!(
        parser.push_chunk(&OutputChunk {
            offset: (split + 1) as u64,
            stream: OutputStream::Stdout,
            bytes: corrupted[split + 1..].to_vec(),
        }),
        Err(CodexError::Json(_))
    ));
    assert!(parser.observations().is_empty());
}

#[test]
fn completed_provider_turn_stays_unknown_without_complete_capture() {
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN.to_vec(),
        })
        .expect("provider turn parses");
    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn injected_codex_tail_loss_keeps_capture_unqualified() {
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "codex-read".into(),
        occurrence: 2,
        effect: FaultEffect::Drop,
    }])
    .expect("bounded deterministic fault schedule");
    let mut parser = CodexNdjsonStream::new("selected-model", CodexEffort::Low)
        .expect("explicit model accepted");
    let split = TURN.len() / 2;
    assert_eq!(faults.hit("codex-read"), Ok(None));
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN[..split].to_vec(),
        })
        .expect("first provider read");
    assert_eq!(faults.hit("codex-read"), Ok(Some(FaultEffect::Drop)));

    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.capture_complete);
    assert!(!summary.trusted_acceptance());
}

#[test]
fn p24_p29_round8_p08_telegram_worktree_qualification() {
    // The deterministic Codex transcript may report a completed turn, but the
    // worktree qualification remains false until the capture is complete and
    // independently accepted by the surrounding workflow.
    let mut parser = CodexNdjsonStream::new("round8-fixture", CodexEffort::Medium)
        .expect("explicit fixture model accepted");
    let observations = parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN.to_vec(),
        })
        .expect("fixture transcript parses deterministically");

    assert!(
        observations
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );
    assert!(observations.iter().all(|event| !event.trusted_acceptance()));

    let incomplete = parser.finish(Some(0), false);
    assert_eq!(incomplete.termination, CodexTermination::Unknown);
    assert!(!incomplete.capture_complete);
    assert!(!incomplete.trusted_acceptance());
    assert!(!incomplete.native_execution_qualified());
}

fn d(value: u64) -> DecimalU64 {
    DecimalU64::new(value)
}

fn round9_placement_request() -> PlacementRequest {
    PlacementRequest {
        project: ProjectId::new("round9-project").expect("fixture project"),
        root: RootId::new("round9-root").expect("fixture root"),
        task: TaskId::new("round9-task").expect("fixture task"),
        contract_digest: format!("sha256:{}", "a".repeat(64)),
        authority_ref: "round9-grant".into(),
        required_capabilities: vec!["codex".into()],
        locality_refs: vec!["src".into()],
        budget_requirement: BudgetRequirement {
            account_pool: "pool".into(),
            amount: d(5),
        },
    }
}

fn round9_placement_offer() -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new("round9-node").expect("fixture node"),
        boot_id: "round9-boot".into(),
        authority_generation: d(1),
        visible_scopes: Observation {
            value: vec!["round9-project".into()],
            observed_at: d(10),
        },
        capabilities: Observation {
            value: vec!["codex".into()],
            observed_at: d(10),
        },
        account_pool: Observation {
            value: "pool".into(),
            observed_at: d(10),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: d(1),
                    budget_remaining: d(10),
                },
            },
            observed_at: d(10),
        },
        current_pressure: Observation {
            value: Known::Known { value: d(1) },
            observed_at: d(10),
        },
        spool_free: Observation {
            value: Known::Known { value: d(10) },
            observed_at: d(10),
        },
        existing_active_roots: vec![],
        controller_connected: true,
        locality_refs: vec!["src".into()],
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(d(10)),
        ownership_revision: Some(d(1)),
    }
}

#[test]
fn p24_p29_round9_item3_telegram_bridge_migration_qualification() {
    // P04/P07: scripted faults and fake time have exact occurrence and boundary behavior.
    let mut plan = FaultPlan::new(vec![FaultRule {
        point: "telegram-delivery".into(),
        occurrence: 1,
        effect: FaultEffect::Drop,
    }])
    .expect("bounded deterministic fault schedule");
    assert_eq!(plan.hit("telegram-delivery"), Ok(Some(FaultEffect::Drop)));
    assert_eq!(plan.hit("telegram-delivery"), Ok(None));

    let mut clock = FakeClock::new(10);
    let deadline = clock.deadline_after_ms(5).expect("bounded deadline");
    assert!(
        clock.advance_to_ms(9).is_err(),
        "backward clock movement is rejected"
    );
    assert_eq!(clock.now_ms(), 10, "rejected clock skew is atomic");
    clock.advance_ms(5).expect("forward fake time");
    assert!(deadline.is_due(clock.now_ms()), "deadline equality is due");

    // P18: an uncertain pre-mutation bridge reply stays NotApplied on exact retry.
    let key = ForgeKey {
        repository: "fixture/telegram-bridge".into(),
        target: "refs/heads/migration".into(),
    };
    let mut forge = ForgeSimulator::default();
    let old_head = "a".repeat(40);
    let new_head = "b".repeat(40);
    forge
        .seed_ref(key.clone(), old_head.clone())
        .expect("seed bridge ref");
    let request = ForgeRequest {
        id: "round9-migration".into(),
        key: key.clone(),
        expected_head: old_head.clone(),
        new_head,
    };
    assert_eq!(
        forge.submit(request.clone(), Fault::LoseReplyBeforeMutation),
        Ok(ForgeReply::OutcomeUnknown)
    );
    assert_eq!(
        forge.submit(request, Fault::None),
        Ok(ForgeReply::Observed(ForgeOutcome::NotApplied))
    );
    assert_eq!(forge.head(&key), Some(old_head.as_str()));
    assert_eq!(forge.applied_mutations(), 0);

    // P21: a completed provider transcript without complete capture cannot qualify.
    let mut parser =
        CodexNdjsonStream::new("round9-model", CodexEffort::Low).expect("explicit fixture model");
    parser
        .push_chunk(&OutputChunk {
            offset: 0,
            stream: OutputStream::Stdout,
            bytes: TURN.to_vec(),
        })
        .expect("deterministic provider fixture");
    let summary = parser.finish(Some(0), false);
    assert_eq!(summary.termination, CodexTermination::Unknown);
    assert!(!summary.trusted_acceptance());
    assert!(!summary.native_execution_qualified());

    // P24: stale ownership evidence at the age boundary yields no placement candidate.
    let policy = PlacementPolicy {
        max_observation_age: d(10),
        maximum_pressure: d(5),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(1)),
    };
    let proposal = propose_placement(
        &round9_placement_request(),
        &[round9_placement_offer()],
        &policy,
        21,
    )
    .expect("stale ownership is a safe rejection");
    assert!(proposal.candidates.is_empty());
    assert_eq!(proposal.rejected.len(), 1);
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
    let mut revision_mismatch = round9_placement_offer();
    revision_mismatch.ownership_revision = Some(d(2));
    let mismatched = propose_placement(
        &round9_placement_request(),
        &[revision_mismatch],
        &policy,
        10,
    )
    .expect("ownership revision mismatch is a safe rejection");
    assert!(mismatched.candidates.is_empty());
    assert!(!mismatched.authority_granted);

    // P25: missing recovery fencing blocks resume and external effects.
    let (manifest, observation, _) = recovery_support::valid_recovery_evidence();
    let recovery = plan_recovery(&manifest, &observation, None).expect("bounded recovery plan");
    assert!(!recovery.resume_proposed);
    assert!(!recovery.restore_authorized());
    assert!(!recovery.effects_authorized());

    // P26: integer overflow and unknown provider cost remain explicit edge cases.
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
    let unknown = serde_json::to_value(ObservedAmount::Unknown {
        reason: "bridge supplied no cost".into(),
    })
    .expect("unknown cost serializes");
    assert_eq!(unknown["kind"], "unknown");
}

#[test]
fn p24_p29_round10_item3_workspace_gate_audit() {
    // P29's workspace registry keeps custody observational and scopes ownership
    // to declared paths, even when the Git metadata directory exists.
    let temp = tempfile::tempdir().expect("workspace audit fixture");
    let root = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let workspace = root.join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("source fixture");
    fs::create_dir_all(workspace.join(".git")).expect("Git metadata fixture");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round10-node").expect("node fixture"),
        root_id: RootId::new("round10-root").expect("root fixture"),
        project_id: ProjectId::new("round10-project").expect("project fixture"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let validated = spec.validate().expect("bounded workspace gate");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert_eq!(validated.git_metadata, GitMetadata::Directory);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../outside")));
    assert_eq!(spec.validate().expect("repeat audit"), validated);

    let mut invalid_scope = spec;
    invalid_scope.owned_paths.push(PathBuf::from("../outside"));
    assert!(invalid_scope.validate().is_err());

    // P24's placement decision independently rejects stale ownership evidence;
    // a registered workspace alone cannot grant placement or authority.
    let mut stale_offer = round9_placement_offer();
    stale_offer.ownership_observed_at = Some(d(10));
    let proposal = propose_placement(
        &round9_placement_request(),
        &[stale_offer],
        &PlacementPolicy {
            max_observation_age: d(10),
            maximum_pressure: d(5),
            minimum_spool_reserve: d(2),
            disconnected_continuation_only: true,
            ownership_revision: Some(d(1)),
        },
        21,
    )
    .expect("stale ownership is a safe rejection");
    assert!(proposal.candidates.is_empty());
    assert_eq!(proposal.rejected.len(), 1);
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}

#[test]
fn p24_p29_round11_item4_final_validation_gate() {
    // P29 workspace validation is stable, registered-only, and restricted to
    // declared paths even when the workspace has Git metadata.
    let temp = tempfile::tempdir().expect("final validation fixture");
    let root = fs::canonicalize(temp.path()).expect("canonical fixture root");
    let workspace = root.join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("source fixture");
    fs::create_dir_all(workspace.join(".git")).expect("Git metadata fixture");
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round11-node").expect("node fixture"),
        root_id: RootId::new("round11-root").expect("root fixture"),
        project_id: ProjectId::new("round11-project").expect("project fixture"),
        workspace,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git"), PathBuf::from("secrets")],
    };
    let validated = spec.validate().expect("workspace gate validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert_eq!(validated.git_metadata, GitMetadata::Directory);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../outside")));
    assert_eq!(spec.validate().expect("repeat workspace audit"), validated);

    // P24 admits only fresh, revision-matched placement evidence. A valid
    // placement proposal still grants no execution, publication, or authority.
    let request = round9_placement_request();
    let policy = PlacementPolicy {
        max_observation_age: d(10),
        maximum_pressure: d(5),
        minimum_spool_reserve: d(2),
        disconnected_continuation_only: true,
        ownership_revision: Some(d(1)),
    };
    let fresh = propose_placement(&request, &[round9_placement_offer()], &policy, 10)
        .expect("fresh placement evidence");
    assert_eq!(fresh.candidates.len(), 1);
    assert!(!fresh.execution_granted);
    assert!(!fresh.publication_granted);
    assert!(!fresh.authority_granted);

    let mut stale = round9_placement_offer();
    stale.ownership_observed_at = Some(d(10));
    stale.ownership_revision = Some(d(2));
    let rejected = propose_placement(&request, &[stale], &policy, 21)
        .expect("stale and mismatched evidence is a safe rejection");
    assert!(rejected.candidates.is_empty());
    assert_eq!(rejected.rejected.len(), 1);
    assert!(!rejected.execution_granted);
    assert!(!rejected.publication_granted);
    assert!(!rejected.authority_granted);
}

#[test]
fn p24_p29_round12_item5_redteam_audit_gate() {
    // The receipt is bound to exact log bytes and exact packet scope. Missing,
    // changed, or promoted fields must fail the audit.
    let log = b"round12 deterministic P24/P29 qualification log\n";
    let digest = format!("{:x}", Sha256::digest(log));
    let receipt = serde_json::json!({
        "packet_ids": ["P24", "P29"],
        "status": "passed",
        "log_sha256": digest,
        "execution_granted": false,
        "publication_granted": false,
        "authority_granted": false
    });
    let verify_receipt = |value: &serde_json::Value, bytes: &[u8]| {
        value["packet_ids"] == serde_json::json!(["P24", "P29"])
            && value["status"] == "passed"
            && value["log_sha256"] == format!("{:x}", Sha256::digest(bytes))
            && value["execution_granted"] == false
            && value["publication_granted"] == false
            && value["authority_granted"] == false
    };
    assert!(verify_receipt(&receipt, log));
    assert!(!verify_receipt(&receipt, b"tampered log bytes\n"));

    let mut forged_packet_scope = receipt.clone();
    forged_packet_scope["packet_ids"] = serde_json::json!(["P24", "P30"]);
    assert!(!verify_receipt(&forged_packet_scope, log));
    let mut forged_authority = receipt.clone();
    forged_authority["authority_granted"] = serde_json::Value::Bool(true);
    assert!(!verify_receipt(&forged_authority, log));
    let mut missing_digest = receipt;
    missing_digest
        .as_object_mut()
        .expect("receipt object")
        .remove("log_sha256");
    assert!(!verify_receipt(&missing_digest, log));

    // P29 rejects ownership outside its declared workspace scope.
    let temp = tempfile::tempdir().expect("workspace audit fixture");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(workspace.join("src")).expect("source fixture");
    fs::create_dir_all(workspace.join(".git")).expect("Git metadata fixture");
    let registered = WorkspaceSpec {
        node_id: NodeRef::new("round12-node").expect("node fixture"),
        root_id: RootId::new("round12-root").expect("root fixture"),
        project_id: ProjectId::new("round12-project").expect("project fixture"),
        workspace: fs::canonicalize(&workspace).expect("canonical workspace"),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("registered workspace validates");
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(std::path::Path::new("src/lib.rs")));
    assert!(!registered.owns(std::path::Path::new(".git/config")));
    assert!(!registered.owns(std::path::Path::new("../outside")));

    // P24 unknown capacity remains a safe rejection with no grants.
    let mut unknown_capacity = round9_placement_offer();
    unknown_capacity.capacity.value = Known::Unknown {
        reason: "red-team unknown capacity".into(),
    };
    let proposal = propose_placement(
        &round9_placement_request(),
        &[unknown_capacity],
        &PlacementPolicy {
            max_observation_age: d(10),
            maximum_pressure: d(5),
            minimum_spool_reserve: d(2),
            disconnected_continuation_only: true,
            ownership_revision: Some(d(1)),
        },
        10,
    )
    .expect("unknown capacity is safely rejected");
    assert!(proposal.candidates.is_empty());
    assert!(!proposal.execution_granted);
    assert!(!proposal.publication_granted);
    assert!(!proposal.authority_granted);
}
