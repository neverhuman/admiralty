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
fn p19_round21_post_item6_hermetic_cross_packet_release_qualification() {
    // Two independent, hermetic stores must derive the same address from the
    // same deterministic bundle bytes and return identical verified content.
    let first_directory = tempfile::tempdir().unwrap();
    let second_directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    for directory in [&first_directory, &second_directory] {
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let first_root = std::fs::canonicalize(first_directory.path()).unwrap();
    let second_root = std::fs::canonicalize(second_directory.path()).unwrap();
    let first_store = ArtifactStore::open(&first_root, 1024).unwrap();
    let second_store = ArtifactStore::open(&second_root, 1024).unwrap();
    let bundle = b"round21:p19:hermetic-release-bundle:v1";
    let first_ref = first_store.publish_bytes(bundle).unwrap();
    let second_ref = second_store.publish_bytes(bundle).unwrap();
    let expected_hash = format!("sha256:{:x}", Sha256::digest(bundle));
    assert_eq!(first_ref.sha256, expected_hash);
    assert_eq!(first_ref, second_ref);
    assert_eq!(first_store.read_verified(&first_ref).unwrap(), bundle);
    assert_eq!(second_store.read_verified(&second_ref).unwrap(), bundle);
    assert_eq!(first_store.publish_bytes(bundle).unwrap(), first_ref);

    let head = "c".repeat(40);
    let candidate = "d".repeat(40);
    let generation = DecimalU64::new(21);
    let scope = PublicationScope {
        project_id: ProjectId::new("round21-p19").unwrap(),
        root_id: RootId::new("hermetic-release-root").unwrap(),
        task_id: TaskId::new("cross-packet-publication").unwrap(),
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
            provenance_ref: "review:round21:independent".into(),
        }],
        required_checks: vec!["bundle-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "bundle-integrity".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{}", expected_hash),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };

    // Schema serialization and request identity are stable across independent
    // encode/decode and retry paths; the artifact hash remains bound as evidence.
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    let decoded: PublicationSnapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    let digest = publication_request_digest(&scope, &decoded, &candidate, &generation).unwrap();
    let replay_digest =
        publication_request_digest(&scope, &snapshot, &candidate, &generation).unwrap();
    assert_eq!(digest, replay_digest);
    let proposal = propose_publication(&scope, &decoded, &candidate, &digest, &generation);
    assert!(
        proposal.eligible,
        "verified release evidence: {:?}",
        proposal.reasons
    );
    assert!(!proposal.execution_granted());

    let mut missing_provenance = decoded;
    missing_provenance.checks[0].provenance_ref.clear();
    let missing_digest =
        publication_request_digest(&scope, &missing_provenance, &candidate, &generation).unwrap();
    let rejected = propose_publication(
        &scope,
        &missing_provenance,
        &candidate,
        &missing_digest,
        &generation,
    );
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    let object = first_root.join(first_ref.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object, b"tampered hermetic bundle").unwrap();
    assert!(first_store.read_verified(&first_ref).is_err());
    assert!(first_store.publish_bytes(bundle).is_err());
}
