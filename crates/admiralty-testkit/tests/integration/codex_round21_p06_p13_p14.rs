#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Round 21 hermetic CI and cross-packet release qualification.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ReadinessBlocker, ScheduledTask, SchedulerError,
        SchedulerSnapshot, SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;
use serde_json::json;

#[test]
fn codex_round21_p06_p13_p14() {
    let mut intake = task_fixture("ROUND21-HERMETIC-INTAKE");
    intake.objective = "Preserve the reviewed rationale in a relocatable release bundle".into();
    let mut release = task_fixture("ROUND21-HERMETIC-RELEASE");
    release.dependencies = vec![intake.task_id.clone()];
    let project_id = intake.project_id.clone();
    let intake_id = intake.task_id.clone();
    let release_id = release.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round21-hermetic-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![release, intake],
    };
    let source = serde_json::to_vec(&plan).expect("serialize hermetic workplan");
    let preview = preview_workplan(&source).expect("preview workplan");
    assert_eq!(preview.task_order, [intake_id.clone(), release_id.clone()]);
    assert!(
        plan.tasks
            .iter()
            .flat_map(|task| &task.owned_paths)
            .all(|path| { !std::path::Path::new(path).is_absolute() })
    );

    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round21-hermetic-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = builder.tempdir().expect("create isolated relocation root");
    let store = Store::open(root.path().join("state.sqlite")).expect("open private store");
    let request = RegisterWorkplanRequest {
        command_id: "round21-hermetic-register".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("canonical digest"),
        source: source.clone(),
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("record relocatable checkpoint");
    assert_eq!(
        receipt,
        store
            .register_workplan(request)
            .expect("replay registration without environment dependence")
    );

    let recorded = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read recorded plan")
        .expect("plan remains durable");
    let intake_contract = recorded
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == intake_id)
        .expect("intake contract");
    let release_contract = recorded
        .plan
        .tasks
        .iter()
        .find(|contract| contract.task_id == release_id)
        .expect("release contract");
    let intake_capsule = compile_task_capsule(intake_contract, "round21-hermetic-epoch", 4096)
        .expect("compile intake capsule");
    let release_capsule = compile_task_capsule(release_contract, "round21-hermetic-epoch", 4096)
        .expect("compile release capsule");
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
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("deterministic readiness");
    assert_eq!(proposal.ready.as_slice(), std::slice::from_ref(&intake_id));
    assert!(matches!(
        proposal
            .blocked
            .get(&release_id)
            .and_then(|blockers| blockers.first()),
        Some(ReadinessBlocker::DependencyNotCompleted { dependency, .. }) if dependency == &intake_id
    ));
    assert!(!proposal.execution_granted);

    let receipt_json = serde_json::to_value(&recorded.receipt).expect("receipt JSON");
    let bundle = json!({
        "schema_version": 1,
        "workplan_digest": content_digest(&recorded.plan).expect("registered plan digest"),
        "source_digest": content_digest(&source).expect("raw source digest"),
        "task_order": preview.task_order,
        "capsule_digests": [
            content_digest(intake_contract).expect("intake contract digest"),
            content_digest(release_contract).expect("release contract digest"),
        ],
        "checkpoint": proposal,
        "receipt": receipt_json,
    });
    let first_bytes = serde_json::to_vec(&bundle).expect("serialize hermetic release bundle");
    let second_bytes = serde_json::to_vec(&bundle).expect("repeat bundle serialization");
    assert_eq!(first_bytes, second_bytes);
    let bundle_path = root.path().join("release-bundle.json");
    std::fs::write(&bundle_path, first_bytes).expect("write release artifact under fixture root");
    let reopened = std::fs::read(bundle_path).expect("reopen release artifact");
    let reopened_json: serde_json::Value = serde_json::from_slice(&reopened).expect("bundle JSON");
    assert_eq!(reopened_json["receipt"]["execution_granted"], false);
    assert_eq!(reopened_json["receipt"]["acceptance_granted"], false);

    let mut forged = snapshot.clone();
    forged.tasks[1].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == release_id.as_str()
    ));
    let mut unknown = snapshot;
    unknown.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&unknown, &scope).expect("unknown capacity fails closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);
    assert!(
        store
            .events_after(0, 10)
            .expect("no dispatch events")
            .is_empty()
    );
}
