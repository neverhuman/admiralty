//! Round 11 final P19/P21 validation. Provider facts and forge effects are simulated.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationScope,
        PublicationSnapshot, ReconciliationReceipt, propose_publication,
        publication_request_digest, reduce_publication_intent,
    },
};
use admiralty_runtime::providers::claude::{TurnOutcome, UsageSource, parse_stream};
use admiralty_testkit::forge::{
    Fault as ForgeFault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
};

#[test]
fn p19_round11_item4_final_validation_gate() {
    // P21 observation: a complete successful Claude turn remains untrusted.
    let claude_bytes = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");
    let claude = parse_stream(claude_bytes, Some("fixture-session"))
        .expect("deterministic complete provider fixture");
    assert_eq!(claude.outcome, TurnOutcome::Succeeded);
    assert_eq!(claude.usage_source, UsageSource::ParentTerminal);
    assert!(!claude.trusted_acceptance());
    assert!(!claude.native_execution_qualified());

    // P19 binds review, checks, base/head and authority to a frozen proposal.
    let base = "a".repeat(40);
    let candidate = "c".repeat(40);
    let generation = DecimalU64::new(11);
    let scope = PublicationScope {
        project_id: ProjectId::new("round11-final-validation").expect("project"),
        root_id: RootId::new("round11-root").expect("root"),
        task_id: TaskId::new("claude-publication").expect("task"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["workspace-owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "workspace-owner".into(),
            head: base.clone(),
            dismissed: false,
            provenance_ref: "fixture:round11:review".into(),
        }],
        required_checks: vec!["workspace-gate".into()],
        checks: vec![CheckEvidence {
            check_id: "workspace-gate".into(),
            head: base.clone(),
            state: CheckState::Passed,
            provenance_ref: "fixture:round11:gate-audit".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let request_digest = publication_request_digest(&scope, &snapshot, &candidate, &generation)
        .expect("canonical publication request digest");
    let proposal = propose_publication(&scope, &snapshot, &candidate, &request_digest, &generation);
    assert!(
        proposal.eligible,
        "reviewed exact snapshot: {:?}",
        proposal.reasons
    );
    assert!(!proposal.execution_granted());
    let proposed = proposal.intent.expect("eligible request freezes an intent");
    assert_eq!(proposed.stage, IntentStage::Proposed);
    assert_eq!(proposed.expected_head, base);
    assert_eq!(proposed.request_digest, request_digest);

    // Replay-safe journal transitions precede the simulated external effect.
    let staged = reduce_publication_intent(None, &proposed, None).expect("stage intent");
    let mut claimed_input = staged.clone();
    claimed_input.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed_input, None)
        .expect("claim intent exactly once");
    let mut ambiguous_input = claimed.clone();
    ambiguous_input.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous_input, None)
        .expect("retain uncertain effect state");

    let key = ForgeKey {
        repository: "fixture/round11".into(),
        target: scope.target_ref.clone(),
    };
    let request = ForgeRequest {
        id: ambiguous.intent_key.clone(),
        key: key.clone(),
        expected_head: base.clone(),
        new_head: candidate.clone(),
    };
    let mut forge = ForgeSimulator::new(1, 1).expect("bounded simulated forge");
    forge
        .seed_ref(key.clone(), base.clone())
        .expect("seed exact target head");
    let lost_reply = forge
        .submit(request.clone(), ForgeFault::LoseReplyAfterMutation)
        .expect("record deterministic simulated mutation");
    assert_eq!(lost_reply, ForgeReply::OutcomeUnknown);
    assert!(!lost_reply.execution_granted());
    assert_eq!(forge.head(&key), Some(candidate.as_str()));
    assert_eq!(forge.applied_mutations(), 1);

    let observation = forge
        .reconcile(&request.id)
        .expect("independent read observes the immutable request outcome")
        .clone();
    assert_eq!(
        observation,
        ForgeOutcome::Applied {
            previous_head: base.clone(),
            new_head: candidate.clone(),
        }
    );
    let mut receipt = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: base.clone(),
        effect_occurred: true,
        provenance_ref: "fixture:round11:independent-probe".into(),
    };
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt)).is_err());

    receipt.observed_head = candidate.clone();
    let completed = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt))
        .expect("matching independent reconciliation closes the intent");
    assert_eq!(completed.stage, IntentStage::ReconciledSuccess);
    assert_eq!(completed.effect_receipt, Some(receipt.clone()));

    // Exact replay returns the original observation without a second mutation;
    // durable intent replay is idempotent as well.
    assert_eq!(
        forge.submit(request, ForgeFault::None),
        Ok(ForgeReply::Observed(observation))
    );
    assert_eq!(forge.applied_mutations(), 1);
    assert_eq!(
        reduce_publication_intent(Some(&completed), &completed, Some(&receipt)),
        Ok(completed)
    );
}
