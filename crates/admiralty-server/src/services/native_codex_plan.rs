//! Inert two-phase Codex launch composition for the trusted native coordinator.
//!
//! Phase one calls the existing runtime Codex adapter and retains the original
//! descriptor-backed preparation. Phase two binds its exact argv/environment to
//! the caller's current admission and resource authority; it issues no authority,
//! reserves/releases nothing, starts no process, and records no task transition.
//! Model selection/slot proposals are not authenticated decisions. The supplied
//! `CurrentAdmission` must authenticate the committed selection/capacity/context
//! and policy; `ResourcePermitAuthority` must authenticate the exact live permit.
//! NativeTaskLoop rechecks both before effects. Neither help text nor this value
//! proves executable identity, provider observation, or native qualification.

use super::{
    controller::{CurrentAdmission, LaunchRequest},
    native_executor::{NativeExecutionPlan, native_resource_claims_digest},
};
use admiralty_core::{
    SchedulingState, SourceIdentity, TaskView, content_digest, limits::bounded_json_size,
};
use admiralty_runtime::{
    models::ModelSpec,
    providers::codex::{
        self, CodexAdmission, CodexCliObservation, CodexExecutionOptions, PreparedCodexLaunch,
    },
    reservations::Reservation,
    resource_gate::{
        QualifiedEnvironment, ResourcePermitAuthority, ResourcePermitClaims, SystemdBackendConfig,
        VerifiedResourcePermit,
    },
    workspace::{preparation::PreparedWorkspace, registry::WorkspaceSpec},
};
use std::{
    fmt,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// All references describe data, never grants. CLI observations must originate
/// from the registered trusted collector, not a worker JSON/help-text payload.
pub struct CodexPlanInput<'a> {
    pub task: &'a TaskView,
    pub admission: &'a CodexAdmission,
    pub model: &'a ModelSpec,
    pub options: CodexExecutionOptions,
    pub cli: &'a CodexCliObservation,
    pub task_capsule: &'a str,
    /// Canonical `content_digest(task_capsule)` (JSON string encoding), not the
    /// Store role-render raw UTF-8 digest. The caller authenticates that source
    /// render separately, and authenticates this exact launch-context digest.
    pub context_digest: &'a str,
    pub policy_digest: &'a str,
    pub source: &'a SourceIdentity,
    pub observed_utc: &'a str,
    pub not_before: u64,
    /// Trusted service-local clock in the CLI observation's tick domain.
    pub now_tick: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexPlanError {
    InvalidInput,
    WorkspaceCustody,
    ProviderPreparation,
    ResourceBinding,
    AdmissionDenied,
    PermitDenied,
    Clock,
}
impl fmt::Display for CodexPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "native Codex input binding differs or exceeds bounds",
            Self::WorkspaceCustody => "native Codex original workspace custody differs",
            Self::ProviderPreparation => "registered Codex adapter rejected preparation",
            Self::ResourceBinding => "native Codex resource/reservation binding differs",
            Self::AdmissionDenied => "trusted current Codex admission denied",
            Self::PermitDenied => "trusted current Codex resource permit denied",
            Self::Clock => "native Codex clock is unavailable or lease expired",
        })
    }
}
impl std::error::Error for CodexPlanError {}

#[must_use = "retain the original workspace on preparation rejection"]
pub struct CodexPreparationFailure {
    error: CodexPlanError,
    workspace: Box<PreparedWorkspace>,
}
impl CodexPreparationFailure {
    pub fn into_parts(self) -> (CodexPlanError, PreparedWorkspace) {
        (self.error, *self.workspace)
    }
}
impl fmt::Debug for CodexPreparationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Non-Deserialize/non-Clone custody container, not an execution capability.
pub struct PreparedNativeCodexPlan {
    task: TaskView,
    workspace: PreparedWorkspace,
    provider: PreparedCodexLaunch,
    request: LaunchRequest,
    prepared_tick: u64,
}
impl PreparedNativeCodexPlan {
    pub fn prepared_provider(&self) -> &PreparedCodexLaunch {
        &self.provider
    }
    pub fn prepared_workspace(&self) -> &PreparedWorkspace {
        &self.workspace
    }
    pub fn launch_request(&self) -> &LaunchRequest {
        &self.request
    }

