//! Versioned canonical execution requirements contract (`P02` / `MASTER_PLAN` §6.1).
//!
//! Binds base commit, environment/runner requirements, environment allowlist,
//! explicit shell interpreter trust boundary, deadline, responsible role, and
//! required delivery stage to the closed v1 `TaskContract` without mutating
//! v1 wire compatibility.

use crate::{
    ProjectId, RootId, TaskContract, TaskId, content_digest,
    limits::bounded_json_size,
    placement::{BudgetRequirement, PlacementRequest},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const EXECUTION_REQUIREMENTS_SCHEMA_VERSION: u32 = 1;
pub const MAX_EXECUTION_REQUIREMENTS_JSON_BYTES: usize = 128 * 1024;
pub const MAX_CAPABILITIES: usize = 64;
pub const MAX_LOCALITY_REFS: usize = 64;
pub const MAX_ENV_ALLOWLIST: usize = 64;
pub const MAX_ENV_VAR_NAME_BYTES: usize = 128;
pub const MAX_LABEL_BYTES: usize = 256;
pub const MAX_WALL_CLOCK_SECS: u32 = 7 * 86_400;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRunnerMode {
    TrustedLocalWorktree,
    PrivateCloneRootlessOci,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponsibleRole {
    Worker,
    Verifier,
    Auditor,
    Captain,
    SuperCaptain,
    Owner,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum RequiredDeliveryBoundary {
    SourceOnly,
    VerifiedCandidate,
    MergedOnMain,
    Packaged,
    Staged,
    CanaryQualified,
    ReleasedAndRunning,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellBoundaryPolicy {
    ForbidShell,
    DeclaredInterpreter {
        interpreter_program: String,
        trust_boundary_ref: String,
        justification: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRunnerRequirements {
    pub runner_mode: ExecutionRunnerMode,
    pub required_capabilities: Vec<String>,
    pub locality_refs: Vec<String>,
    pub budget_requirement: BudgetRequirement,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRequirementsV2 {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub base_commit_sha: String,
    pub runner: ExecutionRunnerRequirements,
    pub environment_allowlist: Vec<String>,
    pub shell_boundary: ShellBoundaryPolicy,
    pub deadline_utc: String,
    pub max_wall_clock_secs: u32,
    pub responsible_role: ResponsibleRole,
    pub required_delivery_stage: RequiredDeliveryBoundary,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRequirementsCapsuleBinding {
    pub schema_version: u32,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub requirements_digest: String,
    pub placement_request_digest: String,
    pub base_commit_sha: String,
    pub responsible_role: ResponsibleRole,
    pub required_delivery_stage: RequiredDeliveryBoundary,
    pub rendered_summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionRequirementsError {
    #[error("execution requirements exceed size or collection bounds")]
    BoundExceeded,
    #[error("invalid execution requirements: {0}")]
    Invalid(&'static str),
    #[error("execution requirements do not match task contract: {0}")]
    ContractMismatch(&'static str),
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_label(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

fn valid_env_var_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_ENV_VAR_NAME_BYTES {
        return false;
    }
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_uppercase() || first == b'_') {
        return false;
    }
    if !bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    ![
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PRIVATE_KEY",
        "API_KEY",
        "CREDENTIAL",
        "BEARER",
    ]
    .iter()
    .any(|forbidden| upper.contains(forbidden))
}

fn is_shell_program(program: &str) -> bool {
    let base = program.rsplit('/').next().unwrap_or(program);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "csh" | "tcsh"
    )
}

impl ExecutionRequirementsV2 {
    pub fn validate(&self) -> Result<(), ExecutionRequirementsError> {
        bounded_json_size(self, MAX_EXECUTION_REQUIREMENTS_JSON_BYTES)
            .map_err(|_| ExecutionRequirementsError::BoundExceeded)?;
        if self.schema_version != EXECUTION_REQUIREMENTS_SCHEMA_VERSION {
            return Err(ExecutionRequirementsError::Invalid(
                "unsupported schema_version",
            ));
        }
        if self.contract_revision == 0 {
            return Err(ExecutionRequirementsError::Invalid(
                "contract_revision must be positive",
            ));
        }
        if !valid_sha256_digest(&self.contract_digest) {
            return Err(ExecutionRequirementsError::Invalid(
                "invalid contract_digest",
            ));
        }
        if !valid_git_oid(&self.base_commit_sha) {
            return Err(ExecutionRequirementsError::Invalid(
                "invalid base_commit_sha",
            ));
        }
        if !valid_label(&self.deadline_utc, 64)
            || !self.deadline_utc.ends_with('Z')
            || self.deadline_utc.parse::<jiff::Timestamp>().is_err()
        {
            return Err(ExecutionRequirementsError::Invalid(
                "deadline_utc must be a bounded UTC timestamp ending in Z",
            ));
        }
        if self.max_wall_clock_secs == 0 || self.max_wall_clock_secs > MAX_WALL_CLOCK_SECS {
            return Err(ExecutionRequirementsError::Invalid(
                "max_wall_clock_secs outside permitted range",
            ));
        }
        if self.runner.required_capabilities.len() > MAX_CAPABILITIES
            || self.runner.locality_refs.len() > MAX_LOCALITY_REFS
            || self.environment_allowlist.len() > MAX_ENV_ALLOWLIST
        {
            return Err(ExecutionRequirementsError::BoundExceeded);
        }
        let mut cap_set = BTreeSet::new();
        for cap in &self.runner.required_capabilities {
            if !valid_label(cap, MAX_LABEL_BYTES) || !cap_set.insert(cap.as_str()) {
                return Err(ExecutionRequirementsError::Invalid(
                    "invalid or duplicate capability in required_capabilities",
                ));
            }
        }
        let mut loc_set = BTreeSet::new();
        for loc in &self.runner.locality_refs {
            if !valid_label(loc, MAX_LABEL_BYTES) || !loc_set.insert(loc.as_str()) {
                return Err(ExecutionRequirementsError::Invalid(
                    "invalid or duplicate locality_ref",
                ));
            }
        }
        if !valid_label(
            &self.runner.budget_requirement.account_pool,
            MAX_LABEL_BYTES,
        ) || self
            .runner
            .budget_requirement
            .amount
            .parse()
            .map_err(|_| ExecutionRequirementsError::Invalid("invalid budget amount"))?
            == 0
        {
            return Err(ExecutionRequirementsError::Invalid(
                "budget_requirement requires non-empty account_pool and positive amount",
            ));
        }
        let mut env_set = BTreeSet::new();
        for var in &self.environment_allowlist {
            if !valid_env_var_name(var) || !env_set.insert(var.as_str()) {
                return Err(ExecutionRequirementsError::Invalid(
                    "environment_allowlist contains invalid, duplicate, or secret-bearing variable name",
                ));
            }
        }
        if let ShellBoundaryPolicy::DeclaredInterpreter {
            interpreter_program,
            trust_boundary_ref,
            justification,
        } = &self.shell_boundary
            && (!is_shell_program(interpreter_program)
                || !valid_label(interpreter_program, MAX_LABEL_BYTES)
                || !valid_label(trust_boundary_ref, MAX_LABEL_BYTES)
                || !valid_label(justification, 512))
        {
            return Err(ExecutionRequirementsError::Invalid(
                "declared shell interpreter requires valid shell program, trust_boundary_ref, and justification",
            ));
        }
        Ok(())
    }

    pub fn validate_against_contract(
        &self,
        contract: &TaskContract,
    ) -> Result<String, ExecutionRequirementsError> {
        self.validate()?;
        contract
            .validate()
            .map_err(|_| ExecutionRequirementsError::ContractMismatch("invalid task contract"))?;
        if self.project_id != contract.project_id
            || self.root_id != contract.root_id
            || self.task_id != contract.task_id
            || self.contract_revision != contract.revision
        {
            return Err(ExecutionRequirementsError::ContractMismatch(
                "identity or revision differs from TaskContract",
            ));
        }
        let actual_contract_digest = content_digest(contract)
            .map_err(|_| ExecutionRequirementsError::Invalid("cannot digest TaskContract"))?;
        if self.contract_digest != actual_contract_digest {
            return Err(ExecutionRequirementsError::ContractMismatch(
                "contract_digest differs from TaskContract",
            ));
        }
        for check in &contract.checks {
            if check.timeout_secs > self.max_wall_clock_secs {
                return Err(ExecutionRequirementsError::ContractMismatch(
                    "check timeout_secs exceeds max_wall_clock_secs",
                ));
            }
            let check_uses_shell = is_shell_program(&check.program);
            match &self.shell_boundary {
                ShellBoundaryPolicy::ForbidShell => {
                    if check_uses_shell {
                        return Err(ExecutionRequirementsError::ContractMismatch(
                            "shell check program forbidden when shell_boundary is forbid_shell",
                        ));
                    }
                }
                ShellBoundaryPolicy::DeclaredInterpreter {
                    interpreter_program,
                    ..
                } => {
                    if check_uses_shell && &check.program != interpreter_program {
                        return Err(ExecutionRequirementsError::ContractMismatch(
                            "shell check program does not match declared interpreter_program",
                        ));
                    }
                }
            }
        }
        content_digest(self).map_err(|_| {
            ExecutionRequirementsError::Invalid("cannot digest ExecutionRequirementsV2")
        })
    }

    pub fn to_placement_request(
        &self,
        contract: &TaskContract,
    ) -> Result<PlacementRequest, ExecutionRequirementsError> {
        let _ = self.validate_against_contract(contract)?;
        Ok(PlacementRequest {
            project: self.project_id.clone(),
            root: self.root_id.clone(),
            task: self.task_id.clone(),
            contract_digest: self.contract_digest.clone(),
            authority_ref: contract.authority_ref.clone(),
            required_capabilities: self.runner.required_capabilities.clone(),
            locality_refs: self.runner.locality_refs.clone(),
            budget_requirement: self.runner.budget_requirement.clone(),
        })
    }

    pub fn compile_capsule_binding(
        &self,
        contract: &TaskContract,
    ) -> Result<ExecutionRequirementsCapsuleBinding, ExecutionRequirementsError> {
        let requirements_digest = self.validate_against_contract(contract)?;
        let placement = self.to_placement_request(contract)?;
        let placement_request_digest = content_digest(&placement)
            .map_err(|_| ExecutionRequirementsError::Invalid("cannot digest PlacementRequest"))?;
        let rendered_summary = format!(
            "task={}@{} base={} runner={:?} role={:?} delivery={:?} deadline={} max_wall_clock_secs={} env_allowlist=[{}] req_digest={}",
            self.task_id.as_str(),
            self.contract_revision,
            self.base_commit_sha,
            self.runner.runner_mode,
            self.responsible_role,
            self.required_delivery_stage,
            self.deadline_utc,
            self.max_wall_clock_secs,
            self.environment_allowlist.join(","),
            requirements_digest,
        );
        Ok(ExecutionRequirementsCapsuleBinding {
            schema_version: EXECUTION_REQUIREMENTS_SCHEMA_VERSION,
            task_id: self.task_id.clone(),
            contract_revision: self.contract_revision,
            contract_digest: self.contract_digest.clone(),
            requirements_digest,
            placement_request_digest,
            base_commit_sha: self.base_commit_sha.clone(),
            responsible_role: self.responsible_role,
            required_delivery_stage: self.required_delivery_stage,
            rendered_summary,
        })
    }
}
