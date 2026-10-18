//! Round 16 end-to-end cutover and fault-containment qualification.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ReadinessBlocker, ScheduledTask, SchedulerError,
        SchedulerSnapshot, SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, StoreError, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;

#[test]
fn codex_round16_p06_p13_p14() {
    let mut prerequisite = task_fixture("ROUND16-PREREQUISITE");
    prerequisite.objective = "Complete checkpoint one without granting an effect".into();
    let mut followup = task_fixture("ROUND16-FOLLOWUP");
    followup.dependencies = vec![prerequisite.task_id.clone()];
    followup.objective = "Proceed only after checkpoint one is observed completed".into();
    let project_id = prerequisite.project_id.clone();
    let prerequisite_id = prerequisite.task_id.clone();
    let followup_id = followup.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round16-cutover-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![followup, prerequisite],
    };
    let source = serde_json::to_vec(&plan).expect("serialize bounded plan");
    let preview = preview_workplan(&source).expect("preview decomposed plan");
    assert_eq!(
        preview.task_order,
        [prerequisite_id.clone(), followup_id.clone()]
    );

    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round16-cutover-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder.tempdir().expect("create private fixture root");
    let store = Store::open(root.path().join("round16.sqlite")).expect("open durable store");
    let request = RegisterWorkplanRequest {
        command_id: "round16-cutover-register".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("digest plan"),
        source,
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("persist initial checkpoint");
    assert_eq!(
        receipt,
        store
            .register_workplan(request.clone())
            .expect("same command replay returns exact receipt")
    );

    let mut collision_plan = plan.clone();
    collision_plan.tasks[0]
        .objective
        .push_str(" with changed content");
    let collision_source = serde_json::to_vec(&collision_plan).expect("serialize changed plan");
    let collision_preview = preview_workplan(&collision_source).expect("valid changed plan");
    let mut collision = request.clone();
    collision.source = collision_source;
    collision.source_digest = collision_preview.source_digest;
    collision.canonical_digest = content_digest(&collision_plan).expect("digest changed plan");
    assert!(matches!(
        store.register_workplan(collision),
        Err(StoreError::Workplan(
            admiralty_store::workplans::WorkplanStoreError::IdentityCollision
        ))
    ));

    let persisted = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read checkpoint after collision")
        .expect("original revision remains available");
    assert_eq!(persisted.plan, plan);
    let prerequisite_contract = persisted
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == prerequisite_id)
        .expect("prerequisite contract");
    let followup_contract = persisted
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == followup_id)
        .expect("follow-up contract");
    let prerequisite_capsule = compile_task_capsule(prerequisite_contract, "round16-epoch", 4096)
        .expect("compile first checkpoint context");
    let followup_capsule = compile_task_capsule(followup_contract, "round16-epoch", 4096)
        .expect("compile follow-up checkpoint context");
    assert!(compile_task_capsule(followup_contract, "round16-epoch", 1).is_err());
    assert!(compile_task_capsule(followup_contract, " ", 4096).is_err());

    let scope = SchedulingScope {
        project_id,
        root_id: prerequisite_contract.root_id.clone(),
        revision: 1,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![
            ScheduledTask {
                contract: prerequisite_contract.clone(),
                contract_digest: prerequisite_capsule.contract_digest,
                observed_contract_revision: prerequisite_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
            ScheduledTask {
                contract: followup_contract.clone(),
                contract_digest: followup_capsule.contract_digest,
                observed_contract_revision: followup_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
        ],
        available_quota: Knowledge::Known(2),
        available_capacity: Knowledge::Known(2),
    };
    let initial = propose_ready_tasks(&snapshot, &scope).expect("propose first checkpoint");
    assert_eq!(
        initial.ready.as_slice(),
        std::slice::from_ref(&prerequisite_id)
    );
    assert!(matches!(
        initial
            .blocked
            .get(&followup_id)
            .and_then(|reasons| reasons.first()),
        Some(ReadinessBlocker::DependencyNotCompleted { dependency, .. })
            if dependency == &prerequisite_id
    ));
    assert!(!initial.execution_granted);

    let mut completed = snapshot.clone();
    completed.tasks[0].state = TaskSchedulingState::Completed;
    let cutover = propose_ready_tasks(&completed, &scope).expect("propose next checkpoint");
    assert_eq!(cutover.ready.as_slice(), std::slice::from_ref(&followup_id));
    assert!(!cutover.execution_granted);
    assert_eq!(
        cutover,
        propose_ready_tasks(&completed, &scope).expect("deterministic cutover replay")
    );

    let mut bad_digest = completed.clone();
    bad_digest.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&bad_digest, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == followup_id.as_str()
    ));
    let mut stale = scope.clone();
    stale.revision = 2;
    assert!(matches!(
        propose_ready_tasks(&completed, &stale),
        Err(SchedulerError::ScopeMismatch)
    ));
    let mut unknown = completed;
    unknown.available_quota = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&unknown, &scope).expect("unknown quota fails closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);
    assert!(
        store
            .events_after(0, 10)
            .expect("no execution events")
            .is_empty()
    );
}
