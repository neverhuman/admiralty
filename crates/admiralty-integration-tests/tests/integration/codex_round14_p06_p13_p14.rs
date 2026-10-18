//! Round 14 deterministic replay and fail-closed P06/P13/P14 qualification.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    scheduler::{
        Knowledge, LeaseObservation, ScheduledTask, SchedulerError, SchedulerSnapshot,
        SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
    workplans::{Workplan, WorkplanError, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;

#[test]
fn codex_round14_p06_p13_p14() {
    let mut root_builder = tempfile::Builder::new();
    root_builder.prefix("admiralty-round14-replay-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        root_builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let root = root_builder
        .tempdir()
        .expect("create isolated fixture directory");
    let mut contract = task_fixture("ROUND14-P06-P13-P14");
    contract.objective =
        "Replay the same checkpoint deterministically; never infer execution from readiness".into();
    let project_id = contract.project_id.clone();
    let task_id = contract.task_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round14-workplan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract],
    };
    let source = serde_json::to_vec(&plan).expect("serialize deterministic workplan");
    let preview = preview_workplan(&source).expect("preview valid workplan");
    assert_eq!(preview, preview_workplan(&source).expect("replay preview"));
    assert!(matches!(
        preview_workplan(
            br#"{"schema_version":99,"plan_id":"bad","revision":1,"project_id":"p","tasks":[]}"#
        ),
        Err(WorkplanError::UnsupportedVersion)
    ));

    let request = RegisterWorkplanRequest {
        command_id: "round14-register".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan).expect("digest canonical plan"),
        source,
    };
    let store = Store::open(root.path().join("workplans.sqlite")).expect("open fixture store");
    let receipt = store
        .register_workplan(request.clone())
        .expect("register deterministic revision");
    assert_eq!(
        receipt,
        store
            .register_workplan(request)
            .expect("replay registration exactly")
    );
    let persisted = store
        .workplan(project_id.clone(), plan.plan_id.clone(), Some(1))
        .expect("read persisted checkpoint")
        .expect("registered plan exists");
    assert_eq!(persisted.plan, plan);
    let capsule = compile_task_capsule(&persisted.plan.tasks[0], "round14-epoch", 4096)
        .expect("compile bounded context checkpoint");
    assert!(
        capsule
            .text
            .contains("never infer execution from readiness")
    );

    let scope = SchedulingScope {
        project_id,
        root_id: persisted.plan.tasks[0].root_id.clone(),
        revision: 1,
    };
    let snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![ScheduledTask {
            observed_contract_revision: persisted.plan.tasks[0].revision,
            contract: persisted.plan.tasks[0].clone(),
            contract_digest: capsule.contract_digest.clone(),
            state: TaskSchedulingState::Pending,
            lease: LeaseObservation::Absent,
        }],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    };
    let proposal = propose_ready_tasks(&snapshot, &scope).expect("propose checkpoint readiness");
    assert_eq!(proposal.ready, [task_id]);
    assert!(!proposal.execution_granted);
    assert_eq!(
        proposal,
        propose_ready_tasks(&snapshot, &scope).expect("replay scheduling deterministically")
    );

    let mut forged = snapshot.clone();
    forged.tasks[0].contract_digest = "sha256:forged".into();
    assert!(matches!(
        propose_ready_tasks(&forged, &scope),
        Err(SchedulerError::ContractDigestMismatch(_))
    ));
    let mut unknown = snapshot;
    unknown.available_capacity = Knowledge::Unknown;
    let blocked = propose_ready_tasks(&unknown, &scope).expect("unknown capacity fails closed");
    assert!(blocked.ready.is_empty());
    assert!(!blocked.execution_granted);
    assert!(
        store
            .events_after(0, 10)
            .expect("no task events")
            .is_empty()
    );
}
