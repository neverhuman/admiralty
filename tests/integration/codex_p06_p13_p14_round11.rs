//! Round 11 P06/P13/P14 end-to-end final validation gate.

use admiralty_core::{
    ProjectId, TaskId, content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ScheduledTask, SchedulerSnapshot, SchedulingScope,
        TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;

fn private_root() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round11-validation-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private validation root")
}

#[test]
fn p06_p13_p14_round11_item4_final_validation_gate() {
    let root = private_root();
    let database = root.path().join("round11.sqlite");
    let mut contract = task_fixture("ROUND11-P06-P13-P14");
    contract.objective = "Preserve exact scope; do not launch or accept from a proposal".into();
    contract.non_goals = vec!["Do not publish".into()];
    let project_id = contract.project_id.clone();
    let task_id = contract.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round11-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract.clone()],
    };
    let source = serde_json::to_vec(&plan).expect("serialize fixed plan");
    let preview = preview_workplan(&source).expect("validate canonical workplan");
    let request = RegisterWorkplanRequest {
        command_id: "round11-register-v1".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest.clone(),
        canonical_digest: content_digest(&plan).expect("canonical plan digest"),
        source,
    };

    let store = Store::open(&database).expect("open durable workplan store");
    let receipt = store
        .register_workplan(request.clone())
        .expect("register immutable workplan revision");
    assert_eq!(
        store
            .register_workplan(request)
            .expect("exact registry replay"),
        receipt
    );
    let receipt_json = serde_json::to_value(&receipt).expect("serialize registration receipt");
    assert_eq!(receipt_json["recorded_only"], true);
    assert_eq!(receipt_json["task_events_emitted"], false);
    assert_eq!(receipt_json["execution_granted"], false);
    assert_eq!(receipt_json["acceptance_granted"], false);
    assert!(store.events_after(0, 10).expect("no task event").is_empty());
    drop(store);

    let reopened = Store::open(&database).expect("reopen durable workplan registry");
    let registered = reopened
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read registered revision")
        .expect("revision persists after reopen");
    assert_eq!(registered.plan, plan);
    assert_eq!(registered.receipt, receipt);

    let registered_contract = &registered.plan.tasks[0];
    let capsule = compile_task_capsule(registered_contract, "round11-epoch", 4096)
        .expect("P06 compiles complete bounded task context");
    assert_eq!(capsule.task_id, task_id.to_string());
    assert_eq!(capsule.contract_digest, preview.task_digests[&task_id]);
    assert!(
        capsule
            .text
            .contains("do not launch or accept from a proposal")
    );
    assert!(capsule.text.contains("Do not publish"));

    let scope = SchedulingScope {
        project_id,
        root_id: registered_contract.root_id.clone(),
        revision: 1,
    };
    let scheduled = ScheduledTask {
        observed_contract_revision: registered_contract.revision,
        contract: registered_contract.clone(),
        contract_digest: capsule.contract_digest.clone(),
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
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("P14 readiness proposal");
    assert_eq!(proposal.ready, [task_id]);
    assert!(!proposal.execution_granted);
    assert_eq!(
        proposal,
        propose_ready_tasks(&snapshot, &scope).expect("deterministic replay")
    );
    assert!(
        reopened
            .events_after(0, 10)
            .expect("proposal emits no events")
            .is_empty()
    );
}
