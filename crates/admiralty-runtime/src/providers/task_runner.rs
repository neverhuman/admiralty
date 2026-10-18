//! Configured task-model selection and provider command preparation.
//!
//! This facade only returns a prepared command. It never launches a process,
//! reads credentials, performs provider inference, or treats a prepared command
//! as execution/resource admission.

use super::{codex, opencode_ssh};
use crate::models::{
    CapacitySnapshot, ModelRegistry, ModelSpec, Provider, SelectionOutcome, SelectionRequest,
    WaitReason,
};
use crate::session::LaunchSpec;
use admiralty_core::context::ContextCapsule;
use admiralty_core::{TaskContract, content_digest};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TaskRunnerError {
    #[error("task contract or context binding is invalid")]
    InvalidTask,
    #[error("model registry did not select a runnable configured model: {0:?}")]
    Waiting(WaitReason),
    #[error("selected model is not in this registry revision")]
    SelectionDrift,
    #[error("provider preparation input does not match the selected configured provider")]
    ProviderMismatch,
    #[error("provider setup or authentication requires repair before preparation")]
    RepairRequired,
    #[error("provider preparation failed: {0}")]
    Provider(String),
    #[error("provider executable is not an absolute configured allowlisted path")]
    ProgramNotAllowlisted,
}

#[derive(Clone, Debug)]
pub struct SelectedTaskModel {
    pub registry_revision: String,
    pub model: ModelSpec,
}

/// The observed node's absolute executable allowlist. Paths are compared as
/// configured; this check does not attest file contents or filesystem custody.
#[derive(Clone, Debug, Default)]
pub struct CliAllowlist {
    programs: BTreeSet<PathBuf>,
}

impl CliAllowlist {
    pub fn new(programs: impl IntoIterator<Item = PathBuf>) -> Result<Self, TaskRunnerError> {
        let programs: BTreeSet<_> = programs.into_iter().collect();
        if programs.is_empty() || programs.iter().any(|path| !normalized_absolute(path)) {
            return Err(TaskRunnerError::ProgramNotAllowlisted);
        }
        Ok(Self { programs })
    }

    fn contains(&self, path: &Path) -> bool {
        normalized_absolute(path) && self.programs.contains(path)
    }
}

#[derive(Clone, Debug)]
pub struct TaskIdentity {
    /// Immutable source work-order identity supplied by the caller.
    pub source_ref: String,
    pub attempt_id: String,
    pub attempt_ordinal: u16,
}

pub struct TaskPreparation<'a> {
    pub registry: &'a ModelRegistry,
    pub capacity: &'a CapacitySnapshot,
    pub selection: &'a SelectionRequest,
    pub contract: &'a TaskContract,
    pub context: &'a ContextCapsule,
    pub identity: &'a TaskIdentity,
    pub working_dir: &'a Path,
    pub spool_dir: &'a Path,
    pub cli_allowlist: &'a CliAllowlist,
}

/// Provider-specific inputs already observed/admitted by their owning modules.
/// These inputs never cause a provider call; they only produce a LaunchSpec.
pub enum ProviderPreparation<'a> {
    Codex {
        admission: Box<codex::CodexAdmission>,
        options: codex::CodexExecutionOptions,
        cli: &'a codex::CodexCliObservation,
        now: Option<u64>,
    },
    OpenCodeSsh(&'a opencode_ssh::OpenCodeSshProfile),
}

#[derive(Clone, Debug)]
pub struct PreparedTaskCommand {
    pub registry_revision: String,
    pub model_id: String,
    pub provider_model: String,
    pub source_ref: String,
    pub contract_digest: String,
    pub attempt_id: String,
    pub attempt_ordinal: u16,
    pub context_epoch: String,
    pub launch_spec: LaunchSpec,
}

impl PreparedTaskCommand {
    /// Preparation is intentionally not an execution or resource grant.
    pub fn may_launch(&self) -> bool {
        // This facade has no validator-issued launch permit type yet.
        false
    }
}