    /// Every rejection returns this original container and every resource input.
    /// No cancellation, release, claim, launch, or safe retry is inferred.
    pub fn bind_resources(
        self,
        resources: CodexResourceBinding,
        admission: &impl CurrentAdmission,
    ) -> Result<BoundNativeCodexPlan, CodexBindingFailure> {
        if let Err(error) = self.validate_resources(&resources, admission) {
            return Err(CodexBindingFailure {
                error,
                prepared: Box::new(self),
                resources: Box::new(resources),
            });
        }
        let CodexResourceBinding {
            claims,
            reservation,
            authority,
            environment,
            backend,
            ..
        } = resources;
        let checked = self.workspace.workspace();
        let workspace = WorkspaceSpec {
            node_id: checked.node_id.clone(),
            root_id: checked.root_id.clone(),
            project_id: checked.project_id.clone(),
            workspace: checked.workspace.clone(),
            owned_paths: checked.owned_paths.clone(),
            forbidden_paths: checked.forbidden_paths.clone(),
        };
        let plan = NativeExecutionPlan {
            task_id: self.request.task_id.clone(),
            attempt_id: self.request.attempt_id.clone(),
            contract_digest: self.request.expected_contract_digest.clone(),
            context_digest: self.request.context_digest.clone(),
            policy_digest: self.request.policy_digest.clone(),
            provider_declared: self.request.provider.clone(),
            // Registry id and actual provider model remain separately bound in
            // the provider tuple. Neither becomes an observed model identity.
            model_declared: self.request.model.clone(),
            run_id: self.request.run_id.clone(),
            launch_spec: self.provider.launch_spec().clone(),
            prepared_workspace: self.workspace,
            workspace,
            reservation,
            permit_claims: claims,
            permit_authority: authority,
        };
        Ok(BoundNativeCodexPlan {
            plan,
            request: self.request,
            provider: self.provider,
            environment,
            backend,
        })
    }
    fn validate_resources(
        &self,
        r: &CodexResourceBinding,
        admission: &impl CurrentAdmission,
    ) -> Result<(), CodexPlanError> {
        if r.backend
            .runtime_directory
            .as_os_str()
            .as_encoded_bytes()
            .len()
            > 4096
            || !bounded(&r.backend.delegated_slice, 256)
        {
            return Err(CodexPlanError::ResourceBinding);
        }
        self.workspace
            .validate_current_custody()
            .map_err(|_| CodexPlanError::WorkspaceCustody)?;
        let t = self.provider.tuple();
        let c = &r.claims;
        let s = &r.reservation.scope;
        let now = r.now_tick.ok_or(CodexPlanError::Clock)?;
        let admitted = t
            .admitted_tick
            .parse::<u64>()
            .map_err(|_| CodexPlanError::Clock)?;
        let expires = t
            .expires_tick
            .parse::<u64>()
            .map_err(|_| CodexPlanError::Clock)?;
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .ok_or(CodexPlanError::Clock)?;
        if now < admitted
            || now < self.prepared_tick
            || now >= expires
            || c.expires_at_unix_ms <= wall
            || c.lease_expires_at_unix_ms <= wall
        {
            return Err(CodexPlanError::Clock);
        }
        bounded_json_size(&r.reservation, 16 * 1024)
            .map_err(|_| CodexPlanError::ResourceBinding)?;
        if c.identity.project_id != t.scope.project_id
            || c.identity.task_id != t.scope.task_id
            || c.identity.attempt_id != t.scope.attempt_id
            || c.identity.node_id.as_str() != t.node_id
            // Producer incarnation remains in the Codex tuple/run identity;
            // kernel host-boot incarnation is a separate issuer-authenticated
            // namespace, as in LinkedExecutionReservations. Never equate them.
            || !bounded(&c.host_incarnation,256)
            || c.identity.lease_id != t.lease_ref
            || c.permit_id != t.resource_permit_ref
            || c.reservation_ref != r.reservation.id
            || c.launch_spec_digest
                != content_digest(self.provider.launch_spec())
                    .map_err(|_| CodexPlanError::ResourceBinding)?
            || c.worker_environment_digest
                != r.environment
                    .digest()
                    .map_err(|_| CodexPlanError::ResourceBinding)?
            || c.backend_config_digest
                != r.backend
                    .digest()
                    .map_err(|_| CodexPlanError::ResourceBinding)?
            || !bounded(&c.issuer_ref, 256)
            || !bounded(&c.permit_id, 256)
            || !bounded(&c.reservation_ref, 256)
            || c.cpu_set.is_empty()
            || c.cpu_set.len() > 4096
            || c.registered_cpu_scope.len() > 4096
            || !c.cpu_set.is_subset(&c.registered_cpu_scope)
            || c.memory_max_bytes == 0
            || c.max_tasks == 0
            || c.max_concurrency == 0
            || s.project_id != t.scope.project_id
            || s.root_id != t.scope.root_id
            || s.task_id != t.scope.task_id
            || s.attempt_id != t.scope.attempt_id
            || s.owner != t.scope.owner
            || s.incarnation != c.host_incarnation
            || s.contract_revision != t.scope.revision
            || s.contract_digest != t.scope.contract_digest
            || s.authority_ref != t.scope.authority_ref
            || s.resource_claims_digest
                != native_resource_claims_digest(c).map_err(|_| CodexPlanError::ResourceBinding)?
            || r.reservation.revision == 0
            || !r.reservation.outstanding
            || r.reservation.stop_requested
            || r.reservation.original.get() == 0
            || r.reservation.settled.get() >= r.reservation.original.get()
        {
            return Err(CodexPlanError::ResourceBinding);
        }
        // These trusted process-local adapters are supplied by the service.
        // A matching string, registry proposal, or deserialized bool is not one.
        admission
            .authorize_current(&self.task, &self.request)
            .map_err(|_| CodexPlanError::AdmissionDenied)?;
        VerifiedResourcePermit::verify_current(c.clone(), Arc::clone(&r.authority))
            .map_err(|_| CodexPlanError::PermitDenied)?;
        Ok(())
    }
}

