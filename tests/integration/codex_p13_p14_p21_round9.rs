//! Round 9 P13/P14/P21 workplan-to-provider qualification.

use admiralty_core::scheduler::{
    Knowledge, LeaseObservation, ScheduledTask, SchedulerSnapshot, SchedulingScope,
    TaskSchedulingState, propose_ready_tasks,
};
use admiralty_core::workplans::{Workplan, preview_workplan};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId, content_digest,
};
use admiralty_runtime::providers::claude::{TurnOutcome, parse_stream, qualification};
use serde_json::json;

const CLAUDE_TURN: &[u8] = include_bytes!("../fixtures/providers/claude/continuity-turn.jsonl");

fn task_contract(project_id: ProjectId) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id,
        root_id: RootId::new("round9-root").expect("valid root ID"),
        task_id: TaskId::new("round9-task").expect("valid task ID"),
        revision: 1,
        title: "Round 9 Claude workplan task".into(),
        objective: "Qualify deterministic workplan dispatch observation".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "checked".into(),
            text: "The named deterministic check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["check".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "round9-fixture".into(),
        max_attempts: 1,
        simulated: true,
    }
}

#[test]
fn p13_p14_round9_item3_claude_workplan_dispatch_qualification() {
    let project_id = ProjectId::new("round9-project").expect("valid project ID");
    let contract = task_contract(project_id.clone());
    let workplan = Workplan {
        schema_version: 1,
        plan_id: "round9-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract.clone()],
    };
    let workplan_json = serde_json::to_vec(&workplan).expect("serialize fixed workplan");
    let preview = preview_workplan(&workplan_json).expect("valid canonical workplan preview");
    assert_eq!(preview.task_order, [contract.task_id.clone()]);

    let scope = SchedulingScope {
        project_id,
        root_id: contract.root_id.clone(),
        revision: 1,
    };
    let scheduled = ScheduledTask {
        observed_contract_revision: contract.revision,
        contract: contract.clone(),
        contract_digest: content_digest(&contract).expect("contract digest"),
        state: TaskSchedulingState::Pending,
        lease: LeaseObservation::Absent,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![scheduled],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    };
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("conservative ready proposal");
    assert_eq!(proposal.ready, [contract.task_id]);
    assert!(!proposal.execution_granted);

    let observation = parse_stream(CLAUDE_TURN, Some("fixture-session"))
        .expect("checked-in complete Claude observation");
    assert_eq!(observation.outcome, TurnOutcome::Succeeded);
    assert!(!observation.native_execution_qualified());
    assert!(!observation.trusted_acceptance());
    assert_eq!(qualification()["native_execution_qualified"], json!(false));
    assert_eq!(qualification()["trusted_acceptance"], json!(false));
}
