use crate::{ProjectId, RootId, TaskId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

pub const CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckSpec {
    pub id: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub timeout_secs: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
    pub check_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskContract {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub revision: u32,
    pub title: String,
    pub objective: String,
    pub non_goals: Vec<String>,
    pub owned_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub read_first: Vec<String>,
    pub dependencies: Vec<TaskId>,
    pub criteria: Vec<AcceptanceCriterion>,
    pub checks: Vec<CheckSpec>,
    pub authority_ref: String,
    pub max_attempts: u16,
    pub simulated: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid task contract: {0}")]
pub struct ContractError(pub String);

pub fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0'])
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && !value.split('/').any(|p| p == "..")
}

impl TaskContract {
    pub fn validate(&self) -> Result<(), ContractError> {
        let fail = |s: &str| ContractError(s.to_owned());
        if self.schema_version != CONTRACT_VERSION {
            return Err(fail("unsupported schema version"));
        }
        if self.revision == 0
            || self.title.trim().is_empty()
            || self.objective.trim().is_empty()
            || self.authority_ref.trim().is_empty()
            || self.max_attempts == 0
        {
            return Err(fail(
                "revision, title, objective, authority and attempt budget are required",
            ));
        }
        if self.owned_paths.is_empty() || self.criteria.is_empty() || self.checks.is_empty() {
            return Err(fail(
                "write scope, acceptance criteria and named checks are required",
            ));
        }
        for path in self
            .owned_paths
            .iter()
            .chain(&self.forbidden_paths)
            .chain(&self.read_first)
        {
            if !safe_relative_path(path) {
                return Err(fail(
                    "scope paths must remain relative to the owned repository",
                ));
            }
        }
        let check_ids: BTreeSet<_> = self.checks.iter().map(|c| c.id.as_str()).collect();
        let criterion_ids: BTreeSet<_> = self.criteria.iter().map(|c| c.id.as_str()).collect();
        if check_ids.len() != self.checks.len() || criterion_ids.len() != self.criteria.len() {
            return Err(fail("duplicate check or criterion ID"));
        }
        for c in &self.checks {
            if c.id.is_empty()
                || c.program.trim().is_empty()
                || c.program.contains('\0')
                || c.timeout_secs == 0
                || !safe_relative_path(&c.cwd)
                || c.args.iter().any(|a| a.contains('\0'))
            {
                return Err(fail("invalid argv check or timeout"));
            }
        }
        for c in &self.criteria {
            if c.id.is_empty()
                || c.text.trim().is_empty()
                || c.check_ids.is_empty()
                || c.check_ids
                    .iter()
                    .any(|id| !check_ids.contains(id.as_str()))
            {
                return Err(fail("every criterion must bind existing named checks"));
            }
        }
        let deps: BTreeSet<_> = self.dependencies.iter().collect();
        if deps.len() != self.dependencies.len() || deps.contains(&self.task_id) {
            return Err(fail("duplicate or self dependency"));
        }
        Ok(())
    }
}