pub struct CodexResourceBinding {
    pub claims: ResourcePermitClaims,
    pub reservation: Reservation,
    pub authority: Arc<dyn ResourcePermitAuthority>,
    /// Actual values intended for ResourceGate::configured, not just digests.
    pub environment: QualifiedEnvironment,
    pub backend: SystemdBackendConfig,
    pub now_tick: Option<u64>,
}
#[must_use = "retain original provider/workspace and resource inputs on binding rejection"]
pub struct CodexBindingFailure {
    error: CodexPlanError,
    prepared: Box<PreparedNativeCodexPlan>,
    resources: Box<CodexResourceBinding>,
}
impl CodexBindingFailure {
    pub fn into_parts(
        self,
    ) -> (
        CodexPlanError,
        PreparedNativeCodexPlan,
        CodexResourceBinding,
    ) {
        (self.error, *self.prepared, *self.resources)
    }
}
impl fmt::Debug for CodexBindingFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexBindingFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
/// Still inert; caller must use its exact env/backend in the ResourceGate and
/// hand the original plan/request into the current NativeTaskLoop. Retain the
/// returned provider preparation for CodexNdjsonStream::from_prepared; requested
/// identities remain declarations until actual journal parsing observes them.
pub struct BoundNativeCodexPlan {
    plan: NativeExecutionPlan,
    request: LaunchRequest,
    provider: PreparedCodexLaunch,
    environment: QualifiedEnvironment,
    backend: SystemdBackendConfig,
}
impl BoundNativeCodexPlan {
    pub fn into_parts(
        self,
    ) -> (
        NativeExecutionPlan,
        LaunchRequest,
        PreparedCodexLaunch,
        QualifiedEnvironment,
        SystemdBackendConfig,
    ) {
        (
            self.plan,
            self.request,
            self.provider,
            self.environment,
            self.backend,
        )
    }
}

