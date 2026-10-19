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
    controller::{CurrentAdmission, LaunchRequest, planned_launch_intent},
    native_executor::{NativeExecutionPlan, native_resource_claims_digest},
};
use crate::model_selection::{InferenceSlotClaim, model_config_digest};
use admiralty_core::{
    SchedulingState, SourceIdentity, TaskView, content_digest, limits::bounded_json_size,
};
use admiralty_runtime::{
    models::{ModelSpec, Provider},
    providers::codex::{
        self, CodexAdmission, CodexCliObservation, CodexExecutionOptions, PreparedCodexLaunch,
    },
    reservations::{Reservation, ReservationEvent},
    resource_gate::{
        QualifiedEnvironment, ResourcePermitAuthority, ResourcePermitClaims, SystemdBackendConfig,
        VerifiedResourcePermit,
    },
    session::validate_launch_envelope,
    workspace::{
        preparation::PreparedWorkspace,
        registry::{ValidatedWorkspace, WorkspaceSpec},
    },
};
use admiralty_store::{
    BudgetAndSlotAdmission, admission_reservations,
    inference_slots::{self, SlotEvent},
    outbox::intent_digest,
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
    model: ModelSpec,
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
            task: self.task,
            model: self.model,
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
    task: TaskView,
    model: ModelSpec,
    plan: NativeExecutionPlan,
    request: LaunchRequest,
    provider: PreparedCodexLaunch,
    environment: QualifiedEnvironment,
    backend: SystemdBackendConfig,
}
impl BoundNativeCodexPlan {
    /// Pure data binding to the original preparation. This neither authenticates
    /// physical capacity nor installs an execution or admission authority.
    /// Every rejection returns the original plan, inputs, inference claim, and
    /// read capabilities. Ledger reads retain their own explicit capabilities;
    /// write proofs are never substituted for them.
    pub fn bind_inference_slot(
        self,
        inputs: BudgetAndSlotAdmission,
        inference: InferenceSlotClaim,
        reads: NativeCodexAdmissionReadProofs,
    ) -> Result<SlotBoundNativeCodexPlan, CodexSlotBindingFailure> {
        if let Err(error) = self.validate_inference_slot(&inputs, &inference) {
            return Err(CodexSlotBindingFailure {
                error,
                bound: Box::new(self),
                inputs: Box::new(inputs),
                inference: Box::new(inference),
                reads: Box::new(reads),
            });
        }
        // Validation above bounded and checked these exact values. Capture before
        // the original public plan can be extracted or changed by its caller.
        let expected = match FrozenSlotPlan::capture(&self) {
            Ok(expected) => expected,
            Err(error) => {
                return Err(CodexSlotBindingFailure {
                    error,
                    bound: Box::new(self),
                    inputs: Box::new(inputs),
                    inference: Box::new(inference),
                    reads: Box::new(reads),
                });
            }
        };
        Ok(SlotBoundNativeCodexPlan {
            bound: self,
            admission: NativeCodexSlotAdmission {
                inputs,
                reads,
                expected,
            },
        })
    }

