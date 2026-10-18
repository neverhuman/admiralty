#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Round 22 canary smoke and release-package qualification for P06/P13/P14.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ScheduledTask, SchedulerError, SchedulerSnapshot,
        SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;
use serde_json::json;

#[test]
fn codex_round22_p06_p13_p14() {
    let mut contract = task_fixture("ROUND22-CANARY");
    contract.objective = "Canary smoke preserves release scope and grants no execution".into();
    let project_id = contract.project_id.clone();
    let task_id = contract.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round22-canary-release".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract],
    };
    let source = serde_json::to_vec(&plan).expect("serialize canary workplan");
    let preview = preview_workplan(&source).expect("preview canary workplan");
    assert_eq!(
        preview,
        preview_workplan(&source).expect("repeat canary preview")
    );

    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round22-canary-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder.tempdir().expect("private canary package root");
    let database = root.path().join("canary.sqlite");
    let store = Store::open(&database).expect("open isolated canary store");
    let request = RegisterWorkplanRequest {
        command_id: "round22-canary-register".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("canonical release digest"),
        source,
    };
    let receipt = store
        .register_workplan(request.clone())
        .expect("record canary release checkpoint");
    for _ in 0..4 {
        assert_eq!(
            receipt,
            store
                .register_workplan(request.clone())
                .expect("replay returns original checkpoint receipt")
        );
    }
    drop(store);

    let reopened_store = Store::open(&database).expect("reopen canary state");
    let registered = reopened_store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read release checkpoint")
        .expect("release checkpoint survives reopen");
    assert_eq!(registered.plan, plan);
    assert_eq!(registered.receipt, receipt);
    let registered_contract = &registered.plan.tasks[0];
    let capsule = compile_task_capsule(registered_contract, "round22-fixed-epoch", 4096)
        .expect("compile canary context capsule");
    assert!(
        capsule
            .text
            .contains("Canary smoke preserves release scope")
    );
    assert!(compile_task_capsule(registered_contract, "round22-fixed-epoch", 1).is_err());

    let scope = SchedulingScope {
        project_id,
        root_id: registered_contract.root_id.clone(),
        revision: 1,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![ScheduledTask {
            contract: registered_contract.clone(),
            contract_digest: capsule.contract_digest.clone(),
            observed_contract_revision: registered_contract.revision,
            state: TaskSchedulingState::Pending,
            lease: LeaseObservation::Absent,
        }],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    };
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("canary readiness proposal");
    assert_eq!(proposal.ready.as_slice(), std::slice::from_ref(&task_id));
    assert!(!proposal.execution_granted);
    assert_eq!(
        proposal,
        propose_ready_tasks(&snapshot, &scope).expect("deterministic canary replay")
    );

    let receipt_value = serde_json::to_value(&registered.receipt).expect("receipt JSON");
    assert_eq!(receipt_value["recorded_only"], true);
    assert_eq!(receipt_value["task_events_emitted"], false);
    assert_eq!(receipt_value["execution_granted"], false);
    assert_eq!(receipt_value["acceptance_granted"], false);
    let package = json!({
        "format_version": 1,
        "workplan_digest": content_digest(&registered.plan).expect("plan digest"),
        "task_digest": capsule.contract_digest,
        "readiness_digest": proposal.source_digest,
        "checkpoint": proposal,
        "receipt": receipt_value,
    });
    let package_bytes = serde_json::to_vec(&package).expect("encode deterministic release package");
    let replay_bytes = serde_json::to_vec(&package).expect("repeat package encoding");
    assert_eq!(package_bytes, replay_bytes);
    let package_path = root.path().join("canary-release-package.json");
    std::fs::write(&package_path, &package_bytes).expect("write canary package locally");
    let reopened: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package_path).expect("read canary package"))
            .expect("parse canary package");
    assert_eq!(reopened["receipt"]["acceptance_granted"], false);

    let mut forged = snapshot.clone();
    forged.tasks[0].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == task_id.as_str()
    ));
    let mut unknown = snapshot;
    unknown.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&unknown, &scope).expect("unknown capacity blocks canary");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);
    assert!(
        reopened_store
            .events_after(0, 10)
            .expect("canary emitted no work events")
            .is_empty()
    );
}
