//! P19 pure contracts and opt-in durable gateway qualification. All forge effects
//! are local fixtures; the GitHub adapter never contacts a live repository here.

use admiralty_core::{
    ProjectId, RootId, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationBlock,
        PublicationError, PublicationIntent, PublicationScope, PublicationSnapshot,
        ReconciliationReceipt, propose_publication, publication_request_digest,
        reduce_publication_intent,
    },
};
use admiralty_testkit::forge::{
    Fault as ForgeFault, ForgeKey, ForgeOutcome, ForgeReply, ForgeRequest, ForgeSimulator,
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
fn injected_lost_forge_reply_reconciles_and_replays_without_second_mutation() {
    let (scope, snapshot, generation) = fixtures();
    let proposed = proposal(&scope, &snapshot, "faulted-publication", &generation);
    let staged = reduce_publication_intent(None, &proposed, None).expect("stage proposal");
    let mut claimed = staged.clone();
    claimed.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed, None).expect("claim intent");
    let mut ambiguous = claimed.clone();
    ambiguous.stage = IntentStage::Ambiguous;
    let ambiguous =
        reduce_publication_intent(Some(&claimed), &ambiguous, None).expect("record uncertainty");

    let key = ForgeKey {
        repository: "fixture/p19".into(),
        target: scope.target_ref.clone(),
    };
    let request = ForgeRequest {
        id: ambiguous.intent_key.clone(),
        key: key.clone(),
        expected_head: ambiguous.expected_head.clone(),
        new_head: NEXT_HEAD.into(),
    };
    let mut forge = ForgeSimulator::new(1, 1).expect("bounded forge fixture");
    forge
        .seed_ref(key.clone(), HEAD.into())
        .expect("seed initial target");

    let lost_reply = forge
        .submit(request.clone(), ForgeFault::LoseReplyAfterMutation)
        .expect("fault records a simulated request");
    assert_eq!(lost_reply, ForgeReply::OutcomeUnknown);
    assert!(!lost_reply.execution_granted());
    assert_eq!(forge.head(&key), Some(NEXT_HEAD));
    assert_eq!(forge.applied_mutations(), 1);

    let observation = forge
        .reconcile(&request.id)
        .expect("independent simulated probe finds recorded outcome")
        .clone();
    assert_eq!(
        observation,
        ForgeOutcome::Applied {
            previous_head: HEAD.into(),
            new_head: NEXT_HEAD.into(),
        }
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
        provenance_ref: "fixture:forge-probe:p19".into(),
    };
    let completed = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&receipt))
        .expect("independent observation resolves ambiguous intent");
    assert_eq!(completed.stage, IntentStage::ReconciledSuccess);

    assert_eq!(
        forge.submit(request, ForgeFault::None),
        Ok(ForgeReply::Observed(observation))
    );
    assert_eq!(forge.applied_mutations(), 1);
    assert_eq!(
        reduce_publication_intent(Some(&completed), &completed, Some(&receipt)),
        Ok(completed)
    );
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