    fn validate_inference_slot(
        &self,
        inputs: &BudgetAndSlotAdmission,
        inference: &InferenceSlotClaim,
    ) -> Result<(), CodexPlanError> {
        let invalid = || CodexPlanError::ResourceBinding;
        let plan = &self.plan;
        let request = &self.request;
        let model = &self.model;
        let tuple = self.provider.tuple();
        let contract = &self.task.contract;
        bounded_json_size(model, 16 * 1024).map_err(|_| invalid())?;
        bounded_json_size(inference, 16 * 1024).map_err(|_| invalid())?;
        bounded_json_size(tuple, 256 * 1024).map_err(|_| invalid())?;
        bounded_json_size(&plan.reservation, 16 * 1024).map_err(|_| invalid())?;
        validate_launch_envelope(&plan.launch_spec).map_err(|_| invalid())?;
        let actual_spec_digest = content_digest(&plan.launch_spec).map_err(|_| invalid())?;
        let kernel_digest =
            native_resource_claims_digest(&plan.permit_claims).map_err(|_| invalid())?;
        let routing_digest = model_config_digest(model).map_err(|_| invalid())?;
        let contract_digest = content_digest(contract).map_err(|_| invalid())?;
        if model.provider != Provider::CodexCli
            || !model.enabled
            || request.provider != "codex_cli"
            || request.model != model.id
            || tuple.registry_model_id != model.id
            || tuple.requested_model != model.provider_model
            // The tuple retains FULL ModelSpec identity; inference uses the
            // separate model-selection routing digest. Neither is observed identity.
            || tuple.selected_model_digest != content_digest(model).map_err(|_| invalid())?
            || self.provider.tuple_digest() != content_digest(tuple).map_err(|_| invalid())?
            || actual_spec_digest
                != content_digest(self.provider.launch_spec()).map_err(|_| invalid())?
            || plan.permit_claims.launch_spec_digest != actual_spec_digest
            || tuple.argv_digest != content_digest(&plan.launch_spec.args).map_err(|_| invalid())?
            || tuple.run_id != plan.launch_spec.run_id
            || tuple.run_id != request.run_id
            || tuple.working_dir != plan.launch_spec.cwd
            || tuple.spool_dir != plan.launch_spec.spool_dir
            || tuple.node_id != plan.permit_claims.identity.node_id.as_str()
            || model.node_id.as_ref().is_some_and(|node| node != &tuple.node_id)
            || !canonical_revision(&tuple.registry_revision)
            || inference.project_id != contract.project_id
            || inference.root_id != contract.root_id
            || inference.task_id != contract.task_id
            || inference.attempt_id != request.attempt_id
            || inference.contract_revision != contract.revision
            || inference.contract_digest != contract_digest
            || inference.model_id != model.id
            || inference.provider != model.provider
            || inference.provider_model != model.provider_model
            || inference.tier != model.tier
            // A global model stays None; the actual prepared tuple still binds
            // its configured node. Do not rewrite one namespace into the other.
            || inference.node_id != model.node_id
            || inference.registry_revision != tuple.registry_revision
            || !bounded(&inference.snapshot_revision, 128)
            || inference.snapshot_revision.trim().is_empty()
            || inference.model_config_digest != routing_digest
            || inference.slots_reserved == 0
            || inference.slots_reserved > model.max_concurrency
            || tuple.scope.project_id != contract.project_id
            || tuple.scope.root_id != contract.root_id
            || tuple.scope.task_id != request.task_id
            || tuple.scope.attempt_id != request.attempt_id
            || tuple.scope.owner != request.owner
            || tuple.scope.revision != contract.revision
            || tuple.scope.contract_digest != contract_digest
            || tuple.scope.authority_ref != contract.authority_ref
            || plan.task_id != request.task_id
            || plan.attempt_id != request.attempt_id
            || plan.contract_digest != request.expected_contract_digest
            || plan.contract_digest != contract_digest
            || plan.context_digest != request.context_digest
            || plan.policy_digest != request.policy_digest
            || plan.provider_declared != request.provider
            || plan.model_declared != request.model
            || plan.run_id != request.run_id
            || plan.reservation.scope.resource_claims_digest != kernel_digest
            || plan.reservation.settled.get() != 0
            || plan.reservation.stop_requested
            || !plan.reservation.outstanding
        {
            return Err(invalid());
        }
        let budget_digest = admission_reservations::validate_command(&inputs.budget_command)
            .map_err(|_| invalid())?;
        inference_slots::validate_command(&inputs.slot_command).map_err(|_| invalid())?;
        let ReservationEvent::Reserve {
            reservation_id,
            scope,
            correlation_id,
            amount,
            ..
        } = &inputs.budget_command.event
        else {
            return Err(invalid());
        };
        let SlotEvent::Hold { claim, .. } = &inputs.slot_command.event else {
            return Err(invalid());
        };
        let planned = planned_launch_intent(&self.task, request).map_err(|_| invalid())?;
        let launch_digest = intent_digest(&planned).map_err(|_| invalid())?;
        if inputs.budget_command.source != request.source
            || inputs.slot_command.source != request.source
            // This Store profile requires command source's incarnation to bind
            // the kernel scope. The Codex producer tuple's incarnation remains
            // distinct; no equality with the host-boot namespace is inferred.
            || request.source.incarnation != plan.reservation.scope.incarnation
            || reservation_id != &plan.reservation.id
            || scope != &plan.reservation.scope
            || correlation_id != &plan.reservation.correlation_id
            || amount != &plan.reservation.original
            || inputs.budget_command.expected_ledger_revision.checked_add(1).is_none()
            || inputs.slot_command.expected_ledger_revision.checked_add(1).is_none()
            || inputs.budget_command.launch_intent_digest.as_deref() != Some(&launch_digest)
            || claim.scope != plan.reservation.scope
            || claim.budget_reservation_id != plan.reservation.id
            || claim.budget_command_digest != budget_digest
            || claim.kernel_claims_digest != kernel_digest
            || claim.inference_claims_digest != content_digest(inference).map_err(|_| invalid())?
            || claim.model_id != model.id
            || claim.model_config_digest != routing_digest
            || claim.slots != inference.slots_reserved
            || claim.launch_intent_digest != launch_digest
        {
            return Err(invalid());
        }
        Ok(())
    }

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

/// Inert original preparation plus a private exact slot-input binding. This type
/// does not activate the NativeTaskLoop or replace current Store/resource policy.
pub struct SlotBoundNativeCodexPlan {
    bound: BoundNativeCodexPlan,
    admission: NativeCodexSlotAdmission,
}
impl SlotBoundNativeCodexPlan {
    pub fn into_parts(
        self,
    ) -> (
        NativeExecutionPlan,
        LaunchRequest,
        PreparedCodexLaunch,
        QualifiedEnvironment,
        SystemdBackendConfig,
        NativeCodexSlotAdmission,
    ) {
        let (plan, request, provider, environment, backend) = self.bound.into_parts();
        (
            plan,
            request,
            provider,
            environment,
            backend,
            self.admission,
        )
    }
}

/// Custody-only container for separately issued opaque ledger-read capabilities.
/// Constructing this value authenticates nothing and issues no authority. It
/// deliberately has no Clone, Deserialize, Default, or credential fallback.
pub struct NativeCodexAdmissionReadProofs {
    pub budget: admission_reservations::ReservationProof,
    pub slot: inference_slots::SlotProof,
}

#[must_use = "retain the original plan, admission inputs, inference claim, and read capabilities on rejection"]
pub struct CodexSlotBindingFailure {
    error: CodexPlanError,
    bound: Box<BoundNativeCodexPlan>,
    inputs: Box<BudgetAndSlotAdmission>,
    inference: Box<InferenceSlotClaim>,
    reads: Box<NativeCodexAdmissionReadProofs>,
}
impl CodexSlotBindingFailure {
    pub fn into_parts(
        self,
    ) -> (
        CodexPlanError,
        BoundNativeCodexPlan,
        BudgetAndSlotAdmission,
        InferenceSlotClaim,
        NativeCodexAdmissionReadProofs,
    ) {
        (
            self.error,
            *self.bound,
            *self.inputs,
            *self.inference,
            *self.reads,
        )
    }
}
impl fmt::Debug for CodexSlotBindingFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexSlotBindingFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A factory-retained data binding, never a physical-capacity or execution grant.
/// Fields are private, and the value cannot be cloned or deserialized. Independent
/// Store policy/proofs and current ResourceGate authority remain necessary.
pub struct NativeCodexSlotAdmission {
    inputs: BudgetAndSlotAdmission,
    reads: NativeCodexAdmissionReadProofs,
    expected: FrozenSlotPlan,
}
impl NativeCodexSlotAdmission {
    pub fn inputs(&self) -> &BudgetAndSlotAdmission {
        &self.inputs
    }

