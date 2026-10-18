//! Round 13 work-order rationale binding and release evidence qualification.

use admiralty_core::{
    content_digest,
    context::compile_task_capsule,
    workplans::{Workplan, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use admiralty_testkit::task_fixture;

#[test]
fn p06_p13_p14_round13_wo_intake_origin_rationale_binding() {
    let mut contract = task_fixture("ROUND13-WO-BINDING");
    let origin = "intake://request/round13/42";
    let rationale = "Preserve the operator's stated rationale: verify the release candidate against its agreed scope.";
    contract.objective = format!("Origin: {origin}\nRationale: {rationale}");
    contract.non_goals = vec!["A recorded work order does not grant execution or release".into()];
    let project_id = contract.project_id.clone();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round13-work-order-plan".into(),
        revision: 1,
        project_id: project_id.clone(),
        tasks: vec![contract.clone()],
    };
    let source = serde_json::to_vec(&plan).expect("serialize fixed work-order task");
    let preview = preview_workplan(&source).expect("validate work-order task");
    let task_digest = preview.task_digests[&contract.task_id].clone();
    let source_digest = preview.source_digest;
    let request = RegisterWorkplanRequest {
        command_id: "round13-record-work-order".into(),
        project_id: project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: "0".into(),
        source_digest,
        canonical_digest: content_digest(&plan).expect("canonical plan digest"),
        source,
    };

    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-round13-workorder-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir().expect("private fixture directory");
    let store = Store::open(directory.path().join("workorders.sqlite")).expect("open registry");
    let receipt = store
        .register_workplan(request.clone())
        .expect("record plan revision");
    assert_eq!(
        store.register_workplan(request).expect("idempotent retry"),
        receipt
    );
    let receipt_json = serde_json::to_value(&receipt).expect("receipt JSON");
    assert_eq!(receipt_json["recorded_only"], true);
    assert_eq!(receipt_json["task_events_emitted"], false);
    assert_eq!(receipt_json["execution_granted"], false);
    assert_eq!(receipt_json["acceptance_granted"], false);

    let registered = store
        .workplan(project_id, plan.plan_id, Some(1))
        .expect("read registered plan")
        .expect("revision exists");
    let registered_contract = &registered.plan.tasks[0];
    assert_eq!(
        content_digest(registered_contract).expect("bound contract digest"),
        task_digest
    );
    let capsule = compile_task_capsule(registered_contract, "round13-fixed-epoch", 4096)
        .expect("compile bounded task context");
    assert!(
        capsule.text.contains(origin),
        "intake origin remains attached to task context"
    );
    assert!(
        capsule.text.contains(rationale),
        "rationale remains attached to task context"
    );
    assert_eq!(capsule.contract_digest, task_digest);

    // Current registration records no delivery-stage evidence. A plan receipt
    // therefore cannot be treated as merge, install, running, or release proof.
    assert!(
        store
            .events_after(0, 10)
            .expect("no execution events")
            .is_empty()
    );
    for evidence in ["merge", "install", "running", "release"] {
        assert!(
            !receipt_json.to_string().contains(evidence),
            "registration must not invent {evidence} evidence"
        );
    }
}