pub fn select_model(
    registry: &ModelRegistry,
    request: &SelectionRequest,
    capacity: &CapacitySnapshot,
) -> Result<SelectedTaskModel, TaskRunnerError> {
    let outcome = registry
        .select(request, capacity)
        .map_err(|_| TaskRunnerError::RepairRequired)?;
    let SelectionOutcome::Selected { model_id, .. } = outcome else {
        let SelectionOutcome::Waiting { reason, .. } = outcome else {
            unreachable!("selection outcome variants are exhaustive")
        };
        return Err(TaskRunnerError::Waiting(reason));
    };
    let model = registry
        .models
        .iter()
        .find(|model| model.id == model_id && model.enabled)
        .cloned()
        .ok_or(TaskRunnerError::SelectionDrift)?;
    Ok(SelectedTaskModel {
        registry_revision: registry.revision.clone(),
        model,
    })
}

pub fn prepare(
    task: TaskPreparation<'_>,
    selected: &SelectedTaskModel,
    provider: ProviderPreparation<'_>,
) -> Result<PreparedTaskCommand, TaskRunnerError> {
    task.registry
        .validate()
        .map_err(|_| TaskRunnerError::RepairRequired)?;
    task.contract
        .validate()
        .map_err(|_| TaskRunnerError::InvalidTask)?;
    let current_selection = select_model(task.registry, task.selection, task.capacity)?;
    if current_selection.model.id != selected.model.id {
        return Err(TaskRunnerError::SelectionDrift);
    }
    let contract_digest =
        content_digest(task.contract).map_err(|_| TaskRunnerError::InvalidTask)?;
    if selected.registry_revision != task.registry.revision
        || task
            .registry
            .models
            .iter()
            .find(|model| model.id == selected.model.id)
            != Some(&selected.model)
        || !selected.model.enabled
        || selected.model.provider_model.trim().is_empty()
        || task.context.task_id != task.contract.task_id.to_string()
        || task.context.revision != task.contract.revision
        || task.context.contract_digest != contract_digest
        || task.context.text.trim().is_empty()
        || task.identity.source_ref.trim().is_empty()
        || task.identity.source_ref.len() > 512
        || task.identity.source_ref.chars().any(char::is_control)
        || task.identity.attempt_id.trim().is_empty()
        || task.identity.attempt_id.len() > 128
        || !task
            .identity
            .attempt_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        || task.identity.attempt_ordinal == 0
        || task.identity.attempt_ordinal > task.contract.max_attempts
        || !normalized_absolute(task.working_dir)
        || !normalized_absolute(task.spool_dir)
        || task.working_dir == task.spool_dir
        || task.working_dir.starts_with(task.spool_dir)
        || task.spool_dir.starts_with(task.working_dir)
    {
        return Err(TaskRunnerError::InvalidTask);
    }

    let capsule = format!(
        "ADMIRALTY_TASK source_ref={} attempt_id={} attempt_ordinal={} context_epoch={}\n{}",
        serde_json::to_string(&task.identity.source_ref)
            .map_err(|_| TaskRunnerError::InvalidTask)?,
        serde_json::to_string(&task.identity.attempt_id)
            .map_err(|_| TaskRunnerError::InvalidTask)?,
        task.identity.attempt_ordinal,
        serde_json::to_string(&task.context.context_epoch)
            .map_err(|_| TaskRunnerError::InvalidTask)?,
        task.context.text,
    );
    let launch_spec = match (&selected.model.provider, provider) {
        (
            &Provider::CodexCli,
            ProviderPreparation::Codex {
                admission,
                options,
                cli,
                now,
            },
        ) => {
            let prepared = codex::prepare_launch(
                task.contract,
                *admission,
                &selected.model,
                options,
                cli,
                &capsule,
                now,
            )
            .map_err(|error| TaskRunnerError::Provider(error.to_string()))?;
            prepared.into_launch_spec()
        }
        (&Provider::OpencodeSsh, ProviderPreparation::OpenCodeSsh(profile)) => {
            if profile.model_ref != selected.model.provider_model
                || selected
                    .model
                    .node_id
                    .as_deref()
                    .is_some_and(|node| node != profile.node_id)
            {
                return Err(TaskRunnerError::ProviderMismatch);
            }
            let command = profile
                .launch_spec(&capsule)
                .map_err(|error| TaskRunnerError::Provider(error.to_string()))?;
            LaunchSpec {
                run_id: stable_run_id(task.identity, &contract_digest)?,
                program: command.program,
                args: command.args,
                cwd: task.working_dir.to_path_buf(),
                spool_dir: task.spool_dir.to_path_buf(),
            }
        }
        (&Provider::OpenaiCompatible, _) => return Err(TaskRunnerError::RepairRequired),
        _ => return Err(TaskRunnerError::ProviderMismatch),
    };

    if !task.cli_allowlist.contains(&launch_spec.program) {
        return Err(TaskRunnerError::ProgramNotAllowlisted);
    }
    crate::session::validate_launch_envelope(&launch_spec)
        .map_err(|error| TaskRunnerError::Provider(error.to_string()))?;

    Ok(PreparedTaskCommand {
        registry_revision: selected.registry_revision.clone(),
        model_id: selected.model.id.clone(),
        provider_model: selected.model.provider_model.clone(),
        source_ref: task.identity.source_ref.clone(),
        contract_digest,
        attempt_id: task.identity.attempt_id.clone(),
        attempt_ordinal: task.identity.attempt_ordinal,
        context_epoch: task.context.context_epoch.clone(),
        launch_spec,
    })
}

