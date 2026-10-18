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
fn p19_round22_canary_smoke_release_package_qualification() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let store = ArtifactStore::open(&root, 2048).unwrap();
    let bundle = b"round22:p19:canary-release-package:v1";

    let artifact = store.publish_bytes(bundle).unwrap();
    let expected_hash = format!("sha256:{:x}", Sha256::digest(bundle));
    assert_eq!(
        artifact.sha256, expected_hash,
        "package digest matches bundle bytes"
    );
    assert_eq!(artifact.size_bytes, bundle.len() as u64);
    for _ in 0..16 {
        assert_eq!(store.publish_bytes(bundle).unwrap(), artifact);
        assert_eq!(store.read_verified(&artifact).unwrap(), bundle);
    }

    let base = "e".repeat(40);
    let candidate = "f".repeat(40);
    let generation = DecimalU64::new(22);
    let scope = PublicationScope {
        project_id: ProjectId::new("round22-p19").unwrap(),
        root_id: RootId::new("canary-release-root").unwrap(),
        task_id: TaskId::new("package-publication").unwrap(),
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
            provenance_ref: "review:round22:independent".into(),
        }],
        required_checks: vec!["release-package-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "release-package-integrity".into(),
            head: base.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{expected_hash}"),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let request_digest =
        publication_request_digest(&scope, &snapshot, &candidate, &generation).unwrap();
    let first = propose_publication(&scope, &snapshot, &candidate, &request_digest, &generation);
    assert!(first.eligible, "canary evidence: {:?}", first.reasons);
    assert!(!first.execution_granted());
    for _ in 0..16 {
        let replay =
            propose_publication(&scope, &snapshot, &candidate, &request_digest, &generation);
        assert_eq!(replay.intent, first.intent);
        assert!(!replay.execution_granted());
    }

    let mut missing_provenance = snapshot;
    missing_provenance.checks[0].provenance_ref.clear();
    let negative_digest =
        publication_request_digest(&scope, &missing_provenance, &candidate, &generation).unwrap();
    let rejected = propose_publication(
        &scope,
        &missing_provenance,
        &candidate,
        &negative_digest,
        &generation,
    );
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    let object = root.join(artifact.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object, b"tampered canary package").unwrap();
    assert!(store.read_verified(&artifact).is_err());
    assert!(store.publish_bytes(bundle).is_err());
}
