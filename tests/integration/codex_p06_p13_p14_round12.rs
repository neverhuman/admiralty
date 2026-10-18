//! Round 12 P06/P13/P14 receipt integrity and fail-closed red team gate.

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

fn private_root() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round12-redteam-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().expect("private red-team fixture root")
}

#[test]
fn p06_p13_p14_round12_item5_redteam_audit_gate() {
    let root = private_root();
    let database = root.path().join("registry.sqlite");
    let contract = task_fixture("ROUND12-REDTEAM");
    let project_id = contract.project_id.clone();
    let task_id = contract.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round12-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract.clone()],
    };
    let source = serde_json::to_vec(&plan).expect("serialize fixed workplan");
    let preview = preview_workplan(&source).expect("canonical workplan preview");
    let request = RegisterWorkplanRequest {
        command_id: "round12-registration".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("canonical plan digest"),
        source,
    };

    let store = Store::open(&database).expect("open workplan registry");
    let receipt = store
        .register_workplan(request.clone())
        .expect("register immutable revision");
    assert_eq!(
        store
            .register_workplan(request)
            .expect("exact request replay"),
        receipt
    );
    let receipt_json = serde_json::to_value(&receipt).expect("receipt JSON");
    assert_eq!(receipt_json["recorded_only"], true);
    assert_eq!(receipt_json["execution_granted"], false);
    assert_eq!(receipt_json["acceptance_granted"], false);
    assert!(
        store
            .events_after(0, 10)
            .expect("empty task event log")
            .is_empty()
    );

    let registered = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read registered plan")
        .expect("registered revision exists");
    let registered_contract = &registered.plan.tasks[0];
    let capsule = compile_task_capsule(registered_contract, "redteam-epoch", 4096)
        .expect("compile complete context capsule");
    assert_eq!(
        capsule.contract_digest,
        content_digest(registered_contract).expect("registered contract digest")
    );
    assert!(compile_task_capsule(registered_contract, "redteam-epoch", 1).is_err());
    assert!(compile_task_capsule(registered_contract, " ", 4096).is_err());
    let mut forged_capsule = capsule.clone();
    forged_capsule.contract_digest = "sha256:forged".into();
    assert_ne!(
        forged_capsule.contract_digest,
        content_digest(registered_contract).expect("registered contract digest")
    );

    let scope = SchedulingScope {
        project_id: project_id.clone(),
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
    let mut snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![scheduled],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    };
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("valid readiness proposal");
    assert_eq!(proposal.ready.as_slice(), std::slice::from_ref(&task_id));
    assert!(!proposal.execution_granted);
    snapshot.tasks[0].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&snapshot, &scope),
        Err(SchedulerError::ContractDigestMismatch(id)) if id == task_id.as_str()
    ));
    snapshot.tasks[0].contract_digest = capsule.contract_digest;
    snapshot.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&snapshot, &scope).expect("unknown capacity fails closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);

    let before = store.snapshot().expect("pre-tamper snapshot");
    let attacker = rusqlite::Connection::open(&database).expect("fixture tamper connection");
    assert_eq!(
        attacker
            .execute(
                "UPDATE workplan_registry_revisions SET receipt_json='{}' WHERE project_id=?1 AND plan_id=?2 AND revision=1",
                rusqlite::params![project_id.as_str(), plan.plan_id],
            )
            .expect("tamper persisted receipt"),
        1
    );
    assert!(store.workplan(project_id, plan.plan_id, Some(1)).is_err());
    assert_eq!(
        store.snapshot().expect("tamper must not mutate projection"),
        before
    );
    assert!(
        store
            .events_after(0, 10)
            .expect("tamper emits no task events")
            .is_empty()
    );
}
