//! Offline P19 checks for the public, pure publication proposal/reconciliation API.
//! These tests do not qualify a forge adapter or grant an external write.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationBlock,
        PublicationError, PublicationIntent, PublicationScope, PublicationSnapshot,
        ReconciliationReceipt, propose_publication, publication_request_digest,
        reduce_publication_intent,
    },
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEXT_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn fixtures() -> (PublicationScope, PublicationSnapshot, DecimalU64) {
    let generation = DecimalU64::new(9);
    let scope = PublicationScope {
        project_id: ProjectId::new("project-p19").expect("project fixture"),
        root_id: RootId::new("root-p19").expect("root fixture"),
        task_id: TaskId::new("task-p19").expect("task fixture"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(HEAD.into()),
        current_base: Some(BASE.into()),
        expected_head: Some(HEAD.into()),
        expected_base: Some(BASE.into()),
        required_reviewers: vec!["maintainer".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "maintainer".into(),
            head: HEAD.into(),
            dismissed: false,
            provenance_ref: "fixture:approval:1".into(),
        }],
        required_checks: vec!["ci".into()],
        checks: vec![CheckEvidence {
            check_id: "ci".into(),
            head: HEAD.into(),
            state: CheckState::Passed,
            provenance_ref: "fixture:check:1".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    (scope, snapshot, generation)
}

fn proposal(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    key: &str,
    generation: &DecimalU64,
) -> PublicationIntent {
    let digest = publication_request_digest(scope, snapshot, key, generation)
        .expect("bounded fixture digest");
    let result = propose_publication(scope, snapshot, key, &digest, generation);
    assert!(
        result.eligible,
        "valid fixture proposal: {:?}",
        result.reasons
    );
    assert!(!result.execution_granted());
    result.intent.expect("eligible proposal includes an intent")
}

fn blocked_with(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    key: &str,
    generation: &DecimalU64,
    expected: PublicationBlock,
) {
    let digest = publication_request_digest(scope, snapshot, key, generation)
        .expect("bounded fixture digest");
    let result = propose_publication(scope, snapshot, key, &digest, generation);
    assert!(!result.eligible);
    assert!(!result.execution_granted());
    assert!(result.intent.is_none());
    assert!(
        result.reasons.contains(&expected),
        "reasons: {:?}",
        result.reasons
    );
}

#[test]
fn wrong_or_dismissed_reviewers_cannot_propose_publication() {
    let (scope, mut snapshot, generation) = fixtures();
    snapshot.approvals[0].reviewer = "unconfigured-reviewer".into();
    blocked_with(
        &scope,
        &snapshot,
        "wrong-reviewer",
        &generation,
        PublicationBlock::WrongReviewer,
    );

    let (scope, mut snapshot, generation) = fixtures();
    snapshot.approvals[0].dismissed = true;
    blocked_with(
        &scope,
        &snapshot,
        "dismissed-review",
        &generation,
        PublicationBlock::DismissedApproval,
    );
}

#[test]
fn changed_head_and_pending_check_cannot_propose_publication() {
    let (scope, mut snapshot, generation) = fixtures();
    snapshot.current_head = Some(NEXT_HEAD.into());
    blocked_with(
        &scope,
        &snapshot,
        "new-head",
        &generation,
        PublicationBlock::ChangedHead,
    );

    let (scope, mut snapshot, generation) = fixtures();
    snapshot.checks[0].state = CheckState::Pending;
    blocked_with(
        &scope,
        &snapshot,
        "pending-check",
        &generation,
        PublicationBlock::CheckNotPassed,
    );
}

#[test]
fn stale_authority_generation_is_blocked_and_valid_proposals_grant_no_execution() {
    let (scope, snapshot, _) = fixtures();
    let stale_generation = DecimalU64::new(8);
    blocked_with(
        &scope,
        &snapshot,
        "stale-generation",
        &stale_generation,
        PublicationBlock::StaleAuthorityGeneration,
    );

    let current_generation = DecimalU64::new(9);
    let digest = publication_request_digest(&scope, &snapshot, "valid", &current_generation)
        .expect("bounded fixture digest");
    let result = propose_publication(&scope, &snapshot, "valid", &digest, &current_generation);
    assert!(result.eligible);
    assert!(!result.execution_granted());
    assert!(result.intent.is_some());
}

#[test]
fn stale_digest_cannot_authorize_mutated_admission_evidence() {
    let (scope, mut snapshot, generation) = fixtures();
    let digest = publication_request_digest(&scope, &snapshot, "mutated-evidence", &generation)
        .expect("bounded fixture digest");

    // A worker cannot change the evidence after deriving the request digest and keep
    // the earlier digest as authority for the altered publication proposal.
    snapshot.approvals[0].provenance_ref = "fixture:substituted-approval".into();
    let result = propose_publication(&scope, &snapshot, "mutated-evidence", &digest, &generation);

    assert!(!result.eligible);
    assert!(!result.execution_granted());
    assert!(result.intent.is_none());
    assert!(result.reasons.contains(&PublicationBlock::InvalidEvidence));
}

#[test]
fn owner_only_publication_rejects_a_non_owner_caller() {
    let (scope, mut snapshot, generation) = fixtures();
    snapshot.owner_only = true;
    snapshot.caller_is_owner = false;

    blocked_with(
        &scope,
        &snapshot,
        "non-owner-caller",
        &generation,
        PublicationBlock::OwnerOnly,
    );
}

#[test]
fn malformed_target_ref_is_rejected_at_admission_boundary() {
    let (mut scope, snapshot, generation) = fixtures();
    scope.target_ref = "refs/heads/p19..candidate".into();

    blocked_with(
        &scope,
        &snapshot,
        "malformed-target-ref",
        &generation,
        PublicationBlock::InvalidEvidence,
    );
}

#[test]
fn noncanonical_authority_generation_is_rejected_on_json_input() {
    assert!(serde_json::from_str::<DecimalU64>("\"09\"").is_err());
}

#[test]
fn ambiguous_effect_slot_requires_matching_reconciliation_and_replays_once() {
    let (scope, snapshot, generation) = fixtures();
    let proposed = proposal(&scope, &snapshot, "intent-one", &generation);
    let staged = reduce_publication_intent(None, &proposed, None)
        .expect("proposal enters the local intent log");
    let mut claimed = staged.clone();
    claimed.stage = IntentStage::Claimed;
    let claimed =
        reduce_publication_intent(Some(&staged), &claimed, None).expect("claim is recorded");
    let mut ambiguous = claimed.clone();
    ambiguous.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous, None)
        .expect("uncertain outcome holds the effect slot");

    let second = proposal(&scope, &snapshot, "intent-two", &generation);
    assert_eq!(
        reduce_publication_intent(Some(&ambiguous), &second, None),
        Err(PublicationError::EffectSlotOccupied)
    );
    assert_eq!(
        reduce_publication_intent(Some(&ambiguous), &ambiguous, None)
            .expect("ambiguous replay is idempotent"),
        ambiguous
    );

    let receipt = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: NEXT_HEAD.into(),
        effect_occurred: true,
        provenance_ref: "fixture:reconciliation:1".into(),
    };
    let completed = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt))
        .expect("matching receipt resolves the ambiguous outcome");
    assert_eq!(completed.stage, IntentStage::ReconciledSuccess);
    assert_eq!(
        reduce_publication_intent(Some(&completed), &completed, Some(&receipt))
            .expect("receipt replay is idempotent"),
        completed
    );

    let mut conflicting_receipt = receipt.clone();
    conflicting_receipt.provenance_ref = "fixture:reconciliation:alternate".into();
    assert_eq!(
        reduce_publication_intent(Some(&completed), &completed, Some(&conflicting_receipt)),
        Err(PublicationError::ReceiptCollision)
    );

    let mut reopened = completed.clone();
    reopened.stage = IntentStage::Ambiguous;
    assert_eq!(
        reduce_publication_intent(Some(&completed), &reopened, None),
        Err(PublicationError::InvalidReconciliation)
    );
}
