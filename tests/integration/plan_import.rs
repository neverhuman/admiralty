//! Main-branch workplan import qualification.
//!
//! The current main implementation provides bounded, read-only JSON preview.
//! These tests deliberately exercise that real API without implying that a
//! preview persists work or trusts legacy status fields.

use admiralty_core::workplans::{
    MAX_WORKPLAN_BYTES, Workplan, WorkplanError, preview_workplan, preview_workplan_with_existing,
};
use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId};
use admiralty_testkit::{
    clock::FakeClock,
    faults::{FaultEffect, FaultPlan, FaultRule},
};

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
    assert_eq!(first.task_order.len(), first.task_digests.len());
    assert_eq!(
        first.task_order,
        ["parent", "child"].map(|id| TaskId::new(id).expect("task ID"))
    );
}

#[test]
fn synthetic_retry_after_clock_advance_preserves_preview_identity() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "retry-plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![contract("task", project, &[])],
    };
    let bytes = source(&plan);
    let expected = preview_workplan(&bytes).expect("valid preview");
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "preview".into(),
        occurrence: 1,
        effect: FaultEffect::Fail,
    }])
    .expect("valid deterministic fault schedule");
    let mut clock = FakeClock::new(100);

    assert_eq!(faults.hit("preview"), Ok(Some(FaultEffect::Fail)));
    assert_eq!(clock.advance_ms(5), Ok(105));
    assert_eq!(faults.hit("preview"), Ok(None));
    assert_eq!(preview_workplan(&bytes), Ok(expected));
}

#[test]
fn replay_cannot_rewrite_an_existing_task_identity() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![contract("task", project.clone(), &[])],
    };
    let bytes = source(&plan);
    let existing = preview_workplan(&bytes)
        .expect("valid initial preview")
        .task_digests;
    let mut changed = plan;
    changed.tasks[0].objective = "Changed work under the same task ID".into();

    assert!(matches!(
        preview_workplan_with_existing(&source(&changed), &existing),
        Err(WorkplanError::TaskIdentityConflict(id)) if id == "task"
    ));
}

#[test]
fn truncated_json_is_rejected_without_a_partial_preview() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![contract("task", project, &[])],
    };
    let mut bytes = source(&plan);
    bytes.pop();

    assert!(matches!(
        preview_workplan(&bytes),
        Err(WorkplanError::Json(_))
    ));
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

#[test]
fn source_at_byte_limit_is_accepted() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![contract("task", project, &[])],
    };
    let mut bytes = source(&plan);
    assert!(bytes.len() <= MAX_WORKPLAN_BYTES);
    bytes.resize(MAX_WORKPLAN_BYTES, b' ');

    assert!(preview_workplan(&bytes).is_ok());
}

#[test]
fn cyclic_dependencies_are_rejected_before_import() {
    let project = ProjectId::new("project").expect("valid project ID");
    let plan = Workplan {
        schema_version: 1,
        plan_id: "cycle".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![
            contract("first", project.clone(), &["second"]),
            contract("second", project, &["first"]),
        ],
    };

    assert_eq!(
        preview_workplan(&source(&plan)),
        Err(WorkplanError::DependencyCycle)
    );
}

#[test]
fn typed_toml_and_json_preserve_canonical_identity_but_bind_original_source() {
    use admiralty_core::workplans::source::{PlanSource, preview_source};
    let json =
        br#"{"schema_version":1,"plan_id":"empty","revision":1,"project_id":"project","tasks":[]}"#;
    let toml = b"schema_version = 1\nplan_id = 'empty'\nrevision = 1\nproject_id = 'project'\ntasks = []\n";
    let a = preview_source(json, PlanSource::Json).expect("typed JSON");
    let b = preview_source(toml, PlanSource::Toml).expect("typed TOML");
    assert_eq!(a.canonical_workplan, b.canonical_workplan);
    assert_eq!(a.canonical_digest, b.canonical_digest);
    assert_ne!(a.source_digest, b.source_digest);
    assert_eq!(b, preview_source(toml, PlanSource::Toml).expect("replay"));
    assert!(!b.execution_granted);
    let invalid = [toml.as_slice(), b"legacy_status = 'PASS'\n"].concat();
    assert!(preview_source(&invalid, PlanSource::Toml).is_err());
}

