//! Offline intake adapter for the existing immutable workplan registry.
//!
//! This records validated TaskContracts only. It does not persist a separate
//! WorkOrder revision, delivery requirement, task event, or execution request.

use admiralty_core::{
    SourceIdentity, TaskId, content_digest,
    workplans::{MAX_WORKPLAN_BYTES, Workplan, WorkplanPreview, preview_workplan},
};
use admiralty_store::{Store, workplans::RegisterWorkplanRequest};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const WORK_ORDER_FIELDS: &[&str] = &["schema_version", "plan_id", "revision", "project_id", "tasks"];
const TASK_FIELDS: &[&str] = &[
    "schema_version",
    "project_id",
    "root_id",
    "task_id",
    "revision",
    "title",
    "objective",
    "non_goals",
    "owned_paths",
    "forbidden_paths",
    "read_first",
    "dependencies",
    "criteria",
    "checks",
    "authority_ref",
    "max_attempts",
    "simulated",
];

#[derive(Clone, Debug, Serialize)]
struct PreviewIdentity<'a> {
    source_identity: &'a SourceIdentity,
    source_digest: &'a str,
    plan_id: &'a str,
    revision: u32,
    project_id: &'a admiralty_core::ProjectId,
    task_order: &'a [TaskId],
    task_digests: &'a BTreeMap<TaskId, String>,
}

/// Reviewable, deterministic preview. A preview with diagnostics cannot record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeIntakePreview {
    preview_digest: String,
    source_digest: Option<String>,
    canonical_digest: Option<String>,
    task_order: Vec<TaskId>,
    task_digests: BTreeMap<TaskId, String>,
    acceptance_bindings: Vec<AcceptanceBinding>,
    missing_inputs: Vec<String>,
    missing_dependencies: Vec<String>,
    missing_authority: Vec<String>,
    missing_acceptance_bindings: Vec<String>,
    source_identity: SourceIdentity,
    plan: Option<Workplan>,
    plan_source: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AcceptanceBinding {
    pub task_id: TaskId,
    pub criterion_id: String,
    pub check_ids: Vec<String>,
}

impl NativeIntakePreview {
    pub fn preview_digest(&self) -> &str {
        &self.preview_digest
    }

    pub fn source_digest(&self) -> Option<&str> {
        self.source_digest.as_deref()
    }

    pub fn canonical_digest(&self) -> Option<&str> {
        self.canonical_digest.as_deref()
    }

    pub fn task_order(&self) -> &[TaskId] {
        &self.task_order
    }

    pub fn task_digests(&self) -> &BTreeMap<TaskId, String> {
        &self.task_digests
    }

    pub fn acceptance_bindings(&self) -> &[AcceptanceBinding] {
        &self.acceptance_bindings
    }

    pub fn missing_inputs(&self) -> &[String] {
        &self.missing_inputs
    }

    pub fn missing_dependencies(&self) -> &[String] {
        &self.missing_dependencies
    }

    pub fn missing_authority(&self) -> &[String] {
        &self.missing_authority
    }

    pub fn missing_acceptance_bindings(&self) -> &[String] {
        &self.missing_acceptance_bindings
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeIntakeReceipt {
    /// Source identity is returned to the caller; the current Store registry
    /// binds it through its immutable command ID because it has no source field.
    pub source_identity: SourceIdentity,
    pub workplan_receipt: admiralty_store::workplans::WorkplanReceipt,
}

fn required_fields(value: &Value, fields: &[&str], prefix: &str, missing: &mut Vec<String>) {
    for field in fields {
        if value.get(field).is_none_or(Value::is_null) {
            missing.push(format!("{prefix}{field}"));
        }
    }
}

fn collect_diagnostics(plan: &Value) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let mut missing_inputs = Vec::new();
    let mut missing_dependencies = Vec::new();
    let mut missing_authority = Vec::new();
    let mut missing_acceptance = Vec::new();
    required_fields(plan, WORK_ORDER_FIELDS, "plan.", &mut missing_inputs);

    let Some(tasks) = plan.get("tasks").and_then(Value::as_array) else {
        return (
            missing_inputs,
            missing_dependencies,
            missing_authority,
            missing_acceptance,
        );
    };
    let task_ids: BTreeSet<&str> = tasks
        .iter()
        .filter_map(|task| task.get("task_id").and_then(Value::as_str))
        .collect();
    for (index, task) in tasks.iter().enumerate() {
        let prefix = format!("plan.tasks[{index}].");
        required_fields(task, TASK_FIELDS, &prefix, &mut missing_inputs);
        let id = task
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or("<missing-task-id>");
        if task
            .get("authority_ref")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            missing_authority.push(id.to_owned());
        }
        let criteria = task.get("criteria").and_then(Value::as_array);
        if criteria.is_none_or(Vec::is_empty)
            || criteria.is_some_and(|items| {
                items.iter().any(|criterion| {
                    criterion
                        .get("check_ids")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty)
                })
            })
        {
            missing_acceptance.push(id.to_owned());
        }
        if let Some(dependencies) = task.get("dependencies").and_then(Value::as_array) {
            for dependency in dependencies.iter().filter_map(Value::as_str) {
                if !task_ids.contains(dependency) {
                    missing_dependencies.push(format!("{id}->{dependency}"));
                }
            }
        }
    }
    missing_inputs.sort();
    missing_dependencies.sort();
    missing_dependencies.dedup();
    missing_authority.sort();
    missing_authority.dedup();
    missing_acceptance.sort();
    missing_acceptance.dedup();
    (
        missing_inputs,
        missing_dependencies,
        missing_authority,
        missing_acceptance,
    )
}

