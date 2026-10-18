//! Round 15 release smoke and multi-step replay qualification for P06/P13/P14.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ReadinessBlocker, ScheduledTask, SchedulerError,
        SchedulerSnapshot, SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, WorkplanError, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;

#[test]
fn codex_round15_p06_p13_p14() {
    let mut first = task_fixture("ROUND15-CHECKPOINT-A");
    first.objective = "Complete the prerequisite checkpoint without granting execution".into();
    let mut second = task_fixture("ROUND15-CHECKPOINT-B");
    second.objective = "Complete only after the prerequisite checkpoint is observed".into();
    second.dependencies = vec![first.task_id.clone()];
    let project_id = first.project_id.clone();
    let first_id = first.task_id.clone();
    let second_id = second.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round15-multistep-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        // Deliberately reversed: preview order is stable and dependency-first.
        tasks: vec![second, first],
    };
    let source = serde_json::to_vec(&plan).expect("serialize fixed stress workplan");
    let preview = preview_workplan(&source).expect("preview decomposed workplan");
    assert_eq!(preview.task_order, [first_id.clone(), second_id.clone()]);
    for _ in 0..32 {
        assert_eq!(
            preview,
            preview_workplan(&source).expect("replay bounded preview")
        );
    }
    assert!(matches!(
        preview_workplan(b"not-json"),
        Err(WorkplanError::Json(_))
    ));

    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round15-release-smoke-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder
        .tempdir()
        .expect("create private release-smoke fixture");
    let store = Store::open(root.path().join("workplans.sqlite")).expect("open workplan store");
    let request = RegisterWorkplanRequest {
        command_id: "round15-register-plan".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("digest canonical workplan"),
        source,
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("record plan checkpoint");
    for _ in 0..32 {
        assert_eq!(
            receipt,
            store
                .register_workplan(request.clone())
                .expect("idempotently replay registration")
        );
    }
    let recorded = serde_json::to_value(&receipt).expect("serialize recorded receipt");
    assert_eq!(recorded["recorded_only"], true);
    assert_eq!(recorded["execution_granted"], false);
    assert_eq!(recorded["acceptance_granted"], false);

    let persisted = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read durable workplan checkpoint")
        .expect("recorded revision exists");
    assert_eq!(persisted.plan, plan);
    let first_contract = persisted
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == first_id)
        .expect("prerequisite task persisted");
    let second_contract = persisted
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == second_id)
        .expect("dependent task persisted");
    let first_capsule = compile_task_capsule(first_contract, "round15-checkpoint", 4096)
        .expect("compile prerequisite task checkpoint");
    let second_capsule = compile_task_capsule(second_contract, "round15-checkpoint", 4096)
        .expect("compile dependent task checkpoint");
    assert_eq!(
        first_capsule.contract_digest,
        content_digest(first_contract).expect("digest first")
    );
    assert_eq!(
        second_capsule.contract_digest,
        content_digest(second_contract).expect("digest second")
    );
    assert!(compile_task_capsule(second_contract, "round15-checkpoint", 1).is_err());

    let scope = SchedulingScope {
        project_id,
        root_id: first_contract.root_id.clone(),
        revision: 1,
    };
    let scheduled =
        |contract: &admiralty_core::TaskContract, digest: String, state| ScheduledTask {
            observed_contract_revision: contract.revision,
            contract: contract.clone(),
            contract_digest: digest,
            state,
            lease: LeaseObservation::Absent,
        };
    let mut snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![
            scheduled(
                first_contract,
                first_capsule.contract_digest,
                TaskSchedulingState::Pending,
            ),
            scheduled(
                second_contract,
                second_capsule.contract_digest,
                TaskSchedulingState::Pending,
            ),
        ],
        available_quota: Knowledge::Known(2),
        available_capacity: Knowledge::Known(2),
    };
    let initial = propose_ready_tasks(&snapshot, &scope).expect("initial readiness checkpoint");
    assert_eq!(initial.ready.as_slice(), std::slice::from_ref(&first_id));
    assert!(matches!(
        initial.blocked.get(&second_id).and_then(|items| items.first()),
        Some(ReadinessBlocker::DependencyNotCompleted { dependency, .. }) if dependency == &first_id
    ));
    assert!(!initial.execution_granted);
    assert_eq!(
        initial,
        propose_ready_tasks(&snapshot, &scope).expect("replay initial checkpoint")
    );

    snapshot.tasks[0].state = TaskSchedulingState::Completed;
    let progressed =
        propose_ready_tasks(&snapshot, &scope).expect("next checkpoint after prerequisite");
    assert_eq!(
        progressed.ready.as_slice(),
        std::slice::from_ref(&second_id)
    );
    assert!(!progressed.execution_granted);
    assert_eq!(
        progressed,
        propose_ready_tasks(&snapshot, &scope).expect("replay progressed checkpoint")
    );

    let mut forged = snapshot.clone();
    forged.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == second_id.as_str()
    ));
    let mut stale_scope = scope.clone();
    stale_scope.revision = 2;
    assert!(matches!(
        propose_ready_tasks(&snapshot, &stale_scope),
        Err(SchedulerError::ScopeMismatch)
    ));
    snapshot.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&snapshot, &scope).expect("unknown capacity blocks closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);
    assert!(
        store
            .events_after(0, 10)
            .expect("read unchanged event log")
            .is_empty()
    );
}