#[test]
fn typed_toml_checks_real_task_contracts_and_dependency_cycles() {
    use admiralty_core::workplans::source::{PlanSource, preview_source};
    let task = r#"
[[tasks]]
schema_version = 1
project_id = 'project'
root_id = 'root'
task_id = 'TASK'
revision = 1
title = 'Original task'
objective = 'Preserve a complete typed contract'
non_goals = []
owned_paths = ['src']
forbidden_paths = []
read_first = []
dependencies = []
criteria = [{ id = 'c', text = 'Check passes', check_ids = ['unit'] }]
checks = [{ id = 'unit', program = 'cargo', args = ['test'], cwd = '.', timeout_secs = 30 }]
authority_ref = 'fixture'
max_attempts = 1
simulated = true
"#;
    let source = format!(
        "schema_version = 1\nplan_id = 'plan'\nrevision = 1\nproject_id = 'project'\n{task}"
    );
    let preview = preview_source(source.as_bytes(), PlanSource::Toml).expect("complete task");
    let plan = preview.canonical_workplan.expect("canonical workplan");
    assert_eq!(plan.tasks[0].task_id.as_str(), "TASK");
    let bytes = serde_json::to_vec(&plan).expect("JSON contract");
    let json_preview = preview_source(&bytes, PlanSource::Json).expect("same validation");
    assert_eq!(preview.canonical_digest, json_preview.canonical_digest);
    let cycle = source.replace("dependencies = []", "dependencies = ['TASK']");
    assert!(preview_source(cycle.as_bytes(), PlanSource::Toml).is_err());
    let missing = source.replace("check_ids = ['unit']", "check_ids = ['unknown']");
    assert!(preview_source(missing.as_bytes(), PlanSource::Toml).is_err());
}

#[test]
fn markdown_retains_original_ids_spans_and_unknown_acceptance_for_review() {
    use admiralty_core::workplans::source::{PlanSource, preview_source};
    let source = "# Plan 🧭\r\n| id | title | dependencies | owned_paths | criteria | status |\r\n| --- | --- | --- | --- | --- | --- |\r\n| TASK-1 | Vérify | TASK-0 | src | tests pass | PASS |\r\n- [x] completed\r\n";
    let preview = preview_source(source.as_bytes(), PlanSource::Markdown).expect("preview");
    let task = &preview.observed_tasks[0];
    assert_eq!(task.original_id, "TASK-1");
    assert_eq!(task.dependencies, ["TASK-0"]);
    assert_eq!(task.criteria, ["tests pass"]);
    assert_eq!(task.owned_paths, ["src"]);
    assert_eq!(task.legacy_status.as_deref(), Some("PASS"));
    assert!(
        source[task.source_span.start as usize..task.source_span.end as usize]
            .starts_with("| TASK-1 |")
    );
    assert!(preview.canonical_workplan.is_none());
    assert!(preview.canonical_digest.is_none());
    assert_eq!(preview.diagnostics.len(), 2);
    assert!(!preview.execution_granted);
    assert_eq!(
        preview,
        preview_source(source.as_bytes(), PlanSource::Markdown).expect("replay")
    );
}

#[test]
fn markdown_code_blocks_duplicate_ids_and_ambiguous_rows_do_not_become_contracts() {
    use admiralty_core::workplans::source::{PlanSource, preview_source};
    let source = b"```md\n| id | title |\n| hidden | never import |\n```\n| id | title |\n| A | one |\n| A | two |\n| B | extra | column |\n";
    let result = preview_source(source, PlanSource::Markdown).expect("bounded preview");
    assert_eq!(result.observed_tasks.len(), 2);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.reason.contains("duplicate original"))
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.reason.contains("ambiguous table"))
    );
    assert!(result.canonical_workplan.is_none());
    assert!(preview_source(&vec![b' '; MAX_WORKPLAN_BYTES + 1], PlanSource::Markdown).is_err());
}