    pub(crate) fn budget_read_proof(&self) -> &admission_reservations::ReservationProof {
        &self.reads.budget
    }

    pub(crate) fn slot_read_proof(&self) -> &inference_slots::SlotProof {
        &self.reads.slot
    }

    /// Bounded pure comparison against the exact originally bound preparation.
    /// A global ledger revision may advance independently of this plan; its
    /// reservation's own revision and entire value remain part of this binding.
    /// Filesystem custody and current authority are checked by their existing
    /// execution gates, not attested by this data comparison.
    pub fn matches_plan(&self, request: &LaunchRequest, plan: &NativeExecutionPlan) -> bool {
        let expected = &self.expected;
        request == &expected.request
            && plan.task_id == request.task_id
            && plan.attempt_id == request.attempt_id
            && plan.contract_digest == request.expected_contract_digest
            && plan.context_digest == request.context_digest
            && plan.policy_digest == request.policy_digest
            && plan.provider_declared == request.provider
            && plan.model_declared == request.model
            && plan.run_id == request.run_id
            && plan.reservation == expected.reservation
            && plan.permit_claims == expected.resource_claims
            && Arc::ptr_eq(&plan.permit_authority, &expected.permit_authority)
            && plan.workspace == expected.workspace
            && plan.prepared_workspace.workspace() == &expected.prepared_workspace
            && plan.prepared_workspace.head_oid() == expected.head_oid
            && plan.prepared_workspace.tree_oid() == expected.tree_oid
            && plan.prepared_workspace.task_id() == &request.task_id
            && plan.prepared_workspace.attempt_id() == &request.attempt_id
            && content_digest(&expected.task.contract)
                .is_ok_and(|digest| digest == plan.contract_digest)
            && frozen_spec_digest(&plan.launch_spec)
                .is_ok_and(|digest| digest == expected.launch_spec_digest)
            // Use the production full resource-claim domain, after exact bounded
            // claim equality, so no individual kernel claim can be omitted.
            && native_resource_claims_digest(&plan.permit_claims)
                .is_ok_and(|digest| digest == expected.resource_claims_digest)
    }
}

struct FrozenSlotPlan {
    task: TaskView,
    request: LaunchRequest,
    launch_spec_digest: String,
    resource_claims_digest: String,
    resource_claims: ResourcePermitClaims,
    reservation: Reservation,
    permit_authority: Arc<dyn ResourcePermitAuthority>,
    workspace: WorkspaceSpec,
    prepared_workspace: ValidatedWorkspace,
    head_oid: String,
    tree_oid: String,
}
impl FrozenSlotPlan {
    fn capture(bound: &BoundNativeCodexPlan) -> Result<Self, CodexPlanError> {
        let plan = &bound.plan;
        Ok(Self {
            task: bound.task.clone(),
            request: bound.request.clone(),
            launch_spec_digest: frozen_spec_digest(&plan.launch_spec)?,
            resource_claims_digest: native_resource_claims_digest(&plan.permit_claims)
                .map_err(|_| CodexPlanError::ResourceBinding)?,
            resource_claims: plan.permit_claims.clone(),
            reservation: plan.reservation.clone(),
            permit_authority: Arc::clone(&plan.permit_authority),
            workspace: plan.workspace.clone(),
            prepared_workspace: plan.prepared_workspace.workspace().clone(),
            head_oid: plan.prepared_workspace.head_oid().into(),
            tree_oid: plan.prepared_workspace.tree_oid().into(),
        })
    }
}

fn frozen_spec_digest(
    spec: &admiralty_runtime::session::LaunchSpec,
) -> Result<String, CodexPlanError> {
    bounded_json_size(spec, admiralty_runtime::session::MAX_LAUNCH_SPEC_BYTES)
        .map_err(|_| CodexPlanError::ResourceBinding)?;
    validate_launch_envelope(spec).map_err(|_| CodexPlanError::ResourceBinding)?;
    content_digest(spec).map_err(|_| CodexPlanError::ResourceBinding)
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
        model: input.model.clone(),
        workspace,
        provider,
        request,
        prepared_tick,
    })
}
fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
fn canonical_revision(value: &str) -> bool {
    value.len() <= 20
        && value
            .parse::<u64>()
            .is_ok_and(|revision| revision.to_string() == value)
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
