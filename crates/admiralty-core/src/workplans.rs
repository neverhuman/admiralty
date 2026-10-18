//! Bounded, preview-only import of JSON workplans.
use crate::{ProjectId, TaskContract, TaskId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const WORKPLAN_SCHEMA_VERSION: u32 = 1;
pub const MAX_WORKPLAN_BYTES: usize = 1024 * 1024;
pub const MAX_WORKPLAN_TASKS: usize = 1000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workplan {
    pub schema_version: u32,
    pub plan_id: String,
    pub revision: u32,
    pub project_id: ProjectId,
    pub tasks: Vec<TaskContract>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkplanPreview {
    pub plan_id: String,
    pub revision: u32,
    pub project_id: ProjectId,
    pub source_digest: String,
    /// Task IDs in deterministic dependency-first order.
    pub task_order: Vec<TaskId>,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkplanError {
    #[error("workplan exceeds the 1 MiB input limit")]
    TooLarge,
    #[error("invalid workplan JSON: {0}")]
    Json(String),
    #[error("unsupported workplan schema version")]
    UnsupportedVersion,
    #[error("plan ID must be nonempty and revision must be positive")]
    InvalidMetadata,
    #[error("workplan exceeds the 1000 task limit")]
    TooManyTasks,
    #[error("invalid task contract {task_id}: {reason}")]
    InvalidContract { task_id: String, reason: String },
    #[error("duplicate task ID: {0}")]
    DuplicateTaskId(String),
    #[error("task {task_id} belongs to a different project")]
    ProjectMismatch { task_id: String },
    #[error("task {task_id} depends on missing task {dependency}")]
    MissingDependency { task_id: String, dependency: String },
    #[error("workplan dependencies contain a cycle")]
    DependencyCycle,
}

/// Parse and validate an input document without submitting or mutating tasks.
pub fn preview_workplan(source: &[u8]) -> Result<WorkplanPreview, WorkplanError> {
    if source.len() > MAX_WORKPLAN_BYTES {
        return Err(WorkplanError::TooLarge);
    }
    let plan: Workplan =
        serde_json::from_slice(source).map_err(|error| WorkplanError::Json(error.to_string()))?;
    if plan.schema_version != WORKPLAN_SCHEMA_VERSION {
        return Err(WorkplanError::UnsupportedVersion);
    }
    if plan.plan_id.trim().is_empty() || plan.revision == 0 {
        return Err(WorkplanError::InvalidMetadata);
    }
    if plan.tasks.len() > MAX_WORKPLAN_TASKS {
        return Err(WorkplanError::TooManyTasks);
    }

    let mut tasks = BTreeMap::new();
    for task in plan.tasks {
        let id = task.task_id.clone();
        if task.project_id != plan.project_id {
            return Err(WorkplanError::ProjectMismatch {
                task_id: id.to_string(),
            });
        }
        task.validate()
            .map_err(|error| WorkplanError::InvalidContract {
                task_id: id.to_string(),
                reason: error.0,
            })?;
        if tasks.insert(id.clone(), task).is_some() {
            return Err(WorkplanError::DuplicateTaskId(id.to_string()));
        }
    }

    // Kahn's algorithm, using ordered collections for stable tie breaking. Its
    // work is bounded by the task cap and the contracts' serialized input size.
    let mut indegree: BTreeMap<TaskId, usize> = tasks.keys().cloned().map(|id| (id, 0)).collect();
    let mut dependents: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for (id, task) in &tasks {
        for dependency in &task.dependencies {
            if !tasks.contains_key(dependency) {
                return Err(WorkplanError::MissingDependency {
                    task_id: id.to_string(),
                    dependency: dependency.to_string(),
                });
            }
            if let Some(count) = indegree.get_mut(id) {
                *count += 1;
            }
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(id.clone());
        }
    }

    let mut ready: BTreeSet<TaskId> = indegree
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect();
    let mut task_order = Vec::with_capacity(tasks.len());
    while let Some(id) = ready.pop_first() {
        task_order.push(id.clone());
        if let Some(children) = dependents.get(&id) {
            for child in children {
                if let Some(count) = indegree.get_mut(child) {
                    *count -= 1;
                    if *count == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
    }
    if task_order.len() != tasks.len() {
        return Err(WorkplanError::DependencyCycle);
    }

    Ok(WorkplanPreview {
        plan_id: plan.plan_id,
        revision: plan.revision,
        project_id: plan.project_id,
        source_digest: format!("sha256:{:x}", Sha256::digest(source)),
        task_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcceptanceCriterion, CheckSpec, RootId};

    fn contract(id: &str, dependencies: &[&str]) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("project").expect("valid project ID"),
            root_id: RootId::new("root").expect("valid root ID"),
            task_id: TaskId::new(id).expect("valid task ID"),
            revision: 1,
            title: format!("Task {id}"),
            objective: "Complete the task".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: dependencies
                .iter()
                .map(|dependency| TaskId::new(*dependency).expect("valid dependency ID"))
                .collect(),
            criteria: vec![AcceptanceCriterion {
                id: "criterion".into(),
                text: "The named check passes".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "cargo".into(),
                args: vec!["check".into()],
                cwd: ".".into(),
                timeout_secs: 60,
            }],
            authority_ref: "authority".into(),
            max_attempts: 1,
            simulated: true,
        }
    }

    fn source(tasks: Vec<TaskContract>) -> Vec<u8> {
        serde_json::to_vec(&Workplan {
            schema_version: 1,
            plan_id: "plan".into(),
            revision: 1,
            project_id: ProjectId::new("project").expect("valid project ID"),
            tasks,
        })
        .expect("serialize workplan")
    }

    #[test]
    fn produces_deterministic_dependency_first_preview_and_digest() {
        let input = source(vec![
            contract("b", &["a"]),
            contract("c", &[]),
            contract("a", &[]),
        ]);
        let first = preview_workplan(&input).expect("valid plan");
        let replay = preview_workplan(&input).expect("replay valid plan");
        assert_eq!(first, replay);
        assert_eq!(
            first.task_order,
            ["a", "b", "c"].map(|id| TaskId::new(id).expect("id"))
        );
        assert!(first.source_digest.starts_with("sha256:"));
    }

    #[test]
    fn rejects_malformed_json_unknown_fields_and_unsupported_versions() {
        assert!(matches!(
            preview_workplan(b"{"),
            Err(WorkplanError::Json(_))
        ));
        let mut unknown = source(vec![]);
        unknown.pop();
        unknown.extend_from_slice(b",\"extra\":true}");
        assert!(matches!(
            preview_workplan(&unknown),
            Err(WorkplanError::Json(_))
        ));
        let mut unsupported =
            serde_json::from_slice::<serde_json::Value>(&source(vec![])).expect("valid json");
        unsupported["schema_version"] = 2.into();
        let bytes = serde_json::to_vec(&unsupported).expect("serialize");
        assert_eq!(
            preview_workplan(&bytes),
            Err(WorkplanError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_oversized_documents_and_too_many_tasks() {
        assert_eq!(
            preview_workplan(&vec![b' '; MAX_WORKPLAN_BYTES + 1]),
            Err(WorkplanError::TooLarge)
        );
        let plan = Workplan {
            schema_version: 1,
            plan_id: "plan".into(),
            revision: 1,
            project_id: ProjectId::new("project").expect("valid project ID"),
            tasks: (0..=MAX_WORKPLAN_TASKS)
                .map(|index| contract(&format!("task-{index}"), &[]))
                .collect(),
        };
        let bytes = serde_json::to_vec(&plan).expect("serialize");
        assert_eq!(preview_workplan(&bytes), Err(WorkplanError::TooManyTasks));
    }

    #[test]
    fn rejects_duplicate_ids_missing_dependencies_and_cycles() {
        assert!(matches!(
            preview_workplan(&source(vec![contract("a", &[]), contract("a", &[])])),
            Err(WorkplanError::DuplicateTaskId(_))
        ));
        assert!(matches!(
            preview_workplan(&source(vec![contract("a", &["missing"])])),
            Err(WorkplanError::MissingDependency { .. })
        ));
        assert_eq!(
            preview_workplan(&source(vec![contract("a", &["b"]), contract("b", &["a"])])),
            Err(WorkplanError::DependencyCycle)
        );
    }

    #[test]
    fn rejects_contracts_with_another_project_or_invalid_fields() {
        let mut other_project = contract("a", &[]);
        other_project.project_id = ProjectId::new("elsewhere").expect("valid project ID");
        assert!(matches!(
            preview_workplan(&source(vec![other_project])),
            Err(WorkplanError::ProjectMismatch { .. })
        ));

        let mut invalid = contract("a", &[]);
        invalid.title.clear();
        assert!(matches!(
            preview_workplan(&source(vec![invalid])),
            Err(WorkplanError::InvalidContract { .. })
        ));
    }
}
