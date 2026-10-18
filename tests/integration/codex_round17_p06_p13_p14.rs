//! Round 17 release audit across P06 context, P13 workplans and P14 checkpoints.

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

fn request_for(plan: &Workplan, command_id: &str) -> RegisterWorkplanRequest {
    let source = serde_json::to_vec(plan).expect("serialize qualification plan");
    let preview = preview_workplan(&source).expect("preview qualification plan");
    RegisterWorkplanRequest {
        command_id: command_id.into(),
        project_id: plan.project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: (plan.revision - 1).to_string(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(plan).expect("digest qualification plan"),
        source,
    }
}

#[test]
fn codex_round17_p06_p13_p14() {
    let mut intake = task_fixture("ROUND17-INTAKE");
    intake.objective =
        "Origin: intake://round17/17; rationale: preserve the review checkpoint".into();
    let mut release = task_fixture("ROUND17-RELEASE");
    release.dependencies = vec![intake.task_id.clone()];
    release.objective = "Release checkpoint follows the reviewed intake checkpoint".into();
    let project_id = intake.project_id.clone();
    let intake_id = intake.task_id.clone();
    let release_id = release.task_id.clone();
    let mut plan = Workplan {
        schema_version: 1,
        plan_id: "round17-audit-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![release, intake],
    };

    let first_source = serde_json::to_vec(&plan).expect("serialize initial plan");
    let first_preview = preview_workplan(&first_source).expect("preview initial plan");
    assert_eq!(
        first_preview.task_order,
        [intake_id.clone(), release_id.clone()]
    );
    assert_eq!(
        first_preview,
        preview_workplan(&first_source).expect("deterministic preview replay")
    );

    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round17-audit-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder.tempdir().expect("create private audit root");
    let store = Store::open(root.path().join("round17.sqlite")).expect("open audit registry");
    let first_request = request_for(&plan, "round17-plan-revision-one");
    let first_receipt = store
        .register_workplan(first_request.clone())
        .expect("record first immutable revision");
    assert_eq!(
        first_receipt,
        store
            .register_workplan(first_request.clone())
            .expect("exact first revision replay")
    );

    plan.revision = 2;
    let second_request = request_for(&plan, "round17-plan-revision-two");
    let second_receipt = store
        .register_workplan(second_request.clone())
        .expect("record second checkpoint revision");
    assert_eq!(
        second_receipt,
        store
            .register_workplan(second_request)
            .expect("exact second revision replay")
    );

    let historical = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read historical release evidence")
        .expect("first revision retained");
    assert_eq!(historical.plan.revision, 1);
    assert_eq!(historical.receipt, first_receipt);
    let current = store
        .workplan(project_id.clone(), plan.plan_id.clone(), None)
        .expect("read current revision")
        .expect("current plan retained");
    assert_eq!(current.plan, plan);
    assert_eq!(current.receipt, second_receipt);

    let intake_contract = current
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == intake_id)
        .expect("intake contract exists");
    let release_contract = current
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == release_id)
        .expect("release contract exists");
    let intake_capsule = compile_task_capsule(intake_contract, "round17-epoch", 4096)
        .expect("compile review-bound intake capsule");
    assert!(intake_capsule.text.contains("intake://round17/17"));
    assert!(
        intake_capsule
            .text
            .contains("preserve the review checkpoint")
    );
    let release_capsule = compile_task_capsule(release_contract, "round17-epoch", 4096)
        .expect("compile release capsule");

    let scope = SchedulingScope {
        project_id,
        root_id: intake_contract.root_id.clone(),
        revision: 2,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![
            ScheduledTask {
                contract: intake_contract.clone(),
                contract_digest: intake_capsule.contract_digest,
                observed_contract_revision: intake_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
            ScheduledTask {
                contract: release_contract.clone(),
                contract_digest: release_capsule.contract_digest,
                observed_contract_revision: release_contract.revision,
                state: TaskSchedulingState::Pending,
                lease: LeaseObservation::Absent,
            },
        ],
        available_quota: Knowledge::Known(2),
        available_capacity: Knowledge::Known(2),
    };
    let initial = propose_ready_tasks(&snapshot, &scope).expect("validate release checkpoint");
    assert_eq!(initial.ready.as_slice(), std::slice::from_ref(&intake_id));
    assert!(matches!(
        initial
            .blocked
            .get(&release_id)
            .and_then(|blockers| blockers.first()),
        Some(ReadinessBlocker::DependencyNotCompleted { dependency, .. })
            if dependency == &intake_id
    ));
    assert!(!initial.execution_granted);

    let mut checkpointed = snapshot.clone();
    checkpointed.tasks[0].state = TaskSchedulingState::Completed;
    let next = propose_ready_tasks(&checkpointed, &scope).expect("advance read-only checkpoint");
    assert_eq!(next.ready.as_slice(), std::slice::from_ref(&release_id));
    assert!(!next.execution_granted);
    assert_eq!(
        next,
        propose_ready_tasks(&checkpointed, &scope).expect("replay next checkpoint")
    );

    let reused_identity = request_for(&plan, "round17-plan-revision-one");
    assert!(matches!(
        store.register_workplan(reused_identity),
        Err(StoreError::Workplan(
            admiralty_store::workplans::WorkplanStoreError::IdentityCollision
        ))
    ));

    let mut tampered = checkpointed.clone();
    tampered.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&tampered, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == release_id.as_str()
    ));
    let mut stale_scope = scope.clone();
    stale_scope.revision = 3;
    assert!(matches!(
        propose_ready_tasks(&checkpointed, &stale_scope),
        Err(SchedulerError::ScopeMismatch)
    ));
    checkpointed.available_capacity = Knowledge::Unknown;
    let blocked =
        propose_ready_tasks(&checkpointed, &scope).expect("unknown capacity fails closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);

    for receipt in [first_receipt, second_receipt] {
        let value = serde_json::to_value(receipt).expect("serialize audit receipt");
        assert_eq!(value["recorded_only"], true);
        assert_eq!(value["task_events_emitted"], false);
        assert_eq!(value["execution_granted"], false);
        assert_eq!(value["acceptance_granted"], false);
    }
    assert!(
        store
            .events_after(0, 10)
            .expect("no work dispatched")
            .is_empty()
    );
}
