use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationScope,
        PublicationSnapshot, ReconciliationReceipt, propose_publication,
        publication_request_digest, reduce_publication_intent,
    },
};
use admiralty_store::artifacts::ArtifactStore;
use admiralty_testkit::forge::{
    Fault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn p19_round15_release_smoke_multi_step_stress_qualification() {
    // Content-addressed bundle publication is idempotent over repeated retries.
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("private artifact fixture: {error}"));
    let root = std::fs::canonicalize(directory.path())
        .unwrap_or_else(|error| panic!("canonical artifact fixture: {error}"));
    let store = ArtifactStore::open(&root, 4096)
        .unwrap_or_else(|error| panic!("open artifact store: {error}"));
    let bundle = b"round15 deterministic release bundle v1";
    let reference = store
        .publish_bytes(bundle)
        .unwrap_or_else(|error| panic!("publish bundle: {error}"));
    for _ in 0..64 {
        let replayed = store
            .publish_bytes(bundle)
            .unwrap_or_else(|error| panic!("replay bundle publication: {error}"));
        assert_eq!(
            replayed, reference,
            "same bytes retain the same content address and receipt"
        );
        assert_eq!(
            store.read_verified(&reference).ok().as_deref(),
            Some(bundle.as_slice())
        );
    }
    let oversized_bundle = [b'x'; 4097];
    assert!(store.publish_bytes(&oversized_bundle).is_err());

    // Corruption at the expected object address is detected and never repaired by
    // a retry, preserving fail-closed bundle integrity.
    let object = root.join(reference.sha256.trim_start_matches("sha256:"));
    std::fs::write(&object, b"tampered release bundle bytes")
        .unwrap_or_else(|error| panic!("tamper bundle fixture: {error}"));
    assert!(store.read_verified(&reference).is_err());
    assert!(store.publish_bytes(bundle).is_err());

    // The P19 publication digest binds exact work scope and provenance-bearing
    // review/check observations. Identical requests replay deterministically.
    let head = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(15);
    let scope = PublicationScope {
        project_id: ProjectId::new("round15-p19")
            .unwrap_or_else(|error| panic!("project: {error}")),
        root_id: RootId::new("release-smoke-root").unwrap_or_else(|error| panic!("root: {error}")),
        task_id: TaskId::new("bundle-release-smoke")
            .unwrap_or_else(|error| panic!("task: {error}")),
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
            provenance_ref: "review:sha256:round15-fixture".into(),
        }],
        required_checks: vec!["bundle-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "bundle-integrity".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:{}", reference.sha256),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation)
        .unwrap_or_else(|error| panic!("publication digest: {error}"));
    let first = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(
        first.eligible,
        "valid release smoke evidence: {:?}",
        first.reasons
    );
    assert!(!first.execution_granted());
    for _ in 0..64 {
        let replay = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
        assert_eq!(replay.intent, first.intent);
        assert!(!replay.execution_granted());
    }

    let mut missing_provenance = snapshot.clone();
    missing_provenance.checks[0].provenance_ref.clear();
    let missing_digest =
        publication_request_digest(&scope, &missing_provenance, &candidate, &generation)
            .unwrap_or_else(|error| panic!("negative-path digest: {error}"));
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

    // Multiple durable transitions plus a lost external reply reconcile only
    // against an independent observation matching the immutable publication.
    let Some(proposed) = first.intent.clone() else {
        panic!("eligible proposal contains intent");
    };
    let staged = reduce_publication_intent(None, &proposed, None)
        .unwrap_or_else(|error| panic!("stage intent: {error}"));
    let mut claim_input = staged.clone();
    claim_input.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claim_input, None)
        .unwrap_or_else(|error| panic!("claim intent: {error}"));
    let mut uncertain_input = claimed.clone();
    uncertain_input.stage = IntentStage::Ambiguous;
    let uncertain = reduce_publication_intent(Some(&claimed), &uncertain_input, None)
        .unwrap_or_else(|error| panic!("record ambiguous effect: {error}"));

    let key = ForgeKey {
        repository: "fixture/round15".into(),
        target: scope.target_ref.clone(),
    };
    let request = ForgeRequest {
        id: uncertain.intent_key.clone(),
        key: key.clone(),
        expected_head: head.clone(),
        new_head: candidate.clone(),
    };
    let mut forge = ForgeSimulator::new(1, 1).unwrap_or_else(|error| panic!("forge: {error}"));
    forge
        .seed_ref(key.clone(), head.clone())
        .unwrap_or_else(|error| panic!("seed target: {error}"));
    assert_eq!(
        forge.submit(request.clone(), Fault::LoseReplyAfterMutation),
        Ok(ForgeReply::OutcomeUnknown)
    );

    let receipt = |observed_head: String| ReconciliationReceipt {
        intent_key: uncertain.intent_key.clone(),
        request_digest: uncertain.request_digest.clone(),
        scope: uncertain.scope.clone(),
        expected_head: uncertain.expected_head.clone(),
        expected_base: uncertain.expected_base.clone(),
        authority_generation: uncertain.authority_generation.clone(),
        observed_head,
        effect_occurred: true,
        provenance_ref: "independent:round15:target-read".into(),
    };
    assert!(
        reduce_publication_intent(Some(&uncertain), &uncertain, Some(&receipt(head.clone())))
            .is_err()
    );
    let reconciliation = receipt(candidate.clone());
    let reconciled = reduce_publication_intent(Some(&uncertain), &uncertain, Some(&reconciliation))
        .unwrap_or_else(|error| panic!("reconcile exact effect: {error}"));
    assert_eq!(reconciled.stage, IntentStage::ReconciledSuccess);
    assert_eq!(
        forge.reconcile(&request.id),
        Some(&ForgeOutcome::Applied {
            previous_head: head.clone(),
            new_head: candidate.clone(),
        })
    );
    for _ in 0..64 {
        assert_eq!(
            forge.submit(request.clone(), Fault::None),
            Ok(ForgeReply::Observed(ForgeOutcome::Applied {
                previous_head: head.clone(),
                new_head: candidate.clone(),
            }))
        );
    }
    assert_eq!(forge.applied_mutations(), 1);
}