fn preview_digest(
    source_identity: &SourceIdentity,
    source_digest: &str,
    plan_preview: &WorkplanPreview,
) -> crate::CliResult<String> {
    Ok(content_digest(&PreviewIdentity {
        source_identity,
        source_digest,
        plan_id: &plan_preview.plan_id,
        revision: plan_preview.revision,
        project_id: &plan_preview.project_id,
        task_order: &plan_preview.task_order,
        task_digests: &plan_preview.task_digests,
    })?)
}

/// Parse bounded JSON and produce diagnostics or a deterministic contract preview.
pub fn preview_work_order(
    source: &[u8],
    source_identity: SourceIdentity,
) -> crate::CliResult<NativeIntakePreview> {
    if source.len() > MAX_WORKPLAN_BYTES {
        return Err(crate::invalid("work order exceeds the 1 MiB input limit"));
    }
    source_identity
        .validate()
        .map_err(crate::invalid)?;
    let value: Value = serde_json::from_slice(source)?;
    let plan_value = &value;
    let (missing_inputs, missing_dependencies, missing_authority, missing_acceptance_bindings) =
        collect_diagnostics(plan_value);
    let raw_digest = content_digest(&source)?;
    if !missing_inputs.is_empty()
        || !missing_dependencies.is_empty()
        || !missing_authority.is_empty()
        || !missing_acceptance_bindings.is_empty()
    {
        let preview_digest = content_digest(&(
            &source_identity,
            &raw_digest,
            &missing_inputs,
            &missing_dependencies,
            &missing_authority,
            &missing_acceptance_bindings,
        ))?;
        let mut task_order: Vec<TaskId> = plan_value
            .get("tasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|task| task.get("task_id").and_then(Value::as_str))
            .filter_map(|id| TaskId::new(id.to_owned()).ok())
            .collect();
        task_order.sort();
        task_order.dedup();
        return Ok(NativeIntakePreview {
            preview_digest,
            source_digest: None,
            canonical_digest: None,
            task_order,
            task_digests: BTreeMap::new(),
            acceptance_bindings: Vec::new(),
            missing_inputs,
            missing_dependencies,
            missing_authority,
            missing_acceptance_bindings,
            source_identity,
            plan: None,
            plan_source: None,
        });
    }

    let plan: Workplan = serde_json::from_value(value)?;
    let plan_preview = preview_workplan(source)?;
    let canonical_digest = content_digest(&plan)?;
    let preview_digest = preview_digest(&source_identity, &raw_digest, &plan_preview)?;
    let acceptance_bindings = plan
        .tasks
        .iter()
        .flat_map(|task| {
            task.criteria.iter().map(|criterion| AcceptanceBinding {
                task_id: task.task_id.clone(),
                criterion_id: criterion.id.clone(),
                check_ids: criterion.check_ids.clone(),
            })
        })
        .collect();
    Ok(NativeIntakePreview {
        preview_digest,
        source_digest: Some(plan_preview.source_digest.clone()),
        canonical_digest: Some(canonical_digest),
        task_order: plan_preview.task_order.clone(),
        task_digests: plan_preview.task_digests.clone(),
        acceptance_bindings,
        missing_inputs,
        missing_dependencies,
        missing_authority,
        missing_acceptance_bindings,
        source_identity,
        plan: Some(plan),
        plan_source: Some(source.to_vec()),
    })
}

/// Record only a complete, explicitly selected preview using the registry CAS.
pub fn register_preview(
    store: &Store,
    preview: &NativeIntakePreview,
    selected_preview_digest: &str,
    expected_current_revision: &str,
) -> crate::CliResult<NativeIntakeReceipt> {
    if selected_preview_digest != preview.preview_digest {
        return Err(crate::invalid("selected preview digest is stale"));
    }
    if preview.plan.is_none()
        || !preview.missing_inputs.is_empty()
        || !preview.missing_dependencies.is_empty()
        || !preview.missing_authority.is_empty()
        || !preview.missing_acceptance_bindings.is_empty()
    {
        return Err(crate::invalid("incomplete work order cannot be recorded"));
    }
    let plan = preview
        .plan
        .as_ref()
        .ok_or_else(|| crate::invalid("validated plan is absent"))?;
    let expected = expected_current_revision
        .parse::<u32>()
        .ok()
        .filter(|value| value.to_string() == expected_current_revision)
        .ok_or_else(|| crate::invalid("expected revision must be canonical u32"))?;
    if expected.checked_add(1) != Some(plan.revision) {
        return Err(crate::invalid(
            "plan revision must advance expected current revision by one",
        ));
    }
    let source_identity_digest = content_digest(&preview.source_identity)?;
    let command_id = format!(
        "intake-{}",
        source_identity_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| crate::invalid("unexpected source identity digest"))?
    );
    let request = RegisterWorkplanRequest {
        command_id,
        project_id: plan.project_id.clone(),
        plan_id: plan.plan_id.clone(),
        expected_current_revision: expected_current_revision.to_owned(),
        source: preview
            .plan_source
            .clone()
            .ok_or_else(|| crate::invalid("canonical workplan source is absent"))?,
        source_digest: preview
            .source_digest
            .clone()
            .ok_or_else(|| crate::invalid("workplan source digest is absent"))?,
        canonical_digest: preview
            .canonical_digest
            .clone()
            .ok_or_else(|| crate::invalid("canonical workplan digest is absent"))?,
    };
    let workplan_receipt = store.register_workplan(request)?;
    Ok(NativeIntakeReceipt {
        source_identity: preview.source_identity.clone(),
        workplan_receipt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId};
    use serde_json::json;
    use tempfile::{Builder, TempDir};
    use std::os::unix::fs::PermissionsExt;
    use std::fs;

    fn private_tempdir() -> TempDir {
        let directory = Builder::new()
            .prefix("native-intake-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("private test directory is created under the CLI crate");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("test directory is private");
        directory
    }

    fn identity() -> SourceIdentity {
        SourceIdentity {
            producer_id: "external-work-order-feed".into(),
            incarnation: "incarnation-1".into(),
            sequence: "1".into(),
        }
    }

    fn preview(source: &[u8]) -> crate::CliResult<NativeIntakePreview> {
        preview_work_order(source, identity())
    }

    fn contract(task: &str, dependencies: &[&str], owned_path: &str) -> admiralty_core::TaskContract {
        admiralty_core::TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("native-project").expect("project fixture is valid"),
            root_id: RootId::new("native-root").expect("root fixture is valid"),
            task_id: TaskId::new(task).expect("task fixture is valid"),
            revision: 1,
            title: format!("Implement {task}"),
            objective: format!("Deliver bounded outcome {task}"),
            non_goals: vec!["Do not start execution".into()],
            owned_paths: vec![owned_path.into()],
            forbidden_paths: vec![],
            read_first: vec!["README.md".into()],
            dependencies: dependencies
                .iter()
                .map(|dependency| TaskId::new(*dependency).expect("dependency fixture is valid"))
                .collect(),
            criteria: vec![AcceptanceCriterion {
                id: format!("{task}-criterion"),
                text: "The named check passes".into(),
                check_ids: vec![format!("{task}-check")],
            }],
            checks: vec![CheckSpec {
                id: format!("{task}-check"),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 30,
            }],
            authority_ref: "authority/native-project".into(),
            max_attempts: 1,
            simulated: false,
        }
    }

    fn source(tasks: Vec<admiralty_core::TaskContract>) -> Vec<u8> {
        serde_json::to_vec(&Workplan {
            schema_version: 1,
            plan_id: "native-plan".into(),
            revision: 1,
            project_id: ProjectId::new("native-project").expect("project fixture is valid"),
            tasks,
        })
        .expect("work order fixture serializes")
    }

    #[test]
    fn previews_registers_reopens_and_replays_exact_source_identity() {
        let source = source(vec![contract("dependent", &["base"], "src/dependent"), contract("base", &[], "src/base")]);
        let first = preview(&source).expect("work order preview succeeds");
        let replay = preview(&source).expect("identical preview succeeds");
        assert_eq!(first.preview_digest, replay.preview_digest);
        assert_eq!(first.task_order, ["base", "dependent"].map(|id| TaskId::new(id).expect("valid task ID")));
        assert_eq!(first.task_digests, replay.task_digests);
        assert!(first.missing_inputs.is_empty());
        assert!(first.missing_dependencies.is_empty());
        assert!(first.missing_authority.is_empty());
        assert_eq!(first.task_digests.len(), 2);
        assert_eq!(first.acceptance_bindings.len(), 2);

        let directory = private_tempdir();
        let database = directory.path().join("intake.sqlite");
        let store = Store::open(&database).expect("private test store opens");
        let recorded = register_preview(&store, &first, &first.preview_digest, "0")
            .expect("selected preview registers through Store");
        let duplicate = register_preview(&store, &replay, &replay.preview_digest, "0")
            .expect("exact source retry returns the original registration");
        assert_eq!(recorded.workplan_receipt, duplicate.workplan_receipt);
        let receipt_json = serde_json::to_value(&recorded.workplan_receipt).expect("receipt serializes");
        assert_eq!(receipt_json["execution_granted"], false);
        assert_eq!(receipt_json["acceptance_granted"], false);
        assert_eq!(store.snapshot().expect("snapshot is readable").cursor.event_seq, "0");
        drop(store);

        let reopened = Store::open(&database).expect("registered store reopens");
        let registered = reopened
            .workplan(ProjectId::new("native-project").expect("project ID is valid"), "native-plan".into(), None)
            .expect("registry read succeeds")
            .expect("registered plan is durable");
        assert_eq!(registered.plan.tasks.len(), 2);
        assert_eq!(registered.receipt, recorded.workplan_receipt);
    }

    #[test]
    fn stale_preview_conflicting_source_and_stale_revision_fail_closed() {
        let first_source = source(vec![contract("base", &[], "src/base")]);
        let first = preview(&first_source).expect("first preview succeeds");
        let changed_source = source(vec![contract("base", &[], "src/changed")]);
        let changed = preview(&changed_source).expect("changed contract previews");
        let directory = private_tempdir();
        let store = Store::open(directory.path().join("intake.sqlite"))
            .expect("private test store opens");
        assert!(register_preview(&store, &first, "sha256:stale", "0").is_err());
        register_preview(&store, &first, &first.preview_digest, "0")
            .expect("initial plan registers");
        assert!(register_preview(&store, &first, &first.preview_digest, "0").is_ok());
        assert!(register_preview(&store, &first, &first.preview_digest, "1").is_err());
        assert!(register_preview(&store, &changed, &changed.preview_digest, "0").is_err());
        let current = store
            .workplan(
                ProjectId::new("native-project").expect("project ID is valid"),
                "native-plan".into(),
                None,
            )
            .expect("current registry read succeeds")
            .expect("initial revision remains registered");
        assert_eq!(current.plan.revision, 1);
        assert_eq!(current.plan.tasks[0].owned_paths, vec!["src/base".to_owned()]);
    }

    #[test]
    fn reports_missing_inputs_dependencies_authority_and_acceptance_bindings() {
        let mut value: Value = serde_json::from_slice(&source(vec![contract("task", &["absent"], "src/task")]))
            .expect("work order parses as JSON");
        let task = &mut value["tasks"][0];
        task.as_object_mut().expect("task is an object").remove("title");
        task["authority_ref"] = Value::String(String::new());
        task["criteria"][0]["check_ids"] = json!([]);
        let preview = preview(&serde_json::to_vec(&value).expect("damaged input serializes"))
            .expect("incomplete work order produces review diagnostics");
        assert!(preview.missing_inputs.contains(&"plan.tasks[0].title".into()));
        assert_eq!(preview.missing_dependencies, ["task->absent"]);
        assert_eq!(preview.missing_authority, ["task"]);
        assert_eq!(preview.missing_acceptance_bindings, ["task"]);
        let directory = private_tempdir();
        let store = Store::open(directory.path().join("intake.sqlite"))
            .expect("private test store opens");
        assert!(register_preview(&store, &preview, &preview.preview_digest, "0").is_err());
        assert_eq!(store.snapshot().expect("snapshot is readable").cursor.event_seq, "0");
    }

    #[test]
    fn malformed_cross_project_cycles_freeform_done_and_unowned_paths_reject() {
        assert!(preview(b"{").is_err());
        let cross_project = source(vec![contract("task", &[], "src/task")]);
        let mut value: Value = serde_json::from_slice(&cross_project).expect("fixture JSON parses");
        value["tasks"][0]["project_id"] = json!("foreign-project");
        assert!(preview(&serde_json::to_vec(&value).expect("fixture serializes")).is_err());

        let cycle = source(vec![contract("a", &["b"], "src/a"), contract("b", &["a"], "src/b")]);
        assert!(preview(&cycle).is_err());

        let freeform_done = source(vec![contract("task", &[], "src/task")]);
        let mut value: Value = serde_json::from_slice(&freeform_done).expect("fixture JSON parses");
        value["tasks"][0]["done"] = json!("merged and complete");
        assert!(preview(&serde_json::to_vec(&value).expect("fixture serializes")).is_err());

        let unowned = source(vec![contract("task", &[], "../outside")]);
        assert!(preview(&unowned).is_err());
    }
}
