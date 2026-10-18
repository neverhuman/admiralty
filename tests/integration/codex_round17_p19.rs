//! Round 17 release audit across P19 publication, artifact custody, and provenance.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage,
        PublicationScope, PublicationSnapshot, ReconciliationReceipt,
        propose_publication, publication_request_digest, reduce_publication_intent,
    },
};
use admiralty_store::artifacts::ArtifactStore;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn p19_round17_post_item5_release_audit_cross_packet_invariant_qualification() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    #[cfg(unix)]
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("make private artifact fixture: {error}"));
    let root = std::fs::canonicalize(temp.path())
        .unwrap_or_else(|error| panic!("canonical artifact root: {error}"));
    let store = ArtifactStore::open(&root, 2048)
        .unwrap_or_else(|error| panic!("open artifact store: {error}"));
    let bundle = b"round17:p19:release-bundle:immutable";
    let reference = store
        .publish_bytes(bundle)
        .unwrap_or_else(|error| panic!("publish release bundle: {error}"));
    for _ in 0..32 {
        let replay = store
            .publish_bytes(bundle)
            .unwrap_or_else(|error| panic!("replay release bundle: {error}"));
        assert_eq!(replay, reference, "replay preserves bundle identity");
        assert_eq!(
            store.read_verified(&reference).ok().as_deref(),
            Some(bundle.as_slice()),
            "verified read returns the published bytes"
        );
    }

    let base = "1".repeat(40);
    let candidate = "2".repeat(40);
    let generation = DecimalU64::new(17);
    let scope = PublicationScope {
        project_id: ProjectId::new("round17-release-audit")
            .unwrap_or_else(|error| panic!("project id: {error}")),
        root_id: RootId::new("release-root")
            .unwrap_or_else(|error| panic!("root id: {error}")),
        task_id: TaskId::new("artifact-publication")
            .unwrap_or_else(|error| panic!("task id: {error}")),
        target_ref: "refs/heads/release".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(base.clone()),
        current_base: Some(base.clone()),
        expected_head: Some(base.clone()),
        expected_base: Some(base.clone()),
        required_reviewers: vec!["release-owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "release-owner".into(),
            head: base.clone(),
            dismissed: false,
            provenance_ref: "review:round17:verified".into(),
        }],
        required_checks: vec!["bundle-provenance".into()],
        checks: vec![CheckEvidence {
            check_id: "bundle-provenance".into(),
            head: base.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{}", reference.sha256),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation)
        .unwrap_or_else(|error| panic!("publication digest: {error}"));
    let proposal = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(proposal.eligible, "valid evidence should be eligible: {:?}", proposal.reasons);
    assert!(!proposal.execution_granted(), "proposal is not an external side effect");
    for _ in 0..32 {
        let replay = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
        assert_eq!(replay.intent, proposal.intent, "publication replay is deterministic");
        assert!(!replay.execution_granted());
    }

    let mut altered_provenance = snapshot.clone();
    altered_provenance.checks[0].provenance_ref.push_str(":forged");
    let altered_digest = publication_request_digest(
        &scope,
        &altered_provenance,
        &candidate,
        &generation,
    )
    .unwrap_or_else(|error| panic!("altered-evidence digest: {error}"));
    assert_ne!(digest, altered_digest, "digest binds provenance evidence");
    altered_provenance.checks[0].provenance_ref.clear();
    let missing_digest = publication_request_digest(
        &scope,
        &altered_provenance,
        &candidate,
        &generation,
    )
    .unwrap_or_else(|error| panic!("missing-evidence digest: {error}"));
    let rejected = propose_publication(
        &scope,
        &altered_provenance,
        &candidate,
        &missing_digest,
        &generation,
    );
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    let Some(intent) = proposal.intent else {
        panic!("eligible proposal contains publication intent");
    };
    let staged = reduce_publication_intent(None, &intent, None)
        .unwrap_or_else(|error| panic!("stage publication: {error}"));
    let mut claimed_input = staged.clone();
    claimed_input.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed_input, None)
        .unwrap_or_else(|error| panic!("claim publication: {error}"));
    let mut ambiguous_input = claimed.clone();
    ambiguous_input.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous_input, None)
        .unwrap_or_else(|error| panic!("record uncertain publication: {error}"));
    let receipt = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: candidate.clone(),
        effect_occurred: true,
        provenance_ref: "independent:round17:target-observation".into(),
    };
    let mut forged_receipt = receipt.clone();
    forged_receipt.request_digest.push('0');
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&forged_receipt)).is_err());
    let reconciled = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt))
        .unwrap_or_else(|error| panic!("reconcile observed publication: {error}"));
    assert_eq!(reconciled.stage, IntentStage::ReconciledSuccess);

    // Existing content at the expected address must fail verification and must
    // not be silently repaired by a subsequent publication attempt.
    let object = root.join(reference.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object, b"forged bundle")
        .unwrap_or_else(|error| panic!("tamper bundle fixture: {error}"));
    assert!(store.read_verified(&reference).is_err());
    assert!(store.publish_bytes(bundle).is_err());
}