#[cfg(unix)]
mod durable_gateway {
    use super::*;
    use admiralty_runtime::integration::{
        publisher::{
            EffectObservation, Forge, ForgeObservation, GatewayPolicy, PublicationGateway,
            PublicationRequest,
        },
        reconcile::GitHubForge,
    };
    use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, path::PathBuf, rc::Rc};

    #[derive(Clone)]
    struct LocalForge(Rc<RefCell<LocalState>>);
    struct LocalState {
        observation: ForgeObservation,
        root: PathBuf,
        writes: usize,
        reads: usize,
        change_at_boundary: bool,
        effect: bool,
        probe_available: bool,
    }
    impl Forge for LocalForge {
        fn observe(&mut self, _: &str, _: u64) -> Result<ForgeObservation, String> {
            let mut state = self.0.borrow_mut();
            state.reads += 1;
            if state.change_at_boundary && state.reads == 2 {
                state.observation.checks[0].state = CheckState::Pending;
            }
            Ok(state.observation.clone())
        }
        fn merge_rebase(&mut self, _: &str, _: &PublicationRequest) -> Result<(), String> {
            let mut state = self.0.borrow_mut();
            let journal: serde_json::Value = serde_json::from_slice(
                &fs::read(state.root.join("journal.json")).expect("journal durable before call"),
            )
            .expect("journal JSON");
            assert_eq!(journal["entries"].as_array().expect("entries").len(), 1);
            assert_eq!(journal["entries"][0]["intent"]["stage"], "ambiguous");
            state.writes += 1;
            Err("deterministic lost reply, possibly after a remote effect".into())
        }
        fn reconcile(
            &mut self,
            _: &str,
            _: &PublicationRequest,
        ) -> Result<EffectObservation, String> {
            let state = self.0.borrow();
            if !state.probe_available {
                return Ok(EffectObservation::Unknown);
            }
            if state.effect {
                Ok(EffectObservation::Applied {
                    head: state.observation.head.clone(),
                    commit: NEXT_HEAD.into(),
                    provenance: "fixture:independent-request-probe".into(),
                })
            } else {
                Ok(EffectObservation::NoEffect {
                    provenance: "fixture:quiesced-request-never-applied".into(),
                })
            }
        }
    }
    fn private_tempdir() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("private fixture directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("private fixture mode");
        directory
    }

    fn setup() -> (
        tempfile::TempDir,
        GatewayPolicy,
        PublicationRequest,
        LocalForge,
    ) {
        let root = private_tempdir();
        let (scope, snapshot, generation) = fixtures();
        let policy = GatewayPolicy {
            project_id: scope.project_id.clone(),
            repository: "fixture/disposable".into(),
            target_ref: scope.target_ref.clone(),
            authority_generation: generation,
            required_reviewers: vec!["maintainer".into()],
            required_checks: vec!["ci".into()],
            owner_only: false,
            owners: vec!["owner".into()],
        };
        let request = PublicationRequest {
            intent_key: "durable-request".into(),
            pull_request: 7,
            scope,
            expected_head: HEAD.into(),
            expected_base: BASE.into(),
        };
        let forge = LocalForge(Rc::new(RefCell::new(LocalState {
            observation: ForgeObservation {
                repository: policy.repository.clone(),
                target_ref: policy.target_ref.clone(),
                pull_request: 7,
                head: HEAD.into(),
                base: BASE.into(),
                open: true,
                approvals: snapshot.approvals,
                checks: snapshot.checks,
            },
            root: root.path().to_owned(),
            writes: 0,
            reads: 0,
            change_at_boundary: false,
            effect: true,
            probe_available: false,
        })));
        (root, policy, request, forge)
    }

    #[test]
    fn durable_target_slot_survives_lost_reply_restart_and_generation_rotation() {
        let (root, mut policy, request, forge) = setup();
        let mut gateway =
            PublicationGateway::open(root.path(), policy.clone(), forge.clone()).expect("gateway");
        assert!(
            PublicationGateway::open(root.path(), policy.clone(), forge.clone()).is_err(),
            "second publisher excluded"
        );
        let initial = gateway
            .publish(&request, "worker")
            .expect("ambiguous effect recorded");
        assert_eq!(initial.intent.stage, IntentStage::Ambiguous);
        assert_eq!(forge.0.borrow().writes, 1);
        assert_eq!(gateway.publish(&request, "worker").expect("retry"), initial);
        let mut competing = request.clone();
        competing.intent_key = "competing".into();
        competing.pull_request = 8;
        assert!(gateway.publish(&competing, "worker").is_err());
        policy.authority_generation = DecimalU64::new(10);
        gateway
            .rotate_policy(policy.clone())
            .expect("revoke old generation");
        competing.scope.authority_generation = policy.authority_generation.clone();
        assert!(
            gateway.publish(&competing, "worker").is_err(),
            "generation cannot clear occupied slot"
        );
        drop(gateway);
        let mut restored = PublicationGateway::open(root.path(), policy, forge.clone())
            .expect("restart retains target custody");
        assert_eq!(
            restored
                .publish(&request, "worker")
                .expect("replay without write"),
            initial
        );
        assert_eq!(forge.0.borrow().writes, 1);
        forge.0.borrow_mut().probe_available = true;
        let completed = restored
            .reconcile(&request.intent_key)
            .expect("independent probe");
        assert_eq!(completed.intent.stage, IntentStage::ReconciledSuccess);
        assert_eq!(
            restored
                .reconcile(&request.intent_key)
                .expect("receipt replay"),
            completed
        );
        let mut rebound = request.clone();
        rebound.pull_request = 99;
        assert!(
            restored.publish(&rebound, "worker").is_err(),
            "key cannot rebind another PR"
        );
        assert_eq!(forge.0.borrow().writes, 1);
    }

    #[test]
    fn durable_gateway_rejects_every_stale_admission_before_external_effect() {
        for case in 0..9 {
            let (root, mut policy, mut request, forge) = setup();
            match case {
                0 => forge.0.borrow_mut().observation.approvals[0].reviewer = "stranger".into(),
                1 => forge.0.borrow_mut().observation.approvals[0].dismissed = true,
                2 => forge.0.borrow_mut().observation.head = NEXT_HEAD.into(),
                3 => forge.0.borrow_mut().observation.base = NEXT_HEAD.into(),
                4 => forge.0.borrow_mut().observation.checks[0].state = CheckState::Pending,
                5 => request.scope.authority_generation = DecimalU64::new(8),
                6 => policy.owner_only = true,
                7 => forge.0.borrow_mut().observation.repository = "foreign/repo".into(),
                8 => forge.0.borrow_mut().change_at_boundary = true,
                _ => unreachable!(),
            }
            let mut gateway =
                PublicationGateway::open(root.path(), policy, forge.clone()).expect("gateway");
            assert!(gateway.publish(&request, "worker").is_err(), "case {case}");
            assert_eq!(forge.0.borrow().writes, 0, "case {case}");
            assert!(
                gateway.entries().is_empty(),
                "case {case}: denied requests reserve no slot"
            );
        }
    }

    #[test]
    fn definitive_no_effect_releases_slot_but_unknown_does_not() {
        let (root, policy, request, forge) = setup();
        let mut gateway =
            PublicationGateway::open(root.path(), policy, forge.clone()).expect("gateway");
        gateway.publish(&request, "worker").expect("uncertain");
        assert_eq!(
            gateway
                .reconcile(&request.intent_key)
                .expect("unknown")
                .intent
                .stage,
            IntentStage::Ambiguous
        );
        {
            let mut state = forge.0.borrow_mut();
            state.effect = false;
            state.probe_available = true;
        }
        assert_eq!(
            gateway
                .reconcile(&request.intent_key)
                .expect("definitive no effect")
                .intent
                .stage,
            IntentStage::ReconciledNoEffect
        );
        let mut next = request;
        next.intent_key = "next".into();
        assert!(
            gateway
                .assess(&next, "worker")
                .expect("released slot")
                .eligible
        );
    }

    #[test]
    fn mismatched_probe_corrupt_journal_and_public_directory_fail_closed() {
        let (root, policy, request, forge) = setup();
        let mut gateway =
            PublicationGateway::open(root.path(), policy.clone(), forge.clone()).expect("gateway");
        gateway.publish(&request, "worker").expect("uncertain");
        {
            let mut state = forge.0.borrow_mut();
            state.probe_available = true;
            state.observation.head = BASE.into();
        }
        assert_eq!(
            gateway
                .reconcile(&request.intent_key)
                .expect("foreign effect ignored")
                .intent
                .stage,
            IntentStage::Ambiguous
        );
        drop(gateway);
        let journal = root.path().join("journal.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal).expect("journal")).expect("json");
        value["entries"][0]["request"]["expected_head"] = serde_json::json!(NEXT_HEAD);
        fs::write(&journal, serde_json::to_vec(&value).expect("serialize"))
            .expect("corrupt fixture");
        assert!(PublicationGateway::open(root.path(), policy.clone(), forge.clone()).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755))
            .expect("public fixture");
        assert!(PublicationGateway::open(root.path(), policy, forge).is_err());
    }

    fn github_fixture(root: &std::path::Path) -> GitHubForge {
        let pull = serde_json::json!({ "number":7, "state":"open", "merged":false, "draft":false,
            "head":{"sha":HEAD}, "base":{"sha":BASE,"ref":"main","repo":{"full_name":"fixture/disposable"}} });
        let mut merged = pull.clone();
        merged["merged"] = serde_json::json!(true);
        merged["state"] = serde_json::json!("closed");
        merged["merge_commit_sha"] = serde_json::json!(NEXT_HEAD);
        merged["merged_at"] = serde_json::json!("2026-10-19T14:00:00Z");
        let reviews = serde_json::json!([{ "id":42,"state":"APPROVED","user":{"login":"maintainer"},"commit_id":HEAD }]);
        let runs = serde_json::json!({"total_count":1,"check_runs":[{"id":11,"name":"ci","app":{"id":99},"head_sha":HEAD,"status":"completed","conclusion":"success"}]});
        let statuses = serde_json::json!({"sha":HEAD,"total_count":0,"statuses":[]});
        for (name, value) in [
            ("pull.json", pull),
            ("merged.json", merged),
            ("reviews.json", reviews),
            ("runs.json", runs),
            ("statuses.json", statuses),
        ] {
            fs::write(
                root.join(name),
                serde_json::to_vec(&value).expect("fixture JSON"),
            )
            .expect("fixture write");
        }
        let executable = root.join("fixture-gh");
        fs::write(&executable, r#"#!/bin/sh
set -eu
[ "$HOME" = "$GH_CONFIG_DIR" ]
[ "$GH_HOST" = github.com ]
[ -z "${GH_TOKEN-}${GITHUB_TOKEN-}${CARGO_TARGET_DIR-}${RUSTUP_HOME-}" ]
printf '%s\n' "$*" >> calls.txt
case "$*" in
  *pulls/7/reviews*) cat reviews.json ;;
  *check-runs*) cat runs.json ;;
  *commits/*/status*) cat statuses.json ;;
  *pulls/7) cat pull.json ;;
  'pr merge 7 --repo fixture/disposable --rebase --match-head-commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')
    cp merged.json pull.json
    exit 1 ;; # success occurred but the reply was lost
  *) exit 90 ;;
esac
"#).expect("fixture executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).expect("fixture mode");
        GitHubForge::new(executable, root.to_owned()).expect("isolated fixture adapter")
    }

    #[test]
    fn github_adapter_uses_isolated_credentials_exact_head_rebase_and_independent_probe() {
        let (root, mut policy, request, _) = setup();
        let credentials = private_tempdir();
        let forge = github_fixture(credentials.path());
        policy.required_checks = vec!["check-run:99:ci".into()];
        let mut gateway = PublicationGateway::open(root.path(), policy, forge).expect("gateway");
        let entry = gateway
            .publish(&request, "worker")
            .expect("lost reply independently reconciled");
        assert_eq!(entry.intent.stage, IntentStage::ReconciledSuccess);
        assert_eq!(
            entry.intent.effect_receipt.expect("receipt").observed_head,
            NEXT_HEAD
        );
        gateway
            .publish(&request, "worker")
            .expect("idempotent replay");
        let calls = fs::read_to_string(credentials.path().join("calls.txt")).expect("calls");
        assert_eq!(
            calls.lines().filter(|l| l.starts_with("pr merge ")).count(),
            1
        );
        assert!(!calls.contains("--admin"));
    }

    #[test]
    fn github_adapter_keeps_open_pr_ambiguous_and_latest_dismissal_blocks() {
        let (_, _, request, _) = setup();
        let credentials = private_tempdir();
        let mut forge = github_fixture(credentials.path());
        assert!(matches!(
            forge
                .reconcile("fixture/disposable", &request)
                .expect("open PR"),
            EffectObservation::Unknown
        ));
        let reviews = serde_json::json!([
            {"id":43,"state":"DISMISSED","user":{"login":"maintainer"},"commit_id":HEAD},
            {"id":42,"state":"APPROVED","user":{"login":"maintainer"},"commit_id":HEAD}
        ]);
        fs::write(
            credentials.path().join("reviews.json"),
            serde_json::to_vec(&reviews).expect("reviews"),
        )
        .expect("reviews fixture");
        let observation = forge
            .observe("fixture/disposable", 7)
            .expect("fresh observation");
        assert!(
            observation.approvals[0].dismissed,
            "latest review ID wins even out of order"
        );
        let mut moved = request;
        moved.expected_base = NEXT_HEAD.into();
        assert!(forge.merge_rebase("fixture/disposable", &moved).is_err());
        let calls = fs::read_to_string(credentials.path().join("calls.txt")).expect("calls");
        assert!(!calls.lines().any(|l| l.starts_with("pr merge ")));
    }
}
