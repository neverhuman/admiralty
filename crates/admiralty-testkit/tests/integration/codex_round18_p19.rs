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
fn p19_round18_end_to_end_release_bundle_schema_parity_qualification() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    #[cfg(unix)]
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("private artifact fixture: {error}"));
    let root = std::fs::canonicalize(directory.path())
        .unwrap_or_else(|error| panic!("canonical artifact root: {error}"));
    let store = ArtifactStore::open(&root, 1024)
        .unwrap_or_else(|error| panic!("open artifact store: {error}"));

    let bundle = b"round18:P19:release-bundle:schema-parity:v1";
    let artifact = store
        .publish_bytes(bundle)
        .unwrap_or_else(|error| panic!("publish release bundle: {error}"));
    let repeated = store
        .publish_bytes(bundle)
        .unwrap_or_else(|error| panic!("replay release bundle: {error}"));
    assert_eq!(
        artifact, repeated,
        "release retries retain content identity"
    );
    assert_eq!(artifact.size_bytes, bundle.len() as u64);
    let expected_digest = format!("sha256:{:x}", Sha256::digest(bundle));
    assert_eq!(
        artifact.sha256, expected_digest,
        "reference hashes published bytes"
    );
    assert_eq!(
        store.read_verified(&artifact).ok().as_deref(),
        Some(bundle.as_slice())
    );

    let head = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(18);
    let scope = PublicationScope {
        project_id: ProjectId::new("round18-p19")
            .unwrap_or_else(|error| panic!("project id: {error}")),
        root_id: RootId::new("release-root").unwrap_or_else(|error| panic!("root id: {error}")),
        task_id: TaskId::new("release-bundle").unwrap_or_else(|error| panic!("task id: {error}")),
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
            provenance_ref: "review:round18:independent".into(),
        }],
        required_checks: vec!["published-bundle-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "published-bundle-integrity".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{}", artifact.sha256),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    assert_eq!(
        snapshot.checks[0].provenance_ref,
        format!("artifact:{}", expected_digest),
        "publication provenance identifies the verified bundle digest"
    );

    // The serialized contract retains its declared field set, snake-case enum,
    // and canonical decimal string representation across a serde round trip.
    let encoded = serde_json::to_value(&snapshot)
        .unwrap_or_else(|error| panic!("serialize P19 snapshot: {error}"));
    let object = encoded
        .as_object()
        .unwrap_or_else(|| panic!("snapshot serializes as an object"));
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "approvals",
            "caller_is_owner",
            "checks",
            "current_base",
            "current_head",
            "expected_base",
            "expected_head",
            "owner_only",
            "required_checks",
            "required_reviewers",
            "unresolved_intent",
        ]
    );
    assert_eq!(
        encoded["checks"][0]["state"],
        serde_json::Value::String("passed".into())
    );
    let restored: PublicationSnapshot = serde_json::from_value(encoded.clone())
        .unwrap_or_else(|error| panic!("deserialize P19 snapshot: {error}"));
    assert_eq!(restored, snapshot);
    let mut unknown_field = encoded;
    unknown_field["unexpected"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<PublicationSnapshot>(unknown_field).is_err());

    let digest = publication_request_digest(&scope, &restored, &candidate, &generation)
        .unwrap_or_else(|error| panic!("publication request digest: {error}"));
    let proposal = propose_publication(&scope, &restored, &candidate, &digest, &generation);
    assert!(proposal.eligible, "valid evidence: {:?}", proposal.reasons);
    assert!(!proposal.execution_granted());

    let mut absent_bundle_provenance = restored;
    absent_bundle_provenance.checks[0].provenance_ref.clear();
    let negative_digest =
        publication_request_digest(&scope, &absent_bundle_provenance, &candidate, &generation)
            .unwrap_or_else(|error| panic!("negative-path digest: {error}"));
    let rejected = propose_publication(
        &scope,
        &absent_bundle_provenance,
        &candidate,
        &negative_digest,
        &generation,
    );
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    let object_path = root.join(artifact.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object_path, b"altered bundle")
        .unwrap_or_else(|error| panic!("tamper bundle fixture: {error}"));
    assert!(store.read_verified(&artifact).is_err());
    assert!(store.publish_bytes(bundle).is_err());
}