pub fn prepare_codex_plan(
    workspace: PreparedWorkspace,
    input: CodexPlanInput<'_>,
) -> Result<PreparedNativeCodexPlan, CodexPreparationFailure> {
    let prepared_tick = match input.now_tick {
        Some(tick) => tick,
        None => {
            return Err(CodexPreparationFailure {
                error: CodexPlanError::Clock,
                workspace: Box::new(workspace),
            });
        }
    };
    let prepare = || -> Result<PreparedCodexLaunch, CodexPlanError> {
        if input.task_capsule.is_empty() || input.task_capsule.len() > 64 * 1024 {
            return Err(CodexPlanError::InvalidInput);
        }
        let a = input.admission;
        bounded_json_size(
            &(
                &a.scope,
                &a.registry_revision,
                &a.selected_model_digest,
                &a.cli_observation_digest,
                &a.node_id,
                &a.working_dir,
                &a.spool_root,
                &a.lease_ref,
                &a.resource_permit_ref,
                &a.custody_ref,
                &a.auth_context_ref,
            ),
            16 * 1024,
        )
        .map_err(|_| CodexPlanError::InvalidInput)?;
        bounded_json_size(input.task, 256 * 1024).map_err(|_| CodexPlanError::InvalidInput)?;
        let task = input.task;
        let contract = &task.contract;
        let ws = workspace.workspace();
        let owned: Vec<PathBuf> = contract.owned_paths.iter().map(PathBuf::from).collect();
        let forbidden: Vec<PathBuf> = contract.forbidden_paths.iter().map(PathBuf::from).collect();
        workspace
            .validate_current_custody()
            .map_err(|_| CodexPlanError::WorkspaceCustody)?;
        if task.state != SchedulingState::Ready
            || task.attempt_id.is_some()
            || task.owner.is_some()
            || task.candidate_ref.is_some()
            || ws.project_id != contract.project_id
            || ws.root_id != contract.root_id
            || workspace.task_id() != &contract.task_id
            || workspace.attempt_id() != &input.admission.scope.attempt_id
            || ws.node_id.as_str() != input.admission.node_id
            || ws.workspace != input.admission.working_dir
            || ws.owned_paths != owned
            || ws.forbidden_paths != forbidden
            || !digest(input.context_digest)
            || !digest(input.policy_digest)
            || content_digest(&input.task_capsule).map_err(|_| CodexPlanError::InvalidInput)?
                != input.context_digest
            || input
                .source
                .validate()
                .ok()
                .is_none_or(|seq| seq > i64::MAX as u64)
            || !bounded(&input.source.producer_id, 256)
            || !bounded(&input.source.incarnation, 256)
            || !bounded(input.observed_utc, 128)
            || input.not_before > i64::MAX as u64
        {
            return Err(CodexPlanError::InvalidInput);
        }
        codex::prepare_launch(
            contract,
            input.admission.clone(),
            input.model,
            input.options,
            input.cli,
            input.task_capsule,
            input.now_tick,
        )
        .map_err(|_| CodexPlanError::ProviderPreparation)
    };
    let provider = match prepare() {
        Ok(provider) => provider,
        Err(error) => {
            return Err(CodexPreparationFailure {
                error,
                workspace: Box::new(workspace),
            });
        }
    };
    let request = LaunchRequest {
        source: input.source.clone(),
        task_id: input.task.contract.task_id.clone(),
        expected_contract_digest: input.admission.scope.contract_digest.clone(),
        attempt_id: input.admission.scope.attempt_id.clone(),
        owner: input.admission.scope.owner.clone(),
        run_id: provider.launch_spec().run_id.clone(),
        context_digest: input.context_digest.into(),
        policy_digest: input.policy_digest.into(),
        provider: "codex_cli".into(),
        model: input.model.id.clone(),
        observed_utc: input.observed_utc.into(),
        not_before: input.not_before,
    };
    Ok(PreparedNativeCodexPlan {
        task: input.task.clone(),
        workspace,
        provider,
        request,
        prepared_tick,
    })
}
fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
