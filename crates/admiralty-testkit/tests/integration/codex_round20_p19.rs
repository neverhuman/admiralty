#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, PublicationScope,
        PublicationSnapshot, propose_publication, publication_request_digest,
    },
};
use admiralty_store::artifacts::ArtifactStore;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn p19_round20_post_pr57_release_qualification_strict_clippy_audit() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let store = ArtifactStore::open(&root, 512).unwrap();
    let bundle = b"round20:P19:release:provenance-qualified";

    let artifact = store.publish_bytes(bundle).unwrap();
    let expected_hash = format!("sha256:{:x}", Sha256::digest(bundle));
    assert_eq!(artifact.sha256, expected_hash);
    assert_eq!(artifact.size_bytes, bundle.len() as u64);
    assert_eq!(store.read_verified(&artifact).unwrap(), bundle);
    assert_eq!(store.publish_bytes(bundle).unwrap(), artifact);
    assert!(store.publish_bytes(&[b'x'; 513]).is_err());

    let head = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(20);
    let scope = PublicationScope {
        project_id: ProjectId::new("round20-p19").unwrap(),
        root_id: RootId::new("post-pr57-release").unwrap(),
        task_id: TaskId::new("bundle-publication").unwrap(),
        target_ref: "refs/heads/release".into(),
        authority_generation: generation.clone(),
    };
    let snapshot = PublicationSnapshot {
        current_head: Some(head.clone()),
        current_base: Some(head.clone()),
        expected_head: Some(head.clone()),
        expected_base: Some(head.clone()),
        required_reviewers: vec!["release-owner".into()],
        approvals: vec![ApprovalEvidence {
            reviewer: "release-owner".into(),
            head: head.clone(),
            dismissed: false,
            provenance_ref: "review:round20:independent".into(),
        }],
        required_checks: vec!["release-bundle".into()],
        checks: vec![CheckEvidence {
            check_id: "release-bundle".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{}", artifact.sha256),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation).unwrap();
    let proposal = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(
        proposal.eligible,
        "valid artifact evidence: {:?}",
        proposal.reasons
    );
    assert!(!proposal.execution_granted());

    let mut unproven = snapshot.clone();
    unproven.checks[0].provenance_ref.clear();
    let unproven_digest =
        publication_request_digest(&scope, &unproven, &candidate, &generation).unwrap();
    let rejected =
        propose_publication(&scope, &unproven, &candidate, &unproven_digest, &generation);
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    let object = root.join(artifact.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object, b"tampered release content").unwrap();
    assert!(store.read_verified(&artifact).is_err());
    assert!(store.publish_bytes(bundle).is_err());
}
