//! Round 12 red-team qualification for receipt integrity and fail-closed publication.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationScope,
        PublicationSnapshot, ReconciliationReceipt, propose_publication,
        publication_request_digest, reduce_publication_intent,
    },
    verification::{MAX_RECEIPT_BYTES, MAX_RETAINED_RECEIPTS, ReceiptRegistry, parse_receipt},
};

#[test]
fn p19_round12_item5_redteam_audit_gate() {
    // P19 receipt parser rejects claims, unknown fields, invalid bytes, trailing data,
    // and oversized evidence before handing untrusted material to callers.
    for forged in [
        br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"v","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#.as_slice(),
        br#"{"unknown":true}"#.as_slice(),
        br#"{} trailing"#.as_slice(),
        &[0xff][..],
    ] {
        assert!(parse_receipt(forged).is_err(), "forged input accepted: {forged:?}");
        assert!(parse_receipt(forged).is_err(), "replaying bad bytes remains rejected");
    }
    assert!(parse_receipt(&vec![b' '; MAX_RECEIPT_BYTES + 1]).is_err());
    assert!(ReceiptRegistry::new(0).is_err());
    assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS + 1).is_err());

    // P19 only proposes from exact, current evidence. A stale review or failed check
    // closes admission, and an eligible proposal alone never grants execution.
    let base = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(12);
    let scope = PublicationScope {
        project_id: ProjectId::new("round12-redteam").unwrap(),
        root_id: RootId::new("redteam-root").unwrap(),
        task_id: TaskId::new("receipt-audit").unwrap(),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let mut snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "owner".into(),
            head: base.clone(),
            dismissed: false,
            provenance_ref: "fixture:round12:review".into(),
        }],
        required_checks: vec!["gate".into()],
        checks: vec![CheckEvidence {
            check_id: "gate".into(),
            head: base.clone(),
            state: CheckState::Passed,
            provenance_ref: "fixture:round12:check".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation).unwrap();
    let proposal = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(proposal.eligible);
    assert!(!proposal.execution_granted());
    let intent = proposal.intent.unwrap();

    let staged = reduce_publication_intent(None, &intent, None).unwrap();
    let mut claimed_input = staged.clone();
    claimed_input.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed_input, None).unwrap();
    let mut ambiguous_input = claimed.clone();
    ambiguous_input.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous_input, None).unwrap();

    // Wrong digest, scope, head, or effect claim cannot reconcile ambiguity.
    let mut receipt = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: candidate.clone(),
        effect_occurred: true,
        provenance_ref: "fixture:round12:independent-read".into(),
    };
    receipt.request_digest.push('0');
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt)).is_err());
    receipt.request_digest = ambiguous.request_digest.clone();
    receipt.scope.target_ref.push_str("-forged");
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt)).is_err());
    receipt.scope = ambiguous.scope.clone();
    receipt.observed_head = base.clone();
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt)).is_err());

    snapshot.approvals[0].head = "c".repeat(40);
    let changed_digest =
        publication_request_digest(&scope, &snapshot, &candidate, &generation).unwrap();
    let stale_review =
        propose_publication(&scope, &snapshot, &candidate, &changed_digest, &generation);
    assert!(!stale_review.eligible, "stale approval must fail closed");
    assert!(stale_review.intent.is_none());
}
