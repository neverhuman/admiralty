//! Round 13 P19 work-order and release-evidence boundary qualification.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, PublicationScope,
        PublicationSnapshot, propose_publication, publication_request_digest,
    },
};
use admiralty_runtime::recovery::{Blocker, SessionCustody, plan_recovery};

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn p19_round13_wo_merge_install_running_release_evidence() {
    // A work-order publication proposal binds one exact task, candidate, target,
    // review and check set. It records source admission only; it cannot certify
    // that a package was installed, a session is running it, or a release occurred.
    let head = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(13);
    let scope = PublicationScope {
        project_id: ProjectId::new("round13-workorder").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("wo-merge-install-running").expect("task ID"),
        target_ref: "refs/heads/main".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(head.clone()),
        current_base: Some(head.clone()),
        expected_head: Some(head.clone()),
        expected_base: Some(head.clone()),
        required_reviewers: vec!["maintainer".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "maintainer".into(),
            head: head.clone(),
            dismissed: false,
            provenance_ref: "fixture:round13:review".into(),
        }],
        required_checks: vec!["release-candidate-check".into()],
        checks: vec![CheckEvidence {
            check_id: "release-candidate-check".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: "fixture:round13:check".into(),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation)
        .expect("publication request digest");
    let proposal = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(
        proposal.eligible,
        "evidence-bound proposal: {:?}",
        proposal.reasons
    );
    assert!(!proposal.execution_granted());
    let intent = proposal.intent.expect("eligible proposal carries intent");
    assert_eq!(intent.scope, scope);
    assert_eq!(intent.expected_head, head);
    assert_eq!(
        intent.admission_snapshot.expected_head.as_deref(),
        Some(head.as_str())
    );
    assert_ne!(
        candidate, head,
        "candidate identity is distinct from current worktree head"
    );

    // Release continuity independently needs exact installed bundle observations,
    // a closed backup artifact set, known compatibility and reconciled session custody.
    // A publication result cannot fill any of those evidence fields by implication.
    let (manifest, mut observation, fence) = recovery_fixture::valid_recovery_evidence();
    observation.observed_bundle_digests.clear();
    observation.session_custody = SessionCustody::Unknown;
    let blocked = plan_recovery(&manifest, &observation, Some(&fence)).expect("recovery decision");
    assert!(!blocked.resume_proposed);
    assert!(!blocked.restore_authorized());
    assert!(!blocked.effects_authorized());
    assert!(blocked.blockers.contains(&Blocker::UnknownCompatibility));
    assert!(blocked.blockers.contains(&Blocker::UnknownSessionCustody));

    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let reconciled =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("recovery decision");
    assert!(reconciled.resume_proposed);
    assert!(!reconciled.restore_authorized());
    assert!(!reconciled.effects_authorized());
}
