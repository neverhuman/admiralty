use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationScope,
        PublicationSnapshot, ReconciliationReceipt, propose_publication,
        publication_request_digest, reduce_publication_intent,
    },
};
use admiralty_store::artifacts::ArtifactStore;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use admiralty_testkit::forge::{
    Fault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
};

#[test]
fn codex_round14_p19() {
    // Repeated publication of the same content has one stable provenance digest.
    let Ok(directory) = tempfile::tempdir() else {
        panic!("create temporary artifact fixture");
    };
    let permissions = std::fs::Permissions::from_mode(0o700);
    if let Err(error) = std::fs::set_permissions(directory.path(), permissions) {
        panic!("make artifact root private: {error}");
    }
    let root = match std::fs::canonicalize(directory.path()) {
        Ok(root) => root,
        Err(error) => panic!("canonicalize artifact root: {error}"),
    };
    let store = match ArtifactStore::open(&root, 4096) {
        Ok(store) => store,
        Err(error) => panic!("open artifact store: {error}"),
    };
    let bytes = b"round14 immutable candidate bundle";
    let reference = match store.publish_bytes(bytes) {
        Ok(reference) => reference,
        Err(error) => panic!("publish candidate bundle: {error}"),
    };
    assert_eq!(store.publish_bytes(bytes).ok(), Some(reference.clone()));
    assert_eq!(
        store.read_verified(&reference).ok().as_deref(),
        Some(bytes.as_slice())
    );

    // Corrupt bytes under a valid content-addressed name fail closed on read/replay.
    let object = root.join(reference.sha256.trim_start_matches("sha256:"));
    if let Err(error) = std::fs::write(&object, b"corrupt bundle bytes") {
        panic!("tamper fixture object: {error}");
    }
    assert!(store.read_verified(&reference).is_err());
    assert!(store.publish_bytes(bytes).is_err());

    // P19 publication is deterministic over exact scope, head, review and check
    // provenance. Missing provenance cannot produce an intent.
    let head = "a".repeat(40);
    let candidate = "b".repeat(40);
    let generation = DecimalU64::new(14);
    let scope = PublicationScope {
        project_id: ProjectId::new("round14-p19").unwrap_or_else(|e| panic!("project: {e}")),
        root_id: RootId::new("release-root").unwrap_or_else(|e| panic!("root: {e}")),
        task_id: TaskId::new("artifact-publication").unwrap_or_else(|e| panic!("task: {e}")),
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
            provenance_ref: "artifact:review:sha256:review".into(),
        }],
        required_checks: vec!["bundle-integrity".into()],
        checks: vec![CheckEvidence {
            check_id: "bundle-integrity".into(),
            head: head.clone(),
            state: CheckState::Passed,
            provenance_ref: format!("artifact:bundle:{}", reference.sha256),
        }],
        owner_only: false,
        caller_is_owner: false,
        unresolved_intent: false,
    };
    let digest = publication_request_digest(&scope, &snapshot, &candidate, &generation)
        .unwrap_or_else(|e| panic!("digest request: {e}"));
    let first = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    let replay = propose_publication(&scope, &snapshot, &candidate, &digest, &generation);
    assert!(first.eligible);
    assert!(!first.execution_granted());
    assert_eq!(first.intent, replay.intent);

    let mut unproven = snapshot.clone();
    unproven.checks[0].provenance_ref.clear();
    let changed_digest = publication_request_digest(&scope, &unproven, &candidate, &generation)
        .unwrap_or_else(|e| panic!("digest changed request: {e}"));
    let rejected = propose_publication(&scope, &unproven, &candidate, &changed_digest, &generation);
    assert!(!rejected.eligible);
    assert!(rejected.intent.is_none());
    assert!(!rejected.execution_granted());

    // Simulate a lost publication reply. Reconciliation is tied to exact evidence;
    // replay returns the recorded outcome and does not mutate a second time.
    let Some(proposed) = first.intent else {
        panic!("eligible proposal carries intent");
    };
    let staged =
        reduce_publication_intent(None, &proposed, None).unwrap_or_else(|e| panic!("stage: {e}"));
    let mut claimed_input = staged.clone();
    claimed_input.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed_input, None)
        .unwrap_or_else(|e| panic!("claim: {e}"));
    let mut ambiguous_input = claimed.clone();
    ambiguous_input.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous_input, None)
        .unwrap_or_else(|e| panic!("ambiguous: {e}"));
    let key = ForgeKey {
        repository: "fixture/round14".into(),
        target: scope.target_ref.clone(),
    };
    let request = ForgeRequest {
        id: ambiguous.intent_key.clone(),
        key: key.clone(),
        expected_head: head.clone(),
        new_head: candidate.clone(),
    };
    let mut forge = ForgeSimulator::new(1, 1).unwrap_or_else(|e| panic!("forge: {e}"));
    forge
        .seed_ref(key.clone(), head.clone())
        .unwrap_or_else(|e| panic!("seed: {e}"));
    assert_eq!(
        forge.submit(request.clone(), Fault::LoseReplyAfterMutation),
        Ok(ForgeReply::OutcomeUnknown)
    );
    let mut bad_receipt = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: head.clone(),
        effect_occurred: true,
        provenance_ref: "fixture:independent-read".into(),
    };
    assert!(reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&bad_receipt)).is_err());
    bad_receipt.observed_head = candidate.clone();
    let done = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&bad_receipt))
        .unwrap_or_else(|e| panic!("reconcile exact observed head: {e}"));
    assert_eq!(done.stage, IntentStage::ReconciledSuccess);
    assert_eq!(
        forge.reconcile(&request.id),
        Some(&ForgeOutcome::Applied {
            previous_head: head,
            new_head: candidate,
        })
    );
    assert_eq!(
        forge.submit(request, Fault::None),
        Ok(ForgeReply::Observed(ForgeOutcome::Applied {
            previous_head: "a".repeat(40),
            new_head: "b".repeat(40),
        }))
    );
    assert_eq!(forge.applied_mutations(), 1);
}