fn stable_run_id(
    identity: &TaskIdentity,
    contract_digest: &str,
) -> Result<String, TaskRunnerError> {
    let digest = content_digest(&(
        &identity.source_ref,
        &identity.attempt_id,
        identity.attempt_ordinal,
        contract_digest,
    ))
    .map_err(|_| TaskRunnerError::InvalidTask)?;
    let suffix = digest
        .strip_prefix("sha256:")
        .ok_or(TaskRunnerError::InvalidTask)?;
    Ok(format!("task-{}", &suffix[..suffix.len().min(48)]))
}

fn normalized_absolute(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .to_str()
            .is_none_or(|value| value.chars().any(char::is_control))
    {
        return false;
    }
    path.components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
        && path.file_name().is_some()
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{
        AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskId,
    };
    use std::collections::BTreeSet;

    fn contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("runner-project").expect("project fixture"),
            root_id: RootId::new("runner-root").expect("root fixture"),
            task_id: TaskId::new("runner-task").expect("task fixture"),
            revision: 1,
            title: "Prepare a bounded task command".into(),
            objective: "Keep selection and preparation deterministic".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "Use configured provider identity".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 10,
            }],
            authority_ref: "authority:runner".into(),
            max_attempts: 2,
            simulated: false,
        }
    }

    fn profile() -> opencode_ssh::OpenCodeSshProfile {
        opencode_ssh::OpenCodeSshProfile {
            node_id: "runner-node".into(),
            ssh_alias: "registered-runner".into(),
            ssh_program: "/usr/bin/ssh".into(),
            remote_program: "/opt/bin/opencode".into(),
            remote_config: Some("/etc/admiralty/opencode.json".into()),
            remote_worktree: "/work/runner-task".into(),
            model_ref: "local/configured-model".into(),
        }
    }

    fn registry() -> ModelRegistry {
        let capabilities = BTreeSet::from(["code".into()]);
        ModelRegistry {
            schema_version: 1,
            revision: "7".into(),
            models: vec![
                ModelSpec {
                    id: "configured-low-tier".into(),
                    label: "Human display label that must not become model identity".into(),
                    provider: Provider::OpencodeSsh,
                    provider_model: "local/configured-model".into(),
                    tier: 1,
                    enabled: true,
                    node_id: Some("runner-node".into()),
                    endpoint: None,
                    credential_file_ref: None,
                    max_concurrency: 1,
                    capabilities: capabilities.clone(),
                    task_max_attempts: 2,
                    escalate_to: Some("configured-high-tier".into()),
                },
                ModelSpec {
                    id: "configured-high-tier".into(),
                    label: "Luna".into(),
                    provider: Provider::OpencodeSsh,
                    provider_model: "local/fallback-model".into(),
                    tier: 9,
                    enabled: true,
                    node_id: Some("runner-node".into()),
                    endpoint: None,
                    credential_file_ref: None,
                    max_concurrency: 1,
                    capabilities,
                    task_max_attempts: 2,
                    escalate_to: None,
                },
            ],
        }
    }

    #[test]
    fn configured_codex_model_uses_observed_cli_and_returns_only_prepared_command() {
        let contract = contract();
        let contract_digest = content_digest(&contract).expect("contract digest");
        let model = ModelSpec {
            id: "configured-codex".into(),
            label: "Display-only label".into(),
            provider: Provider::CodexCli,
            provider_model: "native/configured-model".into(),
            tier: 2,
            enabled: true,
            node_id: Some("runner-node".into()),
            endpoint: None,
            credential_file_ref: None,
            max_concurrency: 1,
            capabilities: BTreeSet::from(["code".into()]),
            task_max_attempts: 2,
            escalate_to: None,
        };
        let registry = ModelRegistry {
            schema_version: 1,
            revision: "8".into(),
            models: vec![model.clone()],
        };
        let mut selection = SelectionRequest::default();
        selection.required_capabilities.insert("code".into());
        let capacity = CapacitySnapshot::default();
        let selected = select_model(&registry, &selection, &capacity).expect("select Codex");
        let cli = codex::CodexCliObservation::from_help(
            "runner-node",
            PathBuf::from("/usr/bin/codex"),
            codex::SUPPORTED_CLI_VERSION,
            "--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd",
            "--no-daemon --ask-for-approval",
            Some(10),
        )
        .expect("fake observed CLI help fixture");
        let admission = codex::CodexAdmission {
            scope: super::super::supervision::SupervisionScope {
                project_id: contract.project_id.clone(),
                root_id: contract.root_id.clone(),
                task_id: contract.task_id.clone(),
                attempt_id: AttemptId::new("attempt-codex").expect("attempt fixture"),
                owner: PrincipalId::new("runner-owner").expect("owner fixture"),
                incarnation: "runner-incarnation".into(),
                authority_ref: contract.authority_ref.clone(),
                contract_digest: contract_digest.clone(),
                revision: contract.revision,
            },
            attempt_ordinal: 1,
            registry_revision: registry.revision.clone(),
            selected_model_digest: content_digest(&model).expect("model digest"),
            cli_observation_digest: cli.fingerprint().into(),
            node_id: "runner-node".into(),
            working_dir: PathBuf::from("/work/codex-task"),
            spool_root: PathBuf::from("/var/spool/codex-task"),
            lease_ref: "lease:codex".into(),
            resource_permit_ref: "permit:codex".into(),
            custody_ref: "custody:codex".into(),
            auth_context_ref: "auth:codex".into(),
            admitted_tick: 10,
            expires_tick: 20,
        };
        let context = ContextCapsule {
            task_id: contract.task_id.to_string(),
            revision: contract.revision,
            context_epoch: "epoch-codex".into(),
            contract_digest,
            text: "bounded Codex context".into(),
            estimated_tokens: 4,
            tokenizer: "fixture".into(),
        };
        let identity = TaskIdentity {
            source_ref: "work-order:codex-1".into(),
            attempt_id: "attempt-codex".into(),
            attempt_ordinal: 1,
        };
        let work = PathBuf::from("/work/codex-task");
        let spool = PathBuf::from("/var/spool/codex-task");
        let allowlist =
            CliAllowlist::new([PathBuf::from("/usr/bin/codex")]).expect("CLI allowlist");
        let prepared = prepare(
            TaskPreparation {
                registry: &registry,
                capacity: &capacity,
                selection: &selection,
                contract: &contract,
                context: &context,
                identity: &identity,
                working_dir: &work,
                spool_dir: &spool,
                cli_allowlist: &allowlist,
            },
            &selected,
            ProviderPreparation::Codex {
                admission: Box::new(admission),
                options: codex::CodexExecutionOptions {
                    effort: codex::CodexEffort::Low,
                    sandbox: codex::CodexSandbox::ReadOnly,
                },
                cli: &cli,
                now: Some(11),
            },
        )
        .expect("prepare configured Codex command");
        assert_eq!(prepared.model_id, "configured-codex");
        assert_eq!(prepared.provider_model, "native/configured-model");
        assert_eq!(
            prepared.launch_spec.program,
            PathBuf::from("/usr/bin/codex")
        );
        assert!(
            prepared
                .launch_spec
                .args
                .contains(&"native/configured-model".into())
        );
        assert!(
            prepared
                .launch_spec
                .args
                .iter()
                .any(|arg| arg.contains("work-order:codex-1"))
        );
        assert!(!prepared.may_launch());
    }

    #[test]
    fn configured_selection_prepares_exact_allowlisted_opencode_argv_without_authority() {
        let contract = contract();
        let digest = content_digest(&contract).expect("contract digest");
        let context = ContextCapsule {
            task_id: contract.task_id.to_string(),
            revision: contract.revision,
            context_epoch: "epoch-3".into(),
            contract_digest: digest,
            text: "bounded test context".into(),
            estimated_tokens: 5,
            tokenizer: "fixture".into(),
        };
        let identity = TaskIdentity {
            source_ref: "work-order:runner-1".into(),
            attempt_id: "attempt-1".into(),
            attempt_ordinal: 1,
        };
        let registry = registry();
        let mut selection = SelectionRequest::default();
        selection.required_capabilities.insert("code".into());
        let capacity = CapacitySnapshot::default();
        let selected =
            select_model(&registry, &selection, &capacity).expect("select configured model");
        assert_eq!(selected.model.id, "configured-low-tier");
        assert_eq!(selected.model.provider_model, "local/configured-model");

        let work = PathBuf::from("/work/runner-task");
        let spool = PathBuf::from("/var/spool/admiralty/runner-task");
        let allowlist = CliAllowlist::new([PathBuf::from("/usr/bin/ssh")]).expect("CLI allowlist");
        let prepared = prepare(
            TaskPreparation {
                registry: &registry,
                capacity: &capacity,
                selection: &selection,
                contract: &contract,
                context: &context,
                identity: &identity,
                working_dir: &work,
                spool_dir: &spool,
                cli_allowlist: &allowlist,
            },
            &selected,
            ProviderPreparation::OpenCodeSsh(&profile()),
        )
        .expect("prepare configured provider command");
        assert_eq!(prepared.model_id, "configured-low-tier");
        assert_eq!(prepared.provider_model, "local/configured-model");
        assert_eq!(prepared.source_ref, identity.source_ref);
        assert_eq!(prepared.attempt_id, identity.attempt_id);
        assert_eq!(prepared.context_epoch, "epoch-3");
        assert_eq!(prepared.launch_spec.program, PathBuf::from("/usr/bin/ssh"));
        assert_eq!(prepared.launch_spec.cwd, work);
        assert_eq!(prepared.launch_spec.spool_dir, spool);
        assert_eq!(prepared.launch_spec.args[0], "-T");
        assert!(prepared.launch_spec.args.last().is_some_and(|arg| {
            arg.contains("local/configured-model") && arg.contains("work-order:runner-1")
        }));
        assert!(!prepared.may_launch());
    }

    #[test]
    fn setup_failure_waits_instead_of_selecting_a_stronger_tier() {
        let registry = registry();
        let mut selection = SelectionRequest {
            current_model: Some("configured-low-tier".into()),
            last_failure: Some(crate::models::FailureReason::SetupError),
            allow_escalation: true,
            allow_capacity_fallback: true,
            ..SelectionRequest::default()
        };
        selection.required_capabilities.insert("code".into());
        assert!(matches!(
            select_model(&registry, &selection, &CapacitySnapshot::default()),
            Err(TaskRunnerError::Waiting(WaitReason::SetupRequired))
        ));
    }
}
