use admiralty_core::{
    AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId,
    publication::{
        ApprovalEvidence, CheckEvidence, CheckState, DecimalU64, IntentStage, PublicationBlock,
        PublicationScope, PublicationSnapshot, ReconciliationReceipt, propose_publication,
        publication_request_digest, reduce_publication_intent,
    },
    workplans::{Workplan, preview_workplan},
};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEXT: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn release_fixture() -> (PublicationScope, PublicationSnapshot, DecimalU64) {
    let generation = DecimalU64::new(3);
    (
        PublicationScope {
            project_id: ProjectId::new("round13-project").unwrap(),
            root_id: RootId::new("round13-root").unwrap(),
            task_id: TaskId::new("round13-task").unwrap(),
            target_ref: "refs/heads/release".into(),
            authority_generation: generation.clone(),
        },
        PublicationSnapshot {
            current_head: Some(HEAD.into()),
            current_base: Some(BASE.into()),
            expected_head: Some(HEAD.into()),
            expected_base: Some(BASE.into()),
            required_reviewers: vec!["release-owner".into()],
            approvals: vec![ApprovalEvidence {
                reviewer: "release-owner".into(),
                head: HEAD.into(),
                dismissed: false,
                provenance_ref: "fixture:review:round13".into(),
            }],
            required_checks: vec!["release-qualification".into()],
            checks: vec![CheckEvidence {
                check_id: "release-qualification".into(),
                head: HEAD.into(),
                state: CheckState::Passed,
                provenance_ref: "fixture:check:round13".into(),
            }],
            owner_only: false,
            caller_is_owner: false,
            unresolved_intent: false,
        },
        generation,
    )
}

#[test]
fn p26_p31_round13_wo_cutover_release_evidence() {
    // Work-order preview is deterministic and read-only; task identity is
    // bound to the declared project and dependency order.
    let project = ProjectId::new("round13-project").unwrap();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round13-release-work-order".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![TaskContract {
            schema_version: 1,
            project_id: project.clone(),
            root_id: RootId::new("round13-root").unwrap(),
            task_id: TaskId::new("round13-task").unwrap(),
            revision: 1,
            title: "Qualify release evidence".into(),
            objective: "Require independent approval and passing release checks".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![".git".into()],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "release-evidence".into(),
                text: "Approval and check evidence match the exact head".into(),
                check_ids: vec!["release-qualification".into()],
            }],
            checks: vec![CheckSpec {
                id: "release-qualification".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 60,
            }],
            authority_ref: "fixture:round13".into(),
            max_attempts: 1,
            simulated: true,
        }],
    };
    let source = serde_json::to_vec(&plan).unwrap();
    let first = preview_workplan(&source).unwrap();
    assert_eq!(first, preview_workplan(&source).unwrap());
    assert_eq!(first.task_order, vec![TaskId::new("round13-task").unwrap()]);

    // Release cutover requires matching, undismissed approval and passing
    // check provenance on the expected head; the proposal alone authorizes no effect.
    let (scope, snapshot, generation) = release_fixture();
    let key = "round13-intent";
    let digest = publication_request_digest(&scope, &snapshot, key, &generation).unwrap();
    let proposal = propose_publication(&scope, &snapshot, key, &digest, &generation);
    assert!(proposal.eligible);
    assert!(!proposal.execution_granted());
    let proposed = proposal.intent.unwrap();
    let staged = reduce_publication_intent(None, &proposed, None).unwrap();
    let mut claimed = staged.clone();
    claimed.stage = IntentStage::Claimed;
    let claimed = reduce_publication_intent(Some(&staged), &claimed, None).unwrap();
    let mut ambiguous = claimed.clone();
    ambiguous.stage = IntentStage::Ambiguous;
    let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous, None).unwrap();
    assert_eq!(
        reduce_publication_intent(Some(&ambiguous), &ambiguous, None),
        Ok(ambiguous.clone()),
        "uncertain cutover stays held without external evidence"
    );
    let evidence = ReconciliationReceipt {
        intent_key: ambiguous.intent_key.clone(),
        request_digest: ambiguous.request_digest.clone(),
        scope: ambiguous.scope.clone(),
        expected_head: ambiguous.expected_head.clone(),
        expected_base: ambiguous.expected_base.clone(),
        authority_generation: ambiguous.authority_generation.clone(),
        observed_head: NEXT.into(),
        effect_occurred: true,
        provenance_ref: "fixture:independent-release-probe:round13".into(),
    };
    let completed =
        reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&evidence)).unwrap();
    assert_eq!(completed.stage, IntentStage::ReconciledSuccess);
    assert_eq!(
        reduce_publication_intent(Some(&completed), &completed, Some(&evidence)).unwrap(),
        completed
    );

    let (scope, mut blocked, generation) = release_fixture();
    blocked.checks[0].state = CheckState::Unknown;
    let blocked_digest =
        publication_request_digest(&scope, &blocked, "blocked", &generation).unwrap();
    let blocked = propose_publication(&scope, &blocked, "blocked", &blocked_digest, &generation);
    assert!(!blocked.eligible && !blocked.execution_granted());
    assert!(
        blocked
            .reasons
            .contains(&PublicationBlock::MissingOrUnknownCheck)
    );
}
