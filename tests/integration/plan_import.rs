//! Main-branch workplan import qualification.
//!
//! The current main implementation provides bounded, read-only JSON preview.
//! These tests deliberately exercise that real API without implying that a
//! preview persists work or trusts legacy status fields.

use admiralty_core::workplans::{
    MAX_WORKPLAN_BYTES, Workplan, WorkplanError, preview_workplan, preview_workplan_with_existing,
};
use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId};

fn contract(task_id: &str, project_id: ProjectId, dependencies: &[&str]) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id,
        root_id: RootId::new("root").expect("valid root ID"),
        task_id: TaskId::new(task_id).expect("valid task ID"),
        revision: 1,
        title: format!("Task {task_id}"),
        objective: "Complete a bounded task".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: dependencies
            .iter()
            .map(|id| TaskId::new(*id).expect("valid dependency ID"))
            .collect(),
        criteria: vec![AcceptanceCriterion {
            id: "verified".into(),
            text: "The named check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["check".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "test-authority".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn source(plan: &Workplan) -> Vec<u8> {
    serde_json::to_vec(plan).expect("serialize valid workplan")
}

#[test]
fn repeated_preview_is_idempotent_and_dependency_ordered() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![
            contract("child", project.clone(), &["parent"]),
            contract("parent", project, &[]),
        ],
    };
    let bytes = source(&plan);
    let first = preview_workplan(&bytes).expect("valid preview");
    let replay = preview_workplan_with_existing(&bytes, &first.task_digests)
        .expect("identical replay remains previewable");

    assert_eq!(first, replay);
    assert_eq!(
        first.task_order,
        ["parent", "child"].map(|id| TaskId::new(id).expect("task ID"))
    );
}

#[test]
fn legacy_pass_and_checkbox_fields_cannot_become_trusted_acceptance() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![contract("task", project, &[])],
    };
    let mut value: serde_json::Value =
        serde_json::from_slice(&source(&plan)).expect("serialized workplan JSON");
    value["tasks"][0]["legacy_status"] = "PASS".into();
    value["tasks"][0]["accepted_checkbox"] = true.into();

    assert!(matches!(
        preview_workplan(&serde_json::to_vec(&value).expect("serialize tampered JSON")),
        Err(WorkplanError::Json(_))
    ));
}

#[test]
fn mismatched_project_scope_is_rejected_before_any_admission() {
    let declared = ProjectId::new("declared").expect("valid project ID");
    let foreign = ProjectId::new("foreign").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: declared,
        tasks: vec![contract("task", foreign, &[])],
    };

    assert!(matches!(
        preview_workplan(&source(&plan)),
        Err(WorkplanError::ProjectMismatch { .. })
    ));
}

#[test]
fn oversized_source_is_rejected_before_json_parsing() {
    let oversized = vec![b' '; MAX_WORKPLAN_BYTES + 1];

    assert_eq!(preview_workplan(&oversized), Err(WorkplanError::TooLarge));
}
