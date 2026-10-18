#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Round 19/20 post-PR-57 release qualification and strict lint audit.

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
fn codex_round20_p06_p13_p14() {
    let mut intake = task_fixture("ROUND20-INTAKE");
    intake.objective =
        "Origin: release-audit://item-5; rationale: keep the reviewed checkpoint intact".into();
    let mut release = task_fixture("ROUND20-RELEASE");
    release.dependencies = vec![intake.task_id.clone()];
    release.objective = "Publish only after the intake checkpoint is complete".into();
    let project_id = intake.project_id.clone();
    let intake_id = intake.task_id.clone();
    let release_id = release.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round20-release-audit".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![release, intake],
    };
    let source = serde_json::to_vec(&plan).expect("serialize exact release workplan");
    let preview = preview_workplan(&source).expect("validate release decomposition");
    assert_eq!(preview.task_order, [intake_id.clone(), release_id.clone()]);
    assert_eq!(
        preview,
        preview_workplan(&source).expect("deterministic decomposition replay")
    );

    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round20-post-pr57-");
    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let root = builder.tempdir().expect("private audit root");
    let store = Store::open(root.path().join("audit.sqlite")).expect("open workplan registry");
    let request = RegisterWorkplanRequest {
        command_id: "round20-exact-request".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest.clone(),
        canonical_digest: content_digest(&plan).expect("canonical plan digest"),
        source: source.clone(),
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("persist exact checkpoint");
    for _ in 0..8 {
        assert_eq!(
            receipt,
            store
                .register_workplan(request.clone())
                .expect("exact request replay returns original receipt")
        );
    }

    let mut conflicting_plan = plan.clone();
    conflicting_plan.tasks[0]
        .objective
        .push_str(" with changed request contents");
    let conflicting_source = serde_json::to_vec(&conflicting_plan).expect("serialize changed plan");
    let conflicting_preview =
        preview_workplan(&conflicting_source).expect("changed plan remains structurally valid");
    let conflicting_request = RegisterWorkplanRequest {
        command_id: request.command_id.clone(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: conflicting_preview.source_digest,
        canonical_digest: content_digest(&conflicting_plan).expect("changed plan digest"),
        source: conflicting_source,
    };
    assert!(matches!(
        store.register_workplan(conflicting_request),
        Err(StoreError::Workplan(
            admiralty_store::workplans::WorkplanStoreError::IdentityCollision
        ))
    ));

    let registered = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read durable checkpoint")
        .expect("checkpoint is retained");
    assert_eq!(registered.plan, plan);
    assert_eq!(registered.receipt, receipt);
    let intake_contract = registered
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == intake_id)
        .expect("intake remains bound");
    let release_contract = registered
        .plan
        .tasks
        .iter()
        .find(|task| task.task_id == release_id)
        .expect("release remains bound");
    let intake_capsule = compile_task_capsule(intake_contract, "round20-epoch", 4096)
        .expect("compile rationale-bound capsule");
    assert!(intake_capsule.text.contains("release-audit://item-5"));
    assert!(intake_capsule.text.contains("reviewed checkpoint intact"));
    assert!(compile_task_capsule(intake_contract, "round20-epoch", 1).is_err());
    assert!(compile_task_capsule(intake_contract, " ", 4096).is_err());
    let release_capsule = compile_task_capsule(release_contract, "round20-epoch", 4096)
        .expect("compile dependent release capsule");

    let scope = SchedulingScope {
        project_id,
        root_id: intake_contract.root_id.clone(),
        revision: 1,
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
    let waiting = propose_ready_tasks(&snapshot, &scope).expect("initial release readiness");
    assert_eq!(waiting.ready.as_slice(), std::slice::from_ref(&intake_id));
    assert!(matches!(
        waiting
            .blocked
            .get(&release_id)
            .and_then(|blockers| blockers.first()),
        Some(ReadinessBlocker::DependencyNotCompleted { dependency, .. })
            if dependency == &intake_id
    ));
    assert!(!waiting.execution_granted);

    let mut checkpointed = snapshot.clone();
    checkpointed.tasks[0].state = TaskSchedulingState::Completed;
    let ready = propose_ready_tasks(&checkpointed, &scope).expect("read next checkpoint");
    assert_eq!(ready.ready.as_slice(), std::slice::from_ref(&release_id));
    assert!(!ready.execution_granted);
    assert_eq!(
        ready,
        propose_ready_tasks(&checkpointed, &scope).expect("exact scheduler replay")
    );

    let mut forged = checkpointed.clone();
    forged.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == release_id.as_str()
    ));
    let mut stale_scope = scope.clone();
    stale_scope.revision = 2;
    assert!(matches!(
        propose_ready_tasks(&checkpointed, &stale_scope),
        Err(SchedulerError::ScopeMismatch)
    ));
    checkpointed.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&checkpointed, &scope).expect("unknown capacity blocks");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);

    let receipt_json = serde_json::to_value(receipt).expect("release receipt JSON");
    assert_eq!(receipt_json["recorded_only"], true);
    assert_eq!(receipt_json["task_events_emitted"], false);
    assert_eq!(receipt_json["execution_granted"], false);
    assert_eq!(receipt_json["acceptance_granted"], false);
    assert!(
        store
            .events_after(0, 10)
            .expect("empty execution event log")
            .is_empty()
    );
}
