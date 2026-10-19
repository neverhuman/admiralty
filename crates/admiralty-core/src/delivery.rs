//! WO5: pure, bounded, source-bound delivery and attention projection.
//!
//! Joins independent repository and deployment stage observations against a
//! frozen [`TaskContract`], [`DeliveryRequirement`], and caller-owned non-Deserialize
//! [`TrustedDeliveryContext`] without I/O, clocks, inference, authority issuance,
//! model wakes, dispatch, or template execution.
use crate::{
    AttemptId, PrincipalId, ProjectId, RootId, SessionId, TaskContract, TaskId, content_digest,
    intake::{
        DeliveryRequirement, DeliveryStage, DeliveryTarget, DeliveryTargetKind, TargetRequirement,
    },
    limits::bounded_json_size,
    publication::{ApprovalEvidence, IntentStage, ReconciliationReceipt},
    verification::{ReceiptProducer, ValidatedReceipt},
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

pub const DELIVERY_PROJECTION_VERSION: u32 = 1;
pub const MAX_DELIVERY_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_DELIVERY_JSON_DEPTH: usize = 32;
pub const MAX_DELIVERY_JSON_NODES: usize = 32_768;
pub const MAX_DELIVERY_TARGETS: usize = 32;
pub const MAX_STAGE_OBSERVATIONS: usize = 128;
pub const MAX_BUNDLE_SESSIONS: usize = 128;
pub const MAX_ATTENTION_ITEMS: usize = 128;
pub const MAX_ATTENTION_FACTS: usize = 64;
pub const MAX_HISTORICAL_RECEIPTS: usize = 64;
pub const MAX_OPERATIONAL_BLOCKERS: usize = 64;
pub const MAX_RESCUE_REFS: usize = 32;
pub const MAX_TRAILERS: usize = 32;
pub const MAX_FIELD_BYTES: usize = 4096;
const MAX_TYPED_PROJECTION_BYTES: usize = 4 * MAX_DELIVERY_JSON_BYTES;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeliveryError {
    #[error("unsupported delivery projection schema version")]
    Version,
    #[error("input exceeds {limit} byte limit")]
    InputTooLarge { limit: usize },
    #[error("invalid UTF-8 delivery input")]
    Utf8,
    #[error("invalid or non-closed JSON input")]
    Json,
    #[error("duplicate JSON object key")]
    DuplicateKey,
    #[error("input exceeds nesting, collection, or field bound")]
    Bound,
    #[error("invalid delivery field: {0}")]
    Invalid(&'static str),
    #[error("invalid canonical decimal u64")]
    InvalidDecimal,
    #[error("logical clock arithmetic overflow")]
    Overflow,
    #[error("task contract and delivery requirement binding mismatch")]
    ContractBinding,
    #[error("join identity mismatch: {0:?}")]
    JoinMismatch(JoinMismatchReason),
}

type Result<T> = std::result::Result<T, DeliveryError>;

/// Canonical decimal string for 64-bit logical ticks on the public wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(u64);

impl DecimalU64 {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for DecimalU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = String::deserialize(deserializer)?;
        let parsed = wire.parse::<u64>().map_err(de::Error::custom)?;
        if parsed.to_string() != wire {
            return Err(de::Error::custom("expected canonical decimal u64 string"));
        }
        Ok(Self(parsed))
    }
}

impl JsonSchema for DecimalU64 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DeliveryDecimalU64".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <crate::interfaces::DecimalU64 as JsonSchema>::json_schema(generator)
    }
}

/// Caller-owned non-Deserialize binding context.
///
/// JSON observations cannot self-authenticate their own `provenance_ref`,
/// `ReceiptProducer::IndependentVerifier`, or `ApprovalEvidence`. Only
/// provenance references and validated receipts supplied here by the trusted
/// caller can attest an observation's custody/source.
///
/// ```compile_fail
/// use admiralty_core::delivery::TrustedDeliveryContext;
/// let _: TrustedDeliveryContext = serde_json::from_str("{}")
///     .expect("TrustedDeliveryContext must not implement Deserialize");
/// ```
#[derive(Clone, Debug)]
pub struct TrustedDeliveryContext {
    pub contract: TaskContract,
    pub requirement: DeliveryRequirement,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub default_owner: ResponsibleActor,
    pub now_tick: DecimalU64,
    /// Provenance references independently verified by the caller/Store/gateway.
    pub attested_provenance_refs: BTreeSet<String>,
    /// Optional non-Deserialize `ValidatedReceipt` bindings keyed by `receipt_digest`.
    pub validated_receipts: BTreeMap<String, ValidatedReceipt>,
}

impl TrustedDeliveryContext {
    pub fn validate(&self) -> Result<()> {
        if self.requirement.targets.len() > MAX_DELIVERY_TARGETS
            || self.attested_provenance_refs.len() > MAX_STAGE_OBSERVATIONS * 4
            || self.validated_receipts.len() > MAX_STAGE_OBSERVATIONS
        {
            return Err(DeliveryError::Bound);
        }
        validate_task_contract(&self.contract)?;
        self.requirement
            .validate()
            .map_err(|_| DeliveryError::ContractBinding)?;
        let binds = self
            .requirement
            .binds(&self.contract)
            .map_err(|_| DeliveryError::ContractBinding)?;
        if !binds {
            return Err(DeliveryError::ContractBinding);
        }
        validate_reference(&self.candidate_ref)?;
        validate_actor(&self.default_owner)?;
        for prov in &self.attested_provenance_refs {
            validate_reference(prov)?;
        }
        for (digest_key, receipt) in &self.validated_receipts {
            if !valid_digest(digest_key) || digest_key != receipt.receipt_digest() {
                return Err(DeliveryError::Invalid("validated_receipts key mismatch"));
            }
        }
        Ok(())
    }
}

/// Exact join tuple required on every candidate, stage, bundle, and session observation.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct DeliveryJoinIdentity {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub target_id: String,
    pub requirement_generation: u32,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum JoinMismatchReason {
    ProjectId,
    RootId,
    TaskId,
    ContractRevision,
    ContractDigest,
    AttemptId,
    CandidateRef,
    UnknownTargetId,
    TargetKindMismatch,
    SupersededGeneration,
    FutureGeneration,
}

impl DeliveryJoinIdentity {
    pub fn validate(&self) -> Result<()> {
        if self.contract_revision == 0
            || self.requirement_generation == 0
            || !valid_digest(&self.contract_digest)
        {
            return Err(DeliveryError::Invalid("join revision/generation/digest"));
        }
        validate_reference(&self.candidate_ref)?;
        validate_identifier(&self.target_id)
    }

    /// Compare an observation's join identity against the active task/attempt/candidate
    /// binding and declared target map.
    #[allow(clippy::too_many_arguments)]
    pub fn verify_against(
        &self,
        expected_project: &ProjectId,
        expected_root: &RootId,
        expected_task: &TaskId,
        expected_contract_revision: u32,
        expected_contract_digest: &str,
        expected_attempt: &AttemptId,
        expected_candidate_ref: &str,
        expected_generation: u32,
        declared_targets: &BTreeMap<String, DeliveryTargetKind>,
        observed_target_kind: DeliveryTargetKind,
    ) -> std::result::Result<(), JoinMismatchReason> {
        if &self.project_id != expected_project {
            return Err(JoinMismatchReason::ProjectId);
        }
        if &self.root_id != expected_root {
            return Err(JoinMismatchReason::RootId);
        }
        if &self.task_id != expected_task {
            return Err(JoinMismatchReason::TaskId);
        }
        if self.contract_revision != expected_contract_revision {
            return Err(JoinMismatchReason::ContractRevision);
        }
        if self.contract_digest != expected_contract_digest {
            return Err(JoinMismatchReason::ContractDigest);
        }
        if &self.attempt_id != expected_attempt {
            return Err(JoinMismatchReason::AttemptId);
        }
        if self.candidate_ref != expected_candidate_ref {
            return Err(JoinMismatchReason::CandidateRef);
        }
        let Some(declared_kind) = declared_targets.get(&self.target_id) else {
            return Err(JoinMismatchReason::UnknownTargetId);
        };
        if *declared_kind != observed_target_kind {
            return Err(JoinMismatchReason::TargetKindMismatch);
        }
        if self.requirement_generation < expected_generation {
            return Err(JoinMismatchReason::SupersededGeneration);
        }
        if self.requirement_generation > expected_generation {
            return Err(JoinMismatchReason::FutureGeneration);
        }
        Ok(())
    }
}

/// Observation provenance claim. Even `ClaimedEvidence` remains unattested until
/// matched against `TrustedDeliveryContext.attested_provenance_refs`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservationTrust {
    ClaimedEvidence {
        producer_id: PrincipalId,
        provenance_ref: String,
        evidence_digest: String,
    },
    UntrustedClaim {
        claimant: PrincipalId,
        hint_ref: String,
    },
    TrackingTrailerHint {
        trailer: String,
    },
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessState {
    Fresh,
    Stale,
    ClockSkewBackward,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RescueCandidateBinding {
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
    pub rescue_artifact_digest: String,
    pub rescue_ref: String,
    /// Must remain false by default; a rescue ref is never force-pushed by default.
    pub force_push_permitted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BundleIdentity {
    pub bundle_id: String,
    pub bundle_digest: String,
    pub version: String,
    pub source_commit_sha: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrainDisposition {
    Idle {},
    Draining {
        active_sessions: u32,
        preserved_candidate_refs: Vec<String>,
    },
    PendingUpdate {
        reason: String,
    },
    RollbackRequired {
        reason: String,
        champion_bundle_digest: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionExecutionObservation {
    pub session_id: SessionId,
    pub attempt_id: AttemptId,
    pub requested_model: String,
    pub requested_bundle_digest: String,
    /// Actual observed running model; None when unknown. Never filled from `requested_model`.
    pub observed_running_model: Option<String>,
    /// Actual observed running bundle digest; None when unknown. Never filled from requested/desired.
    pub observed_running_bundle_digest: Option<String>,
    pub provenance_ref: Option<String>,
    pub active_child_work: bool,
    pub unlanded_candidate_ref: Option<String>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum StageObservationDetail {
    Implementation {
        source_commit_sha: String,
        source_tree_sha: String,
        source_artifact_digest: String,
        #[serde(default)]
        tracking_trailers: Vec<String>,
    },
    Check {
        source_commit_sha: String,
        source_tree_sha: String,
        producer: ReceiptProducer,
        all_checks_passed: bool,
        negative_checks_passed: bool,
        receipt_digest: String,
    },
    Review {
        head_commit_sha: String,
        required_reviewers: Vec<String>,
        approvals: Vec<ApprovalEvidence>,
    },
    Merge {
        target_ref: String,
        expected_head_sha: String,
        expected_base_sha: String,
        observed_head_sha: Option<String>,
        intent_key: String,
        intent_stage: IntentStage,
        effect_receipt: Option<ReconciliationReceipt>,
        rescue_binding: Option<RescueCandidateBinding>,
    },
    Package {
        source_commit_sha: String,
        package_manifest_digest: String,
        version: String,
    },
    Stage {
        staged_bundle: BundleIdentity,
    },
    Install {
        installed_bundle: BundleIdentity,
        target_install_ref: String,
        install_receipt_digest: String,
    },
    Running {
        desired_bundle: BundleIdentity,
        installed_bundle: Option<BundleIdentity>,
        sessions: Vec<SessionExecutionObservation>,
        drain_disposition: DrainDisposition,
    },
    Release {
        released_bundle: BundleIdentity,
        release_ref: String,
        release_receipt_digest: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryObservation {
    pub observation_id: String,
    pub join: DeliveryJoinIdentity,
    pub target_kind: DeliveryTargetKind,
    pub trust: ObservationTrust,
    pub observed_tick: DecimalU64,
    pub valid_through_tick: DecimalU64,
    pub detail: StageObservationDetail,
}

/// Immutable receipt from an earlier requirement generation; retained across reopen/supersede.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoricalDeliveryReceipt {
    pub receipt_id: String,
    pub join: DeliveryJoinIdentity,
    pub satisfied_stage: DeliveryStage,
    pub receipt_digest: String,
    pub recorded_tick: DecimalU64,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ResponsibleRole {
    Worker,
    Commodore,
    Admiral,
    Owner,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponsibleActor {
    Assigned {
        role: ResponsibleRole,
        principal_id: PrincipalId,
    },
    RoleQueue {
        role: ResponsibleRole,
    },
    Unassigned {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationalBlockerKind {
    MissingToolchainOrSetup {
        check_or_tool_id: String,
        environment_id: String,
        detail: String,
    },
    ResourcePressureWait {
        resource_dimension: String,
        detail: String,
    },
    DecisionBlocker {
        question_id: String,
        detail: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OperationalBlockerObservation {
    pub blocker_id: String,
    pub target_id: Option<String>,
    pub owner: ResponsibleActor,
    pub provenance_ref: String,
    pub observed_tick: DecimalU64,
    pub valid_through_tick: DecimalU64,
    pub blocker: OperationalBlockerKind,
}

/// Criticality-ordered reason codes. Earlier variants have strictly higher urgency.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    UncertainExternalOutcome,
    PartialMultiRepoLanding,
    UnpreservedRescueWork,
    DecisionBlocker,
    FailedVerificationCheck,
    JoinIdentityMismatch,
    MissingToolchainOrSetup,
    ActualRunningIdentityUnknown,
    ResourcePressureWait,
    StaleInstalledBundle,
    StaleOrBackwardObservation,
    RequirementOrStageUnknown,
    MissingReviewApproval,
}

impl AttentionReason {
    pub const fn severity(self) -> AttentionSeverity {
        match self {
            Self::UncertainExternalOutcome
            | Self::PartialMultiRepoLanding
            | Self::UnpreservedRescueWork => AttentionSeverity::Critical,
            Self::DecisionBlocker
            | Self::FailedVerificationCheck
            | Self::JoinIdentityMismatch
            | Self::MissingToolchainOrSetup => AttentionSeverity::High,
            Self::ActualRunningIdentityUnknown
            | Self::ResourcePressureWait
            | Self::StaleInstalledBundle
            | Self::StaleOrBackwardObservation
            | Self::RequirementOrStageUnknown => AttentionSeverity::Medium,
            Self::MissingReviewApproval => AttentionSeverity::Normal,
        }
    }

    pub const fn default_next_action(self) -> NextAuthorizedAction {
        match self {
            Self::UncertainExternalOutcome => NextAuthorizedAction::ReconcileExternalEffect,
            Self::PartialMultiRepoLanding => NextAuthorizedAction::CompletePartialLanding,
            Self::UnpreservedRescueWork => NextAuthorizedAction::PinRescueCandidate,
            Self::DecisionBlocker => NextAuthorizedAction::RouteDecision,
            Self::FailedVerificationCheck => NextAuthorizedAction::RepairCandidateCheck,
            Self::JoinIdentityMismatch => NextAuthorizedAction::DiscardMismatchedClaim,
            Self::MissingToolchainOrSetup => NextAuthorizedAction::ProvisionToolchainOrSetup,
            Self::ActualRunningIdentityUnknown => NextAuthorizedAction::ProbeRunningIdentity,
            Self::ResourcePressureWait => NextAuthorizedAction::WaitForResourceCapacity,
            Self::StaleInstalledBundle => NextAuthorizedAction::StageOrActivateBundle,
            Self::StaleOrBackwardObservation => NextAuthorizedAction::RefreshObservation,
            Self::RequirementOrStageUnknown => NextAuthorizedAction::ResolveUnknownRequirement,
            Self::MissingReviewApproval => NextAuthorizedAction::AwaitConfiguredReviewer,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSeverity {
    Critical,
    High,
    Medium,
    Normal,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum NextAuthorizedAction {
    ReconcileExternalEffect,
    CompletePartialLanding,
    PinRescueCandidate,
    RouteDecision,
    RepairCandidateCheck,
    DiscardMismatchedClaim,
    ProvisionToolchainOrSetup,
    ProbeRunningIdentity,
    WaitForResourceCapacity,
    StageOrActivateBundle,
    RefreshObservation,
    ResolveUnknownRequirement,
    AwaitConfiguredReviewer,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct AttentionSubject {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub target_id: Option<String>,
    pub requirement_generation: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionItem {
    pub item_id: String,
    pub reason: AttentionReason,
    pub supporting_reasons: Vec<AttentionReason>,
    pub severity: AttentionSeverity,
    pub owner: ResponsibleActor,
    pub subject: AttentionSubject,
    pub first_observed_tick: DecimalU64,
    pub last_observed_tick: DecimalU64,
    pub wait_age_ticks: DecimalU64,
    pub first_observed_ref: String,
    pub latest_evidence_refs: Vec<String>,
    pub freshness: FreshnessState,
    pub facts: Vec<String>,
    pub facts_digest: String,
    pub next_action: NextAuthorizedAction,
}

/// Serialize-only delta summary. Pure projection never issues a model wake.
#[derive(Clone, Debug, Serialize, JsonSchema, PartialEq, Eq)]
pub struct AttentionDelta {
    pub changed_items: Vec<AttentionItem>,
    pub resolved_item_ids: Vec<String>,
    pub unchanged_item_ids: Vec<String>,
    /// True only when at least one actionable fact changed or newly appeared.
    pub actionable_facts_changed: bool,
    manager_wake_issued: bool,
    model_escalation_permitted: bool,
}

impl AttentionDelta {
    pub const fn manager_wake_issued(&self) -> bool {
        self.manager_wake_issued
    }

    pub const fn model_escalation_permitted(&self) -> bool {
        self.model_escalation_permitted
    }
}

/// Deserializable observation batch. Provenance claims inside this batch are
/// never self-authenticating; they are evaluated against [`TrustedDeliveryContext`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryObservationBatch {
    pub schema_version: u32,
    #[serde(default)]
    pub observations: Vec<DeliveryObservation>,
    #[serde(default)]
    pub operational_blockers: Vec<OperationalBlockerObservation>,
    #[serde(default)]
    pub historical_receipts: Vec<HistoricalDeliveryReceipt>,
    #[serde(default)]
    pub prior_attention: Vec<AttentionItem>,
}

/// Variant ordered from most severe/uncertain to `Verified` and `Unknown`.
/// Once a non-Verified anomaly is observed for a stage, additional observations
/// in the same batch cannot mask or overwrite it to `Verified`.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum StageEvaluation {
    UncertainOutcome,
    ClockSkewBackward,
    Stale,
    Failed,
    UnattestedProvenance,
    UntrustedClaim,
    Verified,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetStageMatrix {
    pub implementation: StageEvaluation,
    pub check: StageEvaluation,
    pub review: StageEvaluation,
    pub merge: StageEvaluation,
    pub package: StageEvaluation,
    pub stage: StageEvaluation,
    pub install: StageEvaluation,
    pub running: StageEvaluation,
    pub release: StageEvaluation,
}

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum BundleAlignmentState {
    RollbackPending,
    InstallMissing,
    StaleInstalledBundle,
    ActualRunningUnknown,
    DrainingOldBundle,
    AlignedRunning,
    NotObserved,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionRuntimeView {
    pub session_id: SessionId,
    pub attempt_id: AttemptId,
    pub requested_model: String,
    pub requested_bundle_digest: String,
    pub observed_running_model: Option<String>,
    pub observed_running_bundle_digest: Option<String>,
    pub running_identity_known: bool,
    pub matches_desired_bundle: bool,
    pub active_child_work: bool,
    pub unlanded_candidate_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BundleRuntimeProjection {
    pub desired_bundle: Option<BundleIdentity>,
    pub staged_bundle: Option<BundleIdentity>,
    pub installed_bundle: Option<BundleIdentity>,
    pub released_bundle: Option<BundleIdentity>,
    pub alignment: BundleAlignmentState,
    pub drain_disposition: Option<DrainDisposition>,
    pub sessions: Vec<SessionRuntimeView>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetCompletionState {
    Satisfied {
        required_stage: DeliveryStage,
    },
    Unsatisfied {
        required_stage: DeliveryStage,
        blocking_reasons: Vec<AttentionReason>,
    },
    OptionalSatisfied {
        stage: DeliveryStage,
    },
    OptionalPending {
        stage: DeliveryStage,
    },
    NotApplicable {
        reason: String,
    },
    RequirementUnknown {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalOutcomeState {
    Clear {},
    Merged {
        target_ref: String,
        observed_head_sha: String,
        provenance_ref: String,
    },
    Unlanded {
        target_ref: String,
        rescue_preserved: bool,
        rescue_ref: Option<String>,
    },
    Uncertain {
        target_ref: String,
        intent_key: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetDeliveryProjection {
    pub target_id: String,
    pub target_kind: DeliveryTargetKind,
    pub requirement: TargetRequirement,
    pub stages: TargetStageMatrix,
    pub highest_verified_stage: Option<DeliveryStage>,
    pub completion: TargetCompletionState,
    pub external_outcome: ExternalOutcomeState,
    pub bundle_runtime: Option<BundleRuntimeProjection>,
    pub lookup_hints: Vec<String>,
    pub primary_attention_reason: Option<AttentionReason>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MultiRepoLandingState {
    NoRepositoryTargets {},
    AllLanded {
        landed_targets: Vec<String>,
    },
    Unlanded {
        unlanded_targets: Vec<String>,
    },
    PartialLanding {
        landed_targets: Vec<String>,
        unlanded_targets: Vec<String>,
    },
    UncertainOutcome {
        uncertain_targets: Vec<String>,
        landed_targets: Vec<String>,
        unlanded_targets: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RejectedObservation {
    pub observation_id: String,
    pub target_id: String,
    pub reason: JoinMismatchReason,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCompletionState {
    Satisfied,
    Incomplete,
    BlockedByUnknownRequirement,
    BlockedByUncertainOutcome,
    BlockedByPartialLanding,
}

/// Serialize-only pure delivery projection result.
///
/// Never implements `Deserialize` so untrusted JSON cannot forge completion
/// or authority flags.
///
/// ```compile_fail
/// use admiralty_core::delivery::DeliveryProjection;
/// let _: DeliveryProjection = serde_json::from_str("{}")
///     .expect("untrusted JSON must not reconstruct DeliveryProjection");
/// ```
#[derive(Clone, Debug, Serialize, JsonSchema, PartialEq, Eq)]
pub struct DeliveryProjection {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub attempt_id: AttemptId,
    pub candidate_ref: String,
    pub requirement_generation: u32,
    pub now_tick: DecimalU64,
    pub completion_state: DeliveryCompletionState,
    pub overall_satisfied: bool,
    pub multi_repo_landing: MultiRepoLandingState,
    pub targets: Vec<TargetDeliveryProjection>,
    pub rejected_observations: Vec<RejectedObservation>,
    pub retained_historical_receipts: Vec<HistoricalDeliveryReceipt>,
    pub headline_attention_reason: Option<AttentionReason>,
    pub attention_items: Vec<AttentionItem>,
    pub attention_delta: AttentionDelta,
    acceptance_granted: bool,
    release_authorized: bool,
    dispatch_authorized: bool,
    manager_wake_issued: bool,
    source_authenticated_by_projection: bool,
    pub projection_digest: String,
}

impl DeliveryProjection {
    pub const fn acceptance_granted(&self) -> bool {
        self.acceptance_granted
    }

    pub const fn release_authorized(&self) -> bool {
        self.release_authorized
    }

    pub const fn dispatch_authorized(&self) -> bool {
        self.dispatch_authorized
    }

    pub const fn manager_wake_issued(&self) -> bool {
        self.manager_wake_issued
    }

    pub const fn source_authenticated_by_projection(&self) -> bool {
        self.source_authenticated_by_projection
    }
}

/// Parse bounded UTF-8 JSON observation bytes and project against caller-supplied
/// non-Deserialize [`TrustedDeliveryContext`].
pub fn parse_and_project_delivery(
    context: &TrustedDeliveryContext,
    bytes: &[u8],
) -> Result<DeliveryProjection> {
    let batch = parse_observation_batch(bytes)?;
    project_delivery(context, &batch)
}

/// Bound raw bytes, UTF-8, nesting depth, node count, string sizes, and duplicate keys
/// before deserializing into a closed [`DeliveryObservationBatch`].
pub fn parse_observation_batch(bytes: &[u8]) -> Result<DeliveryObservationBatch> {
    scan_json_bounds(bytes)?;
    let batch: DeliveryObservationBatch =
        serde_json::from_slice(bytes).map_err(|_| DeliveryError::Json)?;
    batch.validate()?;
    Ok(batch)
}

impl DeliveryObservationBatch {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != DELIVERY_PROJECTION_VERSION {
            return Err(DeliveryError::Version);
        }
        if self.observations.len() > MAX_STAGE_OBSERVATIONS
            || self.operational_blockers.len() > MAX_OPERATIONAL_BLOCKERS
            || self.historical_receipts.len() > MAX_HISTORICAL_RECEIPTS
            || self.prior_attention.len() > MAX_ATTENTION_ITEMS
        {
            return Err(DeliveryError::Bound);
        }
        bounded_json_size(self, MAX_DELIVERY_JSON_BYTES).map_err(|_| DeliveryError::Bound)?;

        let mut obs_ids = BTreeSet::new();
        for obs in &self.observations {
            obs.validate()?;
            if !obs_ids.insert(&obs.observation_id) {
                return Err(DeliveryError::Invalid("duplicate observation_id"));
            }
        }
        let mut blocker_ids = BTreeSet::new();
        for blocker in &self.operational_blockers {
            blocker.validate()?;
            if !blocker_ids.insert(&blocker.blocker_id) {
                return Err(DeliveryError::Invalid("duplicate blocker_id"));
            }
        }
        let mut receipt_ids = BTreeSet::new();
        for receipt in &self.historical_receipts {
            receipt.validate()?;
            if !receipt_ids.insert(&receipt.receipt_id) {
                return Err(DeliveryError::Invalid("duplicate historical receipt_id"));
            }
        }
        let mut item_ids = BTreeSet::new();
        for prior in &self.prior_attention {
            prior.validate()?;
            if !item_ids.insert(&prior.item_id) {
                return Err(DeliveryError::Invalid("duplicate prior attention item_id"));
            }
        }
        Ok(())
    }
}

impl DeliveryObservation {
    fn validate(&self) -> Result<()> {
        validate_identifier(&self.observation_id)?;
        self.join.validate()?;
        if self.valid_through_tick < self.observed_tick {
            return Err(DeliveryError::Invalid(
                "valid_through_tick precedes observed_tick",
            ));
        }
        match &self.trust {
            ObservationTrust::ClaimedEvidence {
                provenance_ref,
                evidence_digest,
                ..
            } => {
                validate_reference(provenance_ref)?;
                if !valid_digest(evidence_digest) {
                    return Err(DeliveryError::Invalid("evidence_digest"));
                }
            }
            ObservationTrust::UntrustedClaim { hint_ref, .. } => {
                validate_reference(hint_ref)?;
            }
            ObservationTrust::TrackingTrailerHint { trailer } => {
                validate_text(trailer, 512)?;
            }
        }
        match &self.detail {
            StageObservationDetail::Implementation {
                source_commit_sha,
                source_tree_sha,
                source_artifact_digest,
                tracking_trailers,
            } => {
                if !valid_git_oid(source_commit_sha)
                    || !valid_git_oid(source_tree_sha)
                    || source_commit_sha == source_tree_sha
                    || !valid_digest(source_artifact_digest)
                {
                    return Err(DeliveryError::Invalid("implementation commit/tree/digest"));
                }
                if tracking_trailers.len() > MAX_TRAILERS {
                    return Err(DeliveryError::Bound);
                }
                for trailer in tracking_trailers {
                    validate_text(trailer, 512)?;
                }
            }
            StageObservationDetail::Check {
                source_commit_sha,
                source_tree_sha,
                receipt_digest,
                ..
            } => {
                if !valid_git_oid(source_commit_sha)
                    || !valid_git_oid(source_tree_sha)
                    || source_commit_sha == source_tree_sha
                    || !valid_digest(receipt_digest)
                {
                    return Err(DeliveryError::Invalid("check commit/tree/digest"));
                }
            }
            StageObservationDetail::Review {
                head_commit_sha,
                required_reviewers,
                approvals,
            } => {
                if !valid_git_oid(head_commit_sha) {
                    return Err(DeliveryError::Invalid("review head_commit_sha"));
                }
                if required_reviewers.len() > MAX_STAGE_OBSERVATIONS
                    || approvals.len() > MAX_STAGE_OBSERVATIONS
                {
                    return Err(DeliveryError::Bound);
                }
                for reviewer in required_reviewers {
                    validate_identifier(reviewer)?;
                }
                for approval in approvals {
                    validate_identifier(&approval.reviewer)?;
                    if !valid_git_oid(&approval.head) {
                        return Err(DeliveryError::Invalid("approval head oid"));
                    }
                    validate_reference(&approval.provenance_ref)?;
                }
            }
            StageObservationDetail::Merge {
                target_ref,
                expected_head_sha,
                expected_base_sha,
                observed_head_sha,
                intent_key,
                effect_receipt,
                rescue_binding,
                ..
            } => {
                if !valid_git_ref(target_ref)
                    || !valid_git_oid(expected_head_sha)
                    || !valid_git_oid(expected_base_sha)
                {
                    return Err(DeliveryError::Invalid("merge ref/head/base"));
                }
                if let Some(obs_head) = observed_head_sha
                    && !valid_git_oid(obs_head)
                {
                    return Err(DeliveryError::Invalid("merge observed_head_sha"));
                }
                validate_reference(intent_key)?;
                if let Some(receipt) = effect_receipt {
                    validate_reference(&receipt.intent_key)?;
                    if !valid_digest(&receipt.request_digest)
                        || !valid_git_oid(&receipt.expected_head)
                        || !valid_git_oid(&receipt.expected_base)
                        || !valid_git_oid(&receipt.observed_head)
                        || !valid_git_ref(&receipt.scope.target_ref)
                    {
                        return Err(DeliveryError::Invalid("merge reconciliation receipt"));
                    }
                    validate_reference(&receipt.provenance_ref)?;
                }
                if let Some(rescue) = rescue_binding {
                    validate_rescue_binding(rescue)?;
                }
            }
            StageObservationDetail::Package {
                source_commit_sha,
                package_manifest_digest,
                version,
            } => {
                if !valid_git_oid(source_commit_sha) || !valid_digest(package_manifest_digest) {
                    return Err(DeliveryError::Invalid("package commit/digest"));
                }
                validate_identifier(version)?;
            }
            StageObservationDetail::Stage { staged_bundle } => {
                validate_bundle_identity(staged_bundle)?;
            }
            StageObservationDetail::Install {
                installed_bundle,
                target_install_ref,
                install_receipt_digest,
            } => {
                validate_bundle_identity(installed_bundle)?;
                validate_reference(target_install_ref)?;
                if !valid_digest(install_receipt_digest) {
                    return Err(DeliveryError::Invalid("install_receipt_digest"));
                }
            }
            StageObservationDetail::Running {
                desired_bundle,
                installed_bundle,
                sessions,
                drain_disposition,
            } => {
                validate_bundle_identity(desired_bundle)?;
                if let Some(installed) = installed_bundle {
                    validate_bundle_identity(installed)?;
                }
                if sessions.len() > MAX_BUNDLE_SESSIONS {
                    return Err(DeliveryError::Bound);
                }
                let mut seen_sessions = BTreeSet::new();
                for session in sessions {
                    if !seen_sessions.insert(&session.session_id) {
                        return Err(DeliveryError::Invalid("duplicate session_id"));
                    }
                    validate_identifier(&session.requested_model)?;
                    if !valid_digest(&session.requested_bundle_digest) {
                        return Err(DeliveryError::Invalid("requested_bundle_digest"));
                    }
                    if let Some(obs_model) = &session.observed_running_model {
                        validate_identifier(obs_model)?;
                    }
                    if let Some(obs_bundle) = &session.observed_running_bundle_digest
                        && !valid_digest(obs_bundle)
                    {
                        return Err(DeliveryError::Invalid("observed_running_bundle_digest"));
                    }
                    if let Some(prov) = &session.provenance_ref {
                        validate_reference(prov)?;
                    }
                    if let Some(candidate) = &session.unlanded_candidate_ref {
                        validate_reference(candidate)?;
                    }
                }
                validate_drain_disposition(drain_disposition)?;
            }
            StageObservationDetail::Release {
                released_bundle,
                release_ref,
                release_receipt_digest,
            } => {
                validate_bundle_identity(released_bundle)?;
                validate_reference(release_ref)?;
                if !valid_digest(release_receipt_digest) {
                    return Err(DeliveryError::Invalid("release_receipt_digest"));
                }
            }
        }
        Ok(())
    }
}

impl HistoricalDeliveryReceipt {
    fn validate(&self) -> Result<()> {
        validate_identifier(&self.receipt_id)?;
        self.join.validate()?;
        if !valid_digest(&self.receipt_digest) {
            return Err(DeliveryError::Invalid("historical receipt_digest"));
        }
        Ok(())
    }
}

impl OperationalBlockerObservation {
    fn validate(&self) -> Result<()> {
        validate_identifier(&self.blocker_id)?;
        if let Some(target_id) = &self.target_id {
            validate_identifier(target_id)?;
        }
        validate_actor(&self.owner)?;
        validate_reference(&self.provenance_ref)?;
        if self.valid_through_tick < self.observed_tick {
            return Err(DeliveryError::Invalid(
                "blocker valid_through_tick precedes observed_tick",
            ));
        }
        match &self.blocker {
            OperationalBlockerKind::MissingToolchainOrSetup {
                check_or_tool_id,
                environment_id,
                detail,
            } => {
                validate_identifier(check_or_tool_id)?;
                validate_identifier(environment_id)?;
                validate_text(detail, MAX_FIELD_BYTES)?;
            }
            OperationalBlockerKind::ResourcePressureWait {
                resource_dimension,
                detail,
            } => {
                validate_identifier(resource_dimension)?;
                validate_text(detail, MAX_FIELD_BYTES)?;
            }
            OperationalBlockerKind::DecisionBlocker {
                question_id,
                detail,
            } => {
                validate_identifier(question_id)?;
                validate_text(detail, MAX_FIELD_BYTES)?;
            }
        }
        Ok(())
    }
}

impl AttentionItem {
    fn validate(&self) -> Result<()> {
        validate_reference(&self.item_id)?;
        if self.supporting_reasons.len() > 16
            || self.latest_evidence_refs.len() > MAX_STAGE_OBSERVATIONS
            || self.facts.is_empty()
            || self.facts.len() > MAX_ATTENTION_FACTS
            || !valid_digest(&self.facts_digest)
        {
            return Err(DeliveryError::Bound);
        }
        validate_actor(&self.owner)?;
        validate_reference(&self.first_observed_ref)?;
        for r in &self.latest_evidence_refs {
            validate_reference(r)?;
        }
        for fact in &self.facts {
            validate_text(fact, MAX_FIELD_BYTES)?;
        }
        if self.last_observed_tick < self.first_observed_tick {
            return Err(DeliveryError::Invalid(
                "attention last_observed_tick < first_observed_tick",
            ));
        }
        Ok(())
    }
}

struct PendingAttention {
    target_id: Option<String>,
    reason: AttentionReason,
    owner: ResponsibleActor,
    observed_tick: DecimalU64,
    evidence_ref: String,
    freshness: FreshnessState,
    facts: Vec<String>,
}

/// Compute the deterministic delivery and attention projection from explicit observations
/// and caller-supplied non-Deserialize [`TrustedDeliveryContext`].
pub fn project_delivery(
    context: &TrustedDeliveryContext,
    batch: &DeliveryObservationBatch,
) -> Result<DeliveryProjection> {
    context.validate()?;
    batch.validate()?;
    let contract = &context.contract;
    let requirement = &context.requirement;
    let now_tick = context.now_tick;

    let declared_targets: BTreeMap<String, DeliveryTargetKind> = requirement
        .targets
        .iter()
        .map(|t| (t.id.clone(), t.target_kind))
        .collect();

    let mut rejected_observations = Vec::new();
    let mut valid_by_target: BTreeMap<String, Vec<&DeliveryObservation>> = BTreeMap::new();
    let mut pending_attention: Vec<PendingAttention> = Vec::new();

    for obs in &batch.observations {
        match obs.join.verify_against(
            &contract.project_id,
            &contract.root_id,
            &contract.task_id,
            contract.revision,
            &requirement.contract_digest,
            &context.attempt_id,
            &context.candidate_ref,
            requirement.generation,
            &declared_targets,
            obs.target_kind,
        ) {
            Ok(()) => {
                valid_by_target
                    .entry(obs.join.target_id.clone())
                    .or_default()
                    .push(obs);
            }
            Err(reason) => {
                rejected_observations.push(RejectedObservation {
                    observation_id: obs.observation_id.clone(),
                    target_id: obs.join.target_id.clone(),
                    reason,
                });
                let target_scope = declared_targets
                    .contains_key(&obs.join.target_id)
                    .then(|| obs.join.target_id.clone());
                pending_attention.push(PendingAttention {
                    target_id: target_scope,
                    reason: AttentionReason::JoinIdentityMismatch,
                    owner: context.default_owner.clone(),
                    observed_tick: obs.observed_tick.min(now_tick),
                    evidence_ref: format!("obs:{}", obs.observation_id),
                    freshness: evaluate_freshness(
                        obs.observed_tick,
                        obs.valid_through_tick,
                        now_tick,
                    ),
                    facts: vec![
                        format!("observation_id={}", obs.observation_id),
                        format!("mismatch={reason:?}"),
                        format!("observed_target={}", obs.join.target_id),
                        format!("observed_generation={}", obs.join.requirement_generation),
                        format!("expected_generation={}", requirement.generation),
                    ],
                });
            }
        }
    }

    // Retain historical receipts from prior generations (< requirement.generation)
    // that match the immutable project/root/task identity.
    let mut retained_historical_receipts = Vec::new();
    for receipt in &batch.historical_receipts {
        if receipt.join.project_id == contract.project_id
            && receipt.join.root_id == contract.root_id
            && receipt.join.task_id == contract.task_id
            && receipt.join.requirement_generation < requirement.generation
            && receipt.recorded_tick <= now_tick
        {
            retained_historical_receipts.push(receipt.clone());
        }
    }

    // Operational blockers (missing toolchain/setup, resource pressure, decision blockers).
    for blocker in &batch.operational_blockers {
        if let Some(target_id) = &blocker.target_id
            && !declared_targets.contains_key(target_id)
        {
            return Err(DeliveryError::JoinMismatch(
                JoinMismatchReason::UnknownTargetId,
            ));
        }
        let freshness =
            evaluate_freshness(blocker.observed_tick, blocker.valid_through_tick, now_tick);
        let (reason, mut facts) = match &blocker.blocker {
            OperationalBlockerKind::MissingToolchainOrSetup {
                check_or_tool_id,
                environment_id,
                detail,
            } => (
                AttentionReason::MissingToolchainOrSetup,
                vec![
                    format!("blocker_id={}", blocker.blocker_id),
                    format!("check_or_tool_id={check_or_tool_id}"),
                    format!("environment_id={environment_id}"),
                    format!("detail={detail}"),
                ],
            ),
            OperationalBlockerKind::ResourcePressureWait {
                resource_dimension,
                detail,
            } => (
                AttentionReason::ResourcePressureWait,
                vec![
                    format!("blocker_id={}", blocker.blocker_id),
                    format!("resource_dimension={resource_dimension}"),
                    format!("detail={detail}"),
                    "model_escalation=forbidden".into(),
                ],
            ),
            OperationalBlockerKind::DecisionBlocker {
                question_id,
                detail,
            } => (
                AttentionReason::DecisionBlocker,
                vec![
                    format!("blocker_id={}", blocker.blocker_id),
                    format!("question_id={question_id}"),
                    format!("detail={detail}"),
                ],
            ),
        };
        if freshness != FreshnessState::Fresh {
            facts.push(format!("freshness={freshness:?}"));
        }
        pending_attention.push(PendingAttention {
            target_id: blocker.target_id.clone(),
            reason,
            owner: blocker.owner.clone(),
            observed_tick: blocker.observed_tick.min(now_tick),
            evidence_ref: blocker.provenance_ref.clone(),
            freshness,
            facts,
        });
    }

    let mut target_projections = Vec::with_capacity(requirement.targets.len());
    let mut repo_landed = Vec::new();
    let mut repo_unlanded = Vec::new();
    let mut repo_uncertain = Vec::new();

    for target in &requirement.targets {
        let obs_list = valid_by_target
            .get(&target.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let projected = evaluate_target(context, target, obs_list, &mut pending_attention);
        if target.target_kind == DeliveryTargetKind::Repository
            && !matches!(target.requirement, TargetRequirement::NotApplicable { .. })
        {
            match &projected.external_outcome {
                ExternalOutcomeState::Merged { .. } => repo_landed.push(target.id.clone()),
                ExternalOutcomeState::Uncertain { .. } => repo_uncertain.push(target.id.clone()),
                ExternalOutcomeState::Unlanded { .. } | ExternalOutcomeState::Clear {} => {
                    repo_unlanded.push(target.id.clone());
                }
            }
        }
        target_projections.push(projected);
    }

    let multi_repo_landing =
        if repo_landed.is_empty() && repo_unlanded.is_empty() && repo_uncertain.is_empty() {
            MultiRepoLandingState::NoRepositoryTargets {}
        } else if !repo_uncertain.is_empty() {
            MultiRepoLandingState::UncertainOutcome {
                uncertain_targets: repo_uncertain.clone(),
                landed_targets: repo_landed.clone(),
                unlanded_targets: repo_unlanded.clone(),
            }
        } else if !repo_landed.is_empty() && !repo_unlanded.is_empty() {
            pending_attention.push(PendingAttention {
                target_id: None,
                reason: AttentionReason::PartialMultiRepoLanding,
                owner: context.default_owner.clone(),
                observed_tick: now_tick,
                evidence_ref: format!("task:{}:partial-landing", contract.task_id),
                freshness: FreshnessState::Fresh,
                facts: vec![
                    format!("landed_targets={}", repo_landed.join(",")),
                    format!("unlanded_targets={}", repo_unlanded.join(",")),
                ],
            });
            MultiRepoLandingState::PartialLanding {
                landed_targets: repo_landed,
                unlanded_targets: repo_unlanded,
            }
        } else if !repo_landed.is_empty() {
            MultiRepoLandingState::AllLanded {
                landed_targets: repo_landed,
            }
        } else {
            MultiRepoLandingState::Unlanded {
                unlanded_targets: repo_unlanded,
            }
        };

    let (attention_items, attention_delta) = compile_attention_items(
        contract,
        requirement,
        &context.attempt_id,
        &context.candidate_ref,
        now_tick,
        pending_attention,
        &batch.prior_attention,
    )?;

    for target_proj in &mut target_projections {
        target_proj.primary_attention_reason = attention_items
            .iter()
            .filter(|item| {
                item.subject.target_id.as_deref() == Some(target_proj.target_id.as_str())
            })
            .map(|item| item.reason)
            .min();
    }

    let headline_attention_reason = attention_items.first().map(|item| item.reason);

    let has_unknown_requirement = target_projections
        .iter()
        .any(|t| matches!(t.completion, TargetCompletionState::RequirementUnknown {}));
    let has_uncertain_outcome = target_projections
        .iter()
        .any(|t| matches!(t.external_outcome, ExternalOutcomeState::Uncertain { .. }));
    let has_partial_landing = matches!(
        multi_repo_landing,
        MultiRepoLandingState::PartialLanding { .. }
    );
    let has_required_target = requirement
        .targets
        .iter()
        .any(|t| matches!(t.requirement, TargetRequirement::Required { .. }));
    let all_required_satisfied = has_required_target
        && !has_unknown_requirement
        && !has_uncertain_outcome
        && !has_partial_landing
        && rejected_observations.is_empty()
        && batch.operational_blockers.is_empty()
        && target_projections.iter().all(|t| {
            matches!(
                t.completion,
                TargetCompletionState::Satisfied { .. }
                    | TargetCompletionState::OptionalSatisfied { .. }
                    | TargetCompletionState::OptionalPending { .. }
                    | TargetCompletionState::NotApplicable { .. }
            )
        });

    let completion_state = if has_uncertain_outcome {
        DeliveryCompletionState::BlockedByUncertainOutcome
    } else if has_partial_landing {
        DeliveryCompletionState::BlockedByPartialLanding
    } else if has_unknown_requirement {
        DeliveryCompletionState::BlockedByUnknownRequirement
    } else if all_required_satisfied {
        DeliveryCompletionState::Satisfied
    } else {
        DeliveryCompletionState::Incomplete
    };

    let mut projection = DeliveryProjection {
        schema_version: DELIVERY_PROJECTION_VERSION,
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        contract_revision: contract.revision,
        contract_digest: requirement.contract_digest.clone(),
        attempt_id: context.attempt_id.clone(),
        candidate_ref: context.candidate_ref.clone(),
        requirement_generation: requirement.generation,
        now_tick,
        completion_state,
        overall_satisfied: all_required_satisfied,
        multi_repo_landing,
        targets: target_projections,
        rejected_observations,
        retained_historical_receipts,
        headline_attention_reason,
        attention_items,
        attention_delta,
        acceptance_granted: false,
        release_authorized: false,
        dispatch_authorized: false,
        manager_wake_issued: false,
        source_authenticated_by_projection: false,
        projection_digest: String::new(),
    };
    bounded_json_size(&projection, MAX_TYPED_PROJECTION_BYTES).map_err(|_| DeliveryError::Bound)?;
    projection.projection_digest =
        canonical_domain_hash(b"admiralty.delivery.projection/v1\0", &projection)?;
    Ok(projection)
}

fn evaluate_target(
    context: &TrustedDeliveryContext,
    target: &DeliveryTarget,
    observations: &[&DeliveryObservation],
    pending_attention: &mut Vec<PendingAttention>,
) -> TargetDeliveryProjection {
    let contract = &context.contract;
    let requirement = &context.requirement;
    let attempt_id = &context.attempt_id;
    let candidate_ref = context.candidate_ref.as_str();
    let default_owner = &context.default_owner;
    let now_tick = context.now_tick;

    if let TargetRequirement::NotApplicable { reason } = &target.requirement {
        return TargetDeliveryProjection {
            target_id: target.id.clone(),
            target_kind: target.target_kind,
            requirement: target.requirement.clone(),
            stages: TargetStageMatrix {
                implementation: StageEvaluation::NotApplicable,
                check: StageEvaluation::NotApplicable,
                review: StageEvaluation::NotApplicable,
                merge: StageEvaluation::NotApplicable,
                package: StageEvaluation::NotApplicable,
                stage: StageEvaluation::NotApplicable,
                install: StageEvaluation::NotApplicable,
                running: StageEvaluation::NotApplicable,
                release: StageEvaluation::NotApplicable,
            },
            highest_verified_stage: None,
            completion: TargetCompletionState::NotApplicable {
                reason: reason.clone(),
            },
            external_outcome: ExternalOutcomeState::Clear {},
            bundle_runtime: None,
            lookup_hints: Vec::new(),
            primary_attention_reason: None,
        };
    }

    let mut stages = TargetStageMatrix {
        implementation: StageEvaluation::Unknown,
        check: StageEvaluation::Unknown,
        review: StageEvaluation::Unknown,
        merge: StageEvaluation::Unknown,
        package: StageEvaluation::Unknown,
        stage: StageEvaluation::Unknown,
        install: StageEvaluation::Unknown,
        running: StageEvaluation::Unknown,
        release: StageEvaluation::Unknown,
    };

    let mut lookup_hints = Vec::new();
    let mut candidate_commit: Option<String> = None;
    let mut candidate_tree: Option<String> = None;
    let mut external_outcome = ExternalOutcomeState::Clear {};
    let mut desired_bundle: Option<BundleIdentity> = None;
    let mut staged_bundle: Option<BundleIdentity> = None;
    let mut installed_bundle: Option<BundleIdentity> = None;
    let mut released_bundle: Option<BundleIdentity> = None;
    let mut runtime_projection: Option<BundleRuntimeProjection> = None;
    let mut packaged_commit: Option<String> = None;

    for obs in observations {
        let freshness = evaluate_freshness(obs.observed_tick, obs.valid_through_tick, now_tick);
        let evidence_ref = match &obs.trust {
            ObservationTrust::ClaimedEvidence { provenance_ref, .. } => provenance_ref.clone(),
            ObservationTrust::UntrustedClaim { hint_ref, .. } => hint_ref.clone(),
            ObservationTrust::TrackingTrailerHint { .. } => format!("obs:{}", obs.observation_id),
        };

        if let ObservationTrust::TrackingTrailerHint { trailer } = &obs.trust
            && !lookup_hints.contains(trailer)
        {
            lookup_hints.push(trailer.clone());
        }
        if let StageObservationDetail::Implementation {
            tracking_trailers, ..
        } = &obs.detail
        {
            for trailer in tracking_trailers {
                if !lookup_hints.contains(trailer) {
                    lookup_hints.push(trailer.clone());
                }
            }
        }

        if freshness == FreshnessState::ClockSkewBackward {
            record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::ClockSkewBackward);
            pending_attention.push(PendingAttention {
                target_id: Some(target.id.clone()),
                reason: AttentionReason::StaleOrBackwardObservation,
                owner: default_owner.clone(),
                observed_tick: now_tick,
                evidence_ref,
                freshness,
                facts: vec![
                    format!("observation_id={}", obs.observation_id),
                    format!("observed_tick={}", obs.observed_tick),
                    "anomaly=backward_logical_clock".into(),
                ],
            });
            continue;
        }
        if freshness == FreshnessState::Stale {
            record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Stale);
            pending_attention.push(PendingAttention {
                target_id: Some(target.id.clone()),
                reason: AttentionReason::StaleOrBackwardObservation,
                owner: default_owner.clone(),
                observed_tick: obs.observed_tick,
                evidence_ref,
                freshness,
                facts: vec![
                    format!("observation_id={}", obs.observation_id),
                    format!("observed_tick={}", obs.observed_tick),
                    format!("valid_through_tick={}", obs.valid_through_tick),
                    "anomaly=stale_observation".into(),
                ],
            });
            continue;
        }

        let claimed_prov = match &obs.trust {
            ObservationTrust::ClaimedEvidence { provenance_ref, .. } => Some(provenance_ref),
            ObservationTrust::UntrustedClaim { .. }
            | ObservationTrust::TrackingTrailerHint { .. } => None,
        };
        let Some(provenance_ref) = claimed_prov else {
            record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::UntrustedClaim);
            continue;
        };

        // Provenance fields in JSON never authenticate themselves: the caller's
        // non-Deserialize `TrustedDeliveryContext` must attest `provenance_ref`.
        if !context.attested_provenance_refs.contains(provenance_ref) {
            record_stage_evaluation(
                &mut stages,
                &obs.detail,
                StageEvaluation::UnattestedProvenance,
            );
            continue;
        }

        match &obs.detail {
            StageObservationDetail::Implementation {
                source_commit_sha,
                source_tree_sha,
                ..
            } => {
                let commit_conflict = candidate_commit
                    .as_ref()
                    .is_some_and(|c| c != source_commit_sha)
                    || candidate_tree
                        .as_ref()
                        .is_some_and(|t| t != source_tree_sha);
                if commit_conflict {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                } else {
                    candidate_commit = Some(source_commit_sha.clone());
                    candidate_tree = Some(source_tree_sha.clone());
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                }
            }
            StageObservationDetail::Check {
                source_commit_sha,
                source_tree_sha,
                producer,
                all_checks_passed,
                negative_checks_passed,
                receipt_digest,
            } => {
                let commit_matches = candidate_commit
                    .as_ref()
                    .is_none_or(|c| c == source_commit_sha)
                    && candidate_tree.as_ref().is_none_or(|t| t == source_tree_sha);
                let receipt_unattested = !context.validated_receipts.is_empty()
                    && !context.validated_receipts.contains_key(receipt_digest);
                let validated_ok = context
                    .validated_receipts
                    .get(receipt_digest)
                    .is_none_or(ValidatedReceipt::all_checks_passed);
                if *producer != ReceiptProducer::IndependentVerifier {
                    record_stage_evaluation(
                        &mut stages,
                        &obs.detail,
                        StageEvaluation::UntrustedClaim,
                    );
                    pending_attention.push(PendingAttention {
                        target_id: Some(target.id.clone()),
                        reason: AttentionReason::FailedVerificationCheck,
                        owner: default_owner.clone(),
                        observed_tick: obs.observed_tick,
                        evidence_ref,
                        freshness,
                        facts: vec![
                            format!("observation_id={}", obs.observation_id),
                            "check_error=provider_claim_not_independent".into(),
                        ],
                    });
                } else if receipt_unattested {
                    record_stage_evaluation(
                        &mut stages,
                        &obs.detail,
                        StageEvaluation::UnattestedProvenance,
                    );
                    pending_attention.push(PendingAttention {
                        target_id: Some(target.id.clone()),
                        reason: AttentionReason::FailedVerificationCheck,
                        owner: default_owner.clone(),
                        observed_tick: obs.observed_tick,
                        evidence_ref,
                        freshness,
                        facts: vec![
                            format!("observation_id={}", obs.observation_id),
                            format!("receipt_digest={receipt_digest}"),
                            "check_error=missing_validated_receipt_binding".into(),
                        ],
                    });
                } else if !commit_matches
                    || !all_checks_passed
                    || !negative_checks_passed
                    || !validated_ok
                {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                    pending_attention.push(PendingAttention {
                        target_id: Some(target.id.clone()),
                        reason: AttentionReason::FailedVerificationCheck,
                        owner: default_owner.clone(),
                        observed_tick: obs.observed_tick,
                        evidence_ref,
                        freshness,
                        facts: vec![
                            format!("observation_id={}", obs.observation_id),
                            format!("receipt_digest={receipt_digest}"),
                            format!("all_checks_passed={all_checks_passed}"),
                            format!("negative_checks_passed={negative_checks_passed}"),
                            format!("commit_matches={commit_matches}"),
                        ],
                    });
                } else {
                    candidate_commit.get_or_insert_with(|| source_commit_sha.clone());
                    candidate_tree.get_or_insert_with(|| source_tree_sha.clone());
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                }
            }
            StageObservationDetail::Review {
                head_commit_sha,
                required_reviewers,
                approvals,
            } => {
                let head_matches = candidate_commit
                    .as_ref()
                    .is_none_or(|c| c == head_commit_sha);
                let configured: BTreeSet<&String> = required_reviewers.iter().collect();
                let approvals_attested = approvals
                    .iter()
                    .all(|a| context.attested_provenance_refs.contains(&a.provenance_ref));
                let valid_review = !configured.is_empty()
                    && configured.len() == required_reviewers.len()
                    && head_matches
                    && approvals_attested
                    && approvals.iter().all(|a| configured.contains(&a.reviewer))
                    && configured.iter().all(|req| {
                        let matching: Vec<_> =
                            approvals.iter().filter(|a| &a.reviewer == *req).collect();
                        !matching.is_empty()
                            && matching
                                .iter()
                                .all(|a| !a.dismissed && &a.head == head_commit_sha)
                    });
                if !approvals.is_empty() && !approvals_attested {
                    record_stage_evaluation(
                        &mut stages,
                        &obs.detail,
                        StageEvaluation::UnattestedProvenance,
                    );
                    pending_attention.push(PendingAttention {
                        target_id: Some(target.id.clone()),
                        reason: AttentionReason::MissingReviewApproval,
                        owner: default_owner.clone(),
                        observed_tick: obs.observed_tick,
                        evidence_ref,
                        freshness,
                        facts: vec![
                            format!("observation_id={}", obs.observation_id),
                            "review_error=unattested_approval_provenance".into(),
                        ],
                    });
                } else if valid_review {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                } else {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                    pending_attention.push(PendingAttention {
                        target_id: Some(target.id.clone()),
                        reason: AttentionReason::MissingReviewApproval,
                        owner: default_owner.clone(),
                        observed_tick: obs.observed_tick,
                        evidence_ref,
                        freshness,
                        facts: vec![
                            format!("observation_id={}", obs.observation_id),
                            format!("head_commit_sha={head_commit_sha}"),
                            format!("required_reviewers={}", required_reviewers.join(",")),
                            format!("approvals_count={}", approvals.len()),
                        ],
                    });
                }
            }
            StageObservationDetail::Merge {
                target_ref,
                expected_head_sha,
                expected_base_sha,
                observed_head_sha,
                intent_key,
                intent_stage,
                effect_receipt,
                rescue_binding,
            } => {
                let head_matches = candidate_commit
                    .as_ref()
                    .is_none_or(|c| c == expected_head_sha);
                match intent_stage {
                    IntentStage::Ambiguous => {
                        record_stage_evaluation(
                            &mut stages,
                            &obs.detail,
                            StageEvaluation::UncertainOutcome,
                        );
                        external_outcome = ExternalOutcomeState::Uncertain {
                            target_ref: target_ref.clone(),
                            intent_key: intent_key.clone(),
                        };
                        pending_attention.push(PendingAttention {
                            target_id: Some(target.id.clone()),
                            reason: AttentionReason::UncertainExternalOutcome,
                            owner: default_owner.clone(),
                            observed_tick: obs.observed_tick,
                            evidence_ref: evidence_ref.clone(),
                            freshness,
                            facts: vec![
                                format!("target_id={}", target.id),
                                format!("target_ref={target_ref}"),
                                format!("intent_key={intent_key}"),
                                "status=ambiguous_remote_outcome_holds_slot".into(),
                            ],
                        });
                        check_rescue_preservation(
                            contract,
                            attempt_id,
                            candidate_ref,
                            candidate_commit.as_deref().unwrap_or(expected_head_sha),
                            candidate_tree.as_deref(),
                            rescue_binding.as_ref(),
                            &target.id,
                            default_owner,
                            obs.observed_tick,
                            &evidence_ref,
                            freshness,
                            pending_attention,
                        );
                    }
                    IntentStage::ReconciledSuccess => {
                        let receipt_prov_unattested = effect_receipt.as_ref().is_some_and(|r| {
                            !context.attested_provenance_refs.contains(&r.provenance_ref)
                        });
                        let receipt_ok = effect_receipt.as_ref().is_some_and(|r| {
                            r.effect_occurred
                                && context.attested_provenance_refs.contains(&r.provenance_ref)
                                && &r.intent_key == intent_key
                                && &r.expected_head == expected_head_sha
                                && &r.expected_base == expected_base_sha
                                && r.observed_head != *expected_base_sha
                                && Some(&r.observed_head) == observed_head_sha.as_ref()
                                && r.scope.project_id == contract.project_id
                                && r.scope.root_id == contract.root_id
                                && r.scope.task_id == contract.task_id
                                && &r.scope.target_ref == target_ref
                        });
                        if receipt_prov_unattested {
                            record_stage_evaluation(
                                &mut stages,
                                &obs.detail,
                                StageEvaluation::UnattestedProvenance,
                            );
                            if !matches!(external_outcome, ExternalOutcomeState::Uncertain { .. }) {
                                external_outcome = ExternalOutcomeState::Unlanded {
                                    target_ref: target_ref.clone(),
                                    rescue_preserved: false,
                                    rescue_ref: None,
                                };
                            }
                        } else if head_matches && receipt_ok {
                            record_stage_evaluation(
                                &mut stages,
                                &obs.detail,
                                StageEvaluation::Verified,
                            );
                            if matches!(external_outcome, ExternalOutcomeState::Clear {}) {
                                let r = effect_receipt.as_ref().expect("checked Some");
                                external_outcome = ExternalOutcomeState::Merged {
                                    target_ref: target_ref.clone(),
                                    observed_head_sha: r.observed_head.clone(),
                                    provenance_ref: r.provenance_ref.clone(),
                                };
                            }
                        } else {
                            record_stage_evaluation(
                                &mut stages,
                                &obs.detail,
                                StageEvaluation::Failed,
                            );
                            if !matches!(external_outcome, ExternalOutcomeState::Uncertain { .. }) {
                                external_outcome = ExternalOutcomeState::Unlanded {
                                    target_ref: target_ref.clone(),
                                    rescue_preserved: false,
                                    rescue_ref: None,
                                };
                            }
                        }
                    }
                    IntentStage::Proposed
                    | IntentStage::Claimed
                    | IntentStage::ReconciledNoEffect => {
                        record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                        let rescue_ok = check_rescue_preservation(
                            contract,
                            attempt_id,
                            candidate_ref,
                            candidate_commit.as_deref().unwrap_or(expected_head_sha),
                            candidate_tree.as_deref(),
                            rescue_binding.as_ref(),
                            &target.id,
                            default_owner,
                            obs.observed_tick,
                            &evidence_ref,
                            freshness,
                            pending_attention,
                        );
                        if !matches!(external_outcome, ExternalOutcomeState::Uncertain { .. }) {
                            external_outcome = ExternalOutcomeState::Unlanded {
                                target_ref: target_ref.clone(),
                                rescue_preserved: rescue_ok,
                                rescue_ref: rescue_binding.as_ref().map(|r| r.rescue_ref.clone()),
                            };
                        }
                    }
                }
            }
            StageObservationDetail::Package {
                source_commit_sha, ..
            } => {
                if packaged_commit
                    .as_ref()
                    .is_some_and(|c| c != source_commit_sha)
                {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                } else {
                    packaged_commit = Some(source_commit_sha.clone());
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                }
            }
            StageObservationDetail::Stage {
                staged_bundle: bundle,
            } => {
                let commit_ok = packaged_commit
                    .as_ref()
                    .is_none_or(|c| c == &bundle.source_commit_sha);
                let bundle_ok = staged_bundle.as_ref().is_none_or(|b| b == bundle);
                if commit_ok && bundle_ok {
                    staged_bundle = Some(bundle.clone());
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                } else {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                }
            }
            StageObservationDetail::Install {
                installed_bundle: bundle,
                ..
            } => {
                let bundle_conflict = installed_bundle.as_ref().is_some_and(|b| b != bundle);
                if !bundle_conflict {
                    installed_bundle = Some(bundle.clone());
                }
                let matches_staged = staged_bundle
                    .as_ref()
                    .is_none_or(|s| s.bundle_digest == bundle.bundle_digest);
                let matches_desired = desired_bundle
                    .as_ref()
                    .is_none_or(|d| d.bundle_digest == bundle.bundle_digest);
                if !bundle_conflict && matches_staged && matches_desired {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                } else {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                }
            }
            StageObservationDetail::Running {
                desired_bundle: desired,
                installed_bundle: obs_installed,
                sessions,
                drain_disposition,
            } => {
                desired_bundle = Some(desired.clone());
                if let Some(inst) = obs_installed {
                    installed_bundle = Some(inst.clone());
                }
                let effective_installed = installed_bundle.as_ref();
                if let Some(inst) = effective_installed {
                    if inst.bundle_digest != desired.bundle_digest {
                        stages.install =
                            merge_stage_evaluation(stages.install, StageEvaluation::Failed);
                    }
                } else {
                    stages.install =
                        merge_stage_evaluation(stages.install, StageEvaluation::Failed);
                }

                let mut session_views = Vec::with_capacity(sessions.len());
                let mut any_unknown_running = sessions.is_empty();
                let mut all_match_desired = !sessions.is_empty();

                for s in sessions {
                    // Never substitute requested_model or requested_bundle_digest for observed running!
                    let prov_attested = s
                        .provenance_ref
                        .as_ref()
                        .is_some_and(|p| context.attested_provenance_refs.contains(p));
                    let running_known = s.observed_running_model.is_some()
                        && s.observed_running_bundle_digest.is_some()
                        && prov_attested;
                    let matches_desired = running_known
                        && s.observed_running_bundle_digest.as_deref()
                            == Some(desired.bundle_digest.as_str());
                    if !running_known {
                        any_unknown_running = true;
                    }
                    if !matches_desired {
                        all_match_desired = false;
                    }
                    session_views.push(SessionRuntimeView {
                        session_id: s.session_id.clone(),
                        attempt_id: s.attempt_id.clone(),
                        requested_model: s.requested_model.clone(),
                        requested_bundle_digest: s.requested_bundle_digest.clone(),
                        observed_running_model: s.observed_running_model.clone(),
                        observed_running_bundle_digest: s.observed_running_bundle_digest.clone(),
                        running_identity_known: running_known,
                        matches_desired_bundle: matches_desired,
                        active_child_work: s.active_child_work,
                        unlanded_candidate_ref: s.unlanded_candidate_ref.clone(),
                    });
                }

                let incoming_alignment =
                    if matches!(drain_disposition, DrainDisposition::RollbackRequired { .. }) {
                        BundleAlignmentState::RollbackPending
                    } else if effective_installed.is_none() {
                        BundleAlignmentState::InstallMissing
                    } else if effective_installed
                        .is_some_and(|inst| inst.bundle_digest != desired.bundle_digest)
                    {
                        BundleAlignmentState::StaleInstalledBundle
                    } else if any_unknown_running {
                        BundleAlignmentState::ActualRunningUnknown
                    } else if !all_match_desired
                        || matches!(
                            drain_disposition,
                            DrainDisposition::Draining { .. }
                                | DrainDisposition::PendingUpdate { .. }
                        )
                    {
                        BundleAlignmentState::DrainingOldBundle
                    } else {
                        BundleAlignmentState::AlignedRunning
                    };

                let alignment = runtime_projection
                    .as_ref()
                    .map_or(incoming_alignment, |prev| {
                        prev.alignment.min(incoming_alignment)
                    });

                match incoming_alignment {
                    BundleAlignmentState::AlignedRunning => {
                        record_stage_evaluation(
                            &mut stages,
                            &obs.detail,
                            StageEvaluation::Verified,
                        );
                    }
                    BundleAlignmentState::ActualRunningUnknown => {
                        if stages.running == StageEvaluation::Verified {
                            stages.running = StageEvaluation::Failed;
                        }
                        pending_attention.push(PendingAttention {
                            target_id: Some(target.id.clone()),
                            reason: AttentionReason::ActualRunningIdentityUnknown,
                            owner: default_owner.clone(),
                            observed_tick: obs.observed_tick,
                            evidence_ref: evidence_ref.clone(),
                            freshness,
                            facts: vec![
                                format!("target_id={}", target.id),
                                format!("desired_bundle={}", desired.bundle_digest),
                                "observed_running_identity=unknown".into(),
                                "requested_substitute_accepted=false".into(),
                            ],
                        });
                    }
                    BundleAlignmentState::StaleInstalledBundle
                    | BundleAlignmentState::DrainingOldBundle
                    | BundleAlignmentState::RollbackPending
                    | BundleAlignmentState::InstallMissing => {
                        record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                        let installed_str = effective_installed
                            .map(|b| b.bundle_digest.as_str())
                            .unwrap_or("none");
                        pending_attention.push(PendingAttention {
                            target_id: Some(target.id.clone()),
                            reason: AttentionReason::StaleInstalledBundle,
                            owner: default_owner.clone(),
                            observed_tick: obs.observed_tick,
                            evidence_ref: evidence_ref.clone(),
                            freshness,
                            facts: vec![
                                format!("target_id={}", target.id),
                                format!("alignment={incoming_alignment:?}"),
                                format!("desired_bundle={}", desired.bundle_digest),
                                format!("installed_bundle={installed_str}"),
                            ],
                        });
                    }
                    BundleAlignmentState::NotObserved => {}
                }

                runtime_projection = Some(BundleRuntimeProjection {
                    desired_bundle: desired_bundle.clone(),
                    staged_bundle: staged_bundle.clone(),
                    installed_bundle: installed_bundle.clone(),
                    released_bundle: released_bundle.clone(),
                    alignment,
                    drain_disposition: Some(drain_disposition.clone()),
                    sessions: session_views,
                });
            }
            StageObservationDetail::Release {
                released_bundle: bundle,
                ..
            } => {
                released_bundle = Some(bundle.clone());
                let matches_desired = desired_bundle
                    .as_ref()
                    .is_none_or(|d| d.bundle_digest == bundle.bundle_digest);
                let matches_installed = installed_bundle
                    .as_ref()
                    .is_some_and(|i| i.bundle_digest == bundle.bundle_digest);
                if matches_desired
                    && matches_installed
                    && stages.package == StageEvaluation::Verified
                    && stages.stage == StageEvaluation::Verified
                    && stages.install == StageEvaluation::Verified
                    && stages.running == StageEvaluation::Verified
                {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Verified);
                } else {
                    record_stage_evaluation(&mut stages, &obs.detail, StageEvaluation::Failed);
                }
                if let Some(proj) = &mut runtime_projection {
                    proj.released_bundle = released_bundle.clone();
                }
            }
        }
    }

    if runtime_projection.is_none()
        && (desired_bundle.is_some()
            || staged_bundle.is_some()
            || installed_bundle.is_some()
            || released_bundle.is_some())
    {
        runtime_projection = Some(BundleRuntimeProjection {
            desired_bundle,
            staged_bundle,
            installed_bundle,
            released_bundle,
            alignment: BundleAlignmentState::NotObserved,
            drain_disposition: None,
            sessions: Vec::new(),
        });
    }

    let highest_verified_stage = compute_highest_verified_stage(target.target_kind, &stages);

    let completion = match &target.requirement {
        TargetRequirement::Unknown {} => {
            pending_attention.push(PendingAttention {
                target_id: Some(target.id.clone()),
                reason: AttentionReason::RequirementOrStageUnknown,
                owner: default_owner.clone(),
                observed_tick: now_tick,
                evidence_ref: format!(
                    "req:{}:gen{}:{}",
                    requirement.task_id, requirement.generation, target.id
                ),
                freshness: FreshnessState::Fresh,
                facts: vec![
                    format!("target_id={}", target.id),
                    "target_requirement=unknown".into(),
                ],
            });
            TargetCompletionState::RequirementUnknown {}
        }
        TargetRequirement::Required { stage } => {
            let blocking = missing_stage_reasons(target.target_kind, *stage, &stages);
            if blocking.is_empty() {
                TargetCompletionState::Satisfied {
                    required_stage: *stage,
                }
            } else {
                for &reason in &blocking {
                    let already_queued = pending_attention.iter().any(|item| {
                        item.target_id.as_deref() == Some(target.id.as_str())
                            && item.reason == reason
                    });
                    if !already_queued {
                        pending_attention.push(PendingAttention {
                            target_id: Some(target.id.clone()),
                            reason,
                            owner: default_owner.clone(),
                            observed_tick: now_tick,
                            evidence_ref: format!(
                                "req:{}:gen{}:{}:{}",
                                requirement.task_id,
                                requirement.generation,
                                target.id,
                                reason_slug(reason)
                            ),
                            freshness: FreshnessState::Fresh,
                            facts: vec![
                                format!("target_id={}", target.id),
                                format!("required_stage={stage:?}"),
                                format!("highest_verified_stage={highest_verified_stage:?}"),
                                format!("blocking_reason={}", reason_slug(reason)),
                            ],
                        });
                    }
                }
                TargetCompletionState::Unsatisfied {
                    required_stage: *stage,
                    blocking_reasons: blocking,
                }
            }
        }
        TargetRequirement::Optional { stage } => {
            if missing_stage_reasons(target.target_kind, *stage, &stages).is_empty() {
                TargetCompletionState::OptionalSatisfied { stage: *stage }
            } else {
                TargetCompletionState::OptionalPending { stage: *stage }
            }
        }
        TargetRequirement::NotApplicable { reason } => TargetCompletionState::NotApplicable {
            reason: reason.clone(),
        },
    };

    TargetDeliveryProjection {
        target_id: target.id.clone(),
        target_kind: target.target_kind,
        requirement: target.requirement.clone(),
        stages,
        highest_verified_stage,
        completion,
        external_outcome,
        bundle_runtime: runtime_projection,
        lookup_hints,
        primary_attention_reason: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn check_rescue_preservation(
    contract: &TaskContract,
    attempt_id: &AttemptId,
    candidate_ref: &str,
    expected_commit_sha: &str,
    expected_tree_sha: Option<&str>,
    rescue_binding: Option<&RescueCandidateBinding>,
    target_id: &str,
    default_owner: &ResponsibleActor,
    observed_tick: DecimalU64,
    evidence_ref: &str,
    freshness: FreshnessState,
    pending_attention: &mut Vec<PendingAttention>,
) -> bool {
    let valid = rescue_binding.is_some_and(|r| {
        r.task_id == contract.task_id
            && &r.attempt_id == attempt_id
            && r.candidate_ref == candidate_ref
            && r.source_commit_sha == expected_commit_sha
            && expected_tree_sha.is_none_or(|t| r.source_tree_sha == t)
            && !r.force_push_permitted
            && valid_rescue_ref(&r.rescue_ref)
    });
    if !valid {
        pending_attention.push(PendingAttention {
            target_id: Some(target_id.to_owned()),
            reason: AttentionReason::UnpreservedRescueWork,
            owner: default_owner.clone(),
            observed_tick,
            evidence_ref: evidence_ref.to_owned(),
            freshness,
            facts: vec![
                format!("target_id={target_id}"),
                format!("candidate_ref={candidate_ref}"),
                "rescue_binding_valid=false".into(),
                "force_push_default=forbidden".into(),
            ],
        });
    }
    valid
}

/// Combine an existing stage evaluation with a newly observed evaluation.
/// Additional observations on the same stage cannot hide an earlier failure,
/// stale clock, unattested provenance, or uncertain external outcome.
fn merge_stage_evaluation(current: StageEvaluation, incoming: StageEvaluation) -> StageEvaluation {
    if current == StageEvaluation::Unknown {
        incoming
    } else if incoming == StageEvaluation::Unknown {
        current
    } else {
        current.min(incoming)
    }
}

fn record_stage_evaluation(
    matrix: &mut TargetStageMatrix,
    detail: &StageObservationDetail,
    incoming: StageEvaluation,
) {
    let slot = match detail {
        StageObservationDetail::Implementation { .. } => &mut matrix.implementation,
        StageObservationDetail::Check { .. } => &mut matrix.check,
        StageObservationDetail::Review { .. } => &mut matrix.review,
        StageObservationDetail::Merge { .. } => &mut matrix.merge,
        StageObservationDetail::Package { .. } => &mut matrix.package,
        StageObservationDetail::Stage { .. } => &mut matrix.stage,
        StageObservationDetail::Install { .. } => &mut matrix.install,
        StageObservationDetail::Running { .. } => &mut matrix.running,
        StageObservationDetail::Release { .. } => &mut matrix.release,
    };
    *slot = merge_stage_evaluation(*slot, incoming);
}

fn compute_highest_verified_stage(
    kind: DeliveryTargetKind,
    s: &TargetStageMatrix,
) -> Option<DeliveryStage> {
    let v = StageEvaluation::Verified;
    match kind {
        DeliveryTargetKind::Repository => {
            if s.implementation == v && s.check == v && s.review == v && s.merge == v {
                Some(DeliveryStage::Merged)
            } else if s.implementation == v && s.check == v {
                Some(DeliveryStage::Verified)
            } else if s.implementation == v {
                Some(DeliveryStage::Candidate)
            } else {
                None
            }
        }
        DeliveryTargetKind::Deployment => {
            if s.package == v && s.stage == v && s.install == v && s.running == v && s.release == v
            {
                Some(DeliveryStage::Released)
            } else if s.package == v && s.stage == v && s.install == v && s.running == v {
                Some(DeliveryStage::Running)
            } else if s.package == v && s.stage == v && s.install == v {
                Some(DeliveryStage::Installed)
            } else if s.package == v && s.stage == v {
                Some(DeliveryStage::Staged)
            } else if s.package == v {
                Some(DeliveryStage::Packaged)
            } else {
                None
            }
        }
    }
}

fn missing_stage_reasons(
    kind: DeliveryTargetKind,
    required_stage: DeliveryStage,
    s: &TargetStageMatrix,
) -> Vec<AttentionReason> {
    let mut reasons = BTreeSet::new();
    let mut check_eval =
        |eval: StageEvaluation, missing_or_failed_reason: AttentionReason| match eval {
            StageEvaluation::Verified => {}
            StageEvaluation::UncertainOutcome => {
                reasons.insert(AttentionReason::UncertainExternalOutcome);
            }
            StageEvaluation::Stale | StageEvaluation::ClockSkewBackward => {
                reasons.insert(AttentionReason::StaleOrBackwardObservation);
            }
            StageEvaluation::Failed
            | StageEvaluation::UnattestedProvenance
            | StageEvaluation::UntrustedClaim
            | StageEvaluation::Unknown
            | StageEvaluation::NotApplicable => {
                reasons.insert(missing_or_failed_reason);
            }
        };

    match (kind, required_stage) {
        (DeliveryTargetKind::Repository, DeliveryStage::Candidate) => {
            check_eval(s.implementation, AttentionReason::RequirementOrStageUnknown);
        }
        (DeliveryTargetKind::Repository, DeliveryStage::Verified) => {
            check_eval(s.implementation, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.check, AttentionReason::FailedVerificationCheck);
        }
        (DeliveryTargetKind::Repository, DeliveryStage::Merged) => {
            check_eval(s.implementation, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.check, AttentionReason::FailedVerificationCheck);
            check_eval(s.review, AttentionReason::MissingReviewApproval);
            check_eval(s.merge, AttentionReason::RequirementOrStageUnknown);
        }
        (DeliveryTargetKind::Deployment, DeliveryStage::Packaged) => {
            check_eval(s.package, AttentionReason::RequirementOrStageUnknown);
        }
        (DeliveryTargetKind::Deployment, DeliveryStage::Staged) => {
            check_eval(s.package, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.stage, AttentionReason::StaleInstalledBundle);
        }
        (DeliveryTargetKind::Deployment, DeliveryStage::Installed) => {
            check_eval(s.package, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.stage, AttentionReason::StaleInstalledBundle);
            check_eval(s.install, AttentionReason::StaleInstalledBundle);
        }
        (DeliveryTargetKind::Deployment, DeliveryStage::Running) => {
            check_eval(s.package, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.stage, AttentionReason::StaleInstalledBundle);
            check_eval(s.install, AttentionReason::StaleInstalledBundle);
            check_eval(s.running, AttentionReason::ActualRunningIdentityUnknown);
        }
        (DeliveryTargetKind::Deployment, DeliveryStage::Released) => {
            check_eval(s.package, AttentionReason::RequirementOrStageUnknown);
            check_eval(s.stage, AttentionReason::StaleInstalledBundle);
            check_eval(s.install, AttentionReason::StaleInstalledBundle);
            check_eval(s.running, AttentionReason::ActualRunningIdentityUnknown);
            check_eval(s.release, AttentionReason::RequirementOrStageUnknown);
        }
        _ => {
            reasons.insert(AttentionReason::RequirementOrStageUnknown);
        }
    }

    reasons.into_iter().collect()
}

fn compile_attention_items(
    contract: &TaskContract,
    requirement: &DeliveryRequirement,
    attempt_id: &AttemptId,
    candidate_ref: &str,
    now_tick: DecimalU64,
    pending: Vec<PendingAttention>,
    prior_attention: &[AttentionItem],
) -> Result<(Vec<AttentionItem>, AttentionDelta)> {
    let prior_by_id: BTreeMap<&str, &AttentionItem> = prior_attention
        .iter()
        .map(|item| (item.item_id.as_str(), item))
        .collect();

    let mut target_reasons: BTreeMap<Option<String>, BTreeSet<AttentionReason>> = BTreeMap::new();
    for entry in &pending {
        target_reasons
            .entry(entry.target_id.clone())
            .or_default()
            .insert(entry.reason);
    }

    let mut grouped: BTreeMap<(Option<String>, AttentionReason), Vec<PendingAttention>> =
        BTreeMap::new();
    for entry in pending {
        grouped
            .entry((entry.target_id.clone(), entry.reason))
            .or_default()
            .push(entry);
    }

    let mut items = Vec::with_capacity(grouped.len());
    let mut changed_items = Vec::new();
    let mut unchanged_item_ids = Vec::new();
    let mut active_ids = BTreeSet::new();

    for ((target_id, reason), entries) in grouped {
        if items.len() >= MAX_ATTENTION_ITEMS {
            return Err(DeliveryError::Bound);
        }
        let target_label = target_id.as_deref().unwrap_or("task");
        let item_id = format!(
            "attn:{}:gen{}:{}:{}",
            contract.task_id,
            requirement.generation,
            target_label,
            reason_slug(reason)
        );
        active_ids.insert(item_id.clone());

        let first_entry = entries.first().expect("non-empty group");
        let owner = first_entry.owner.clone();
        let mut facts_set = BTreeSet::new();
        let mut evidence_refs = BTreeSet::new();
        let mut earliest_tick = first_entry.observed_tick;
        let mut latest_tick = first_entry.observed_tick;
        let mut worst_freshness = first_entry.freshness;

        for e in &entries {
            earliest_tick = earliest_tick.min(e.observed_tick);
            latest_tick = latest_tick.max(e.observed_tick);
            worst_freshness = worst_freshness.max(e.freshness);
            evidence_refs.insert(e.evidence_ref.clone());
            for fact in &e.facts {
                if facts_set.len() < MAX_ATTENTION_FACTS {
                    facts_set.insert(fact.clone());
                }
            }
        }

        let facts: Vec<String> = facts_set.into_iter().collect();
        let subject = AttentionSubject {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            contract_revision: contract.revision,
            attempt_id: attempt_id.clone(),
            candidate_ref: candidate_ref.to_owned(),
            target_id: target_id.clone(),
            requirement_generation: requirement.generation,
        };
        let supporting_reasons: Vec<AttentionReason> = target_reasons
            .get(&target_id)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_else(|| vec![reason]);

        let facts_digest = canonical_domain_hash(
            b"admiralty.delivery.attention-facts/v1\0",
            &(reason, &supporting_reasons, &owner, &subject, &facts),
        )?;

        let (first_observed_tick, last_observed_tick, first_observed_ref, is_unchanged) =
            if let Some(prior) = prior_by_id.get(item_id.as_str()) {
                if now_tick < prior.first_observed_tick || now_tick < prior.last_observed_tick {
                    return Err(DeliveryError::Invalid(
                        "now_tick regressed before prior attention observation",
                    ));
                }
                let unchanged = prior.facts_digest == facts_digest
                    && prior.reason == reason
                    && prior.owner == owner;
                (
                    prior.first_observed_tick,
                    now_tick.max(latest_tick),
                    prior.first_observed_ref.clone(),
                    unchanged,
                )
            } else {
                (
                    earliest_tick.min(now_tick),
                    now_tick.max(latest_tick),
                    first_entry.evidence_ref.clone(),
                    false,
                )
            };

        let wait_age = now_tick
            .get()
            .checked_sub(first_observed_tick.get())
            .ok_or(DeliveryError::Overflow)?;

        let item = AttentionItem {
            item_id: item_id.clone(),
            reason,
            supporting_reasons,
            severity: reason.severity(),
            owner,
            subject,
            first_observed_tick,
            last_observed_tick,
            wait_age_ticks: DecimalU64::new(wait_age),
            first_observed_ref,
            latest_evidence_refs: evidence_refs.into_iter().collect(),
            freshness: worst_freshness,
            facts,
            facts_digest,
            next_action: reason.default_next_action(),
        };

        if is_unchanged {
            unchanged_item_ids.push(item_id);
        } else {
            changed_items.push(item.clone());
        }
        items.push(item);
    }

    items.sort_by(|a, b| {
        a.reason
            .cmp(&b.reason)
            .then_with(|| a.subject.target_id.cmp(&b.subject.target_id))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    changed_items.sort_by(|a, b| {
        a.reason
            .cmp(&b.reason)
            .then_with(|| a.subject.target_id.cmp(&b.subject.target_id))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    let resolved_item_ids: Vec<String> = prior_attention
        .iter()
        .filter(|prior| !active_ids.contains(&prior.item_id))
        .map(|prior| prior.item_id.clone())
        .collect();

    let actionable_facts_changed = !changed_items.is_empty();
    let delta = AttentionDelta {
        changed_items,
        resolved_item_ids,
        unchanged_item_ids,
        actionable_facts_changed,
        manager_wake_issued: false,
        model_escalation_permitted: false,
    };

    Ok((items, delta))
}

fn evaluate_freshness(
    observed_tick: DecimalU64,
    valid_through_tick: DecimalU64,
    now_tick: DecimalU64,
) -> FreshnessState {
    if observed_tick > now_tick {
        FreshnessState::ClockSkewBackward
    } else if now_tick > valid_through_tick {
        FreshnessState::Stale
    } else {
        FreshnessState::Fresh
    }
}

const fn reason_slug(reason: AttentionReason) -> &'static str {
    match reason {
        AttentionReason::UncertainExternalOutcome => "uncertain_external_outcome",
        AttentionReason::PartialMultiRepoLanding => "partial_multi_repo_landing",
        AttentionReason::UnpreservedRescueWork => "unpreserved_rescue_work",
        AttentionReason::DecisionBlocker => "decision_blocker",
        AttentionReason::FailedVerificationCheck => "failed_verification_check",
        AttentionReason::JoinIdentityMismatch => "join_identity_mismatch",
        AttentionReason::MissingToolchainOrSetup => "missing_toolchain_or_setup",
        AttentionReason::ActualRunningIdentityUnknown => "actual_running_identity_unknown",
        AttentionReason::ResourcePressureWait => "resource_pressure_wait",
        AttentionReason::StaleInstalledBundle => "stale_installed_bundle",
        AttentionReason::StaleOrBackwardObservation => "stale_or_backward_observation",
        AttentionReason::RequirementOrStageUnknown => "requirement_or_stage_unknown",
        AttentionReason::MissingReviewApproval => "missing_review_approval",
    }
}

fn validate_task_contract(task: &TaskContract) -> Result<()> {
    if task.non_goals.len() > 64
        || task.owned_paths.len() > 128
        || task.forbidden_paths.len() > 128
        || task.read_first.len() > 128
        || task.dependencies.len() > 1000
        || task.criteria.len() > 128
        || task.checks.len() > 128
    {
        return Err(DeliveryError::Bound);
    }
    for check in &task.checks {
        if check.args.len() > 128 {
            return Err(DeliveryError::Bound);
        }
    }
    bounded_json_size(task, 64 * 1024).map_err(|_| DeliveryError::Bound)?;
    task.validate().map_err(|_| DeliveryError::ContractBinding)
}

fn validate_bundle_identity(bundle: &BundleIdentity) -> Result<()> {
    validate_identifier(&bundle.bundle_id)?;
    validate_identifier(&bundle.version)?;
    if !valid_digest(&bundle.bundle_digest) || !valid_git_oid(&bundle.source_commit_sha) {
        return Err(DeliveryError::Invalid("bundle digest or commit sha"));
    }
    Ok(())
}

fn validate_drain_disposition(drain: &DrainDisposition) -> Result<()> {
    match drain {
        DrainDisposition::Idle {} => Ok(()),
        DrainDisposition::Draining {
            active_sessions,
            preserved_candidate_refs,
        } => {
            if *active_sessions == 0 || preserved_candidate_refs.len() > MAX_RESCUE_REFS {
                return Err(DeliveryError::Bound);
            }
            for r in preserved_candidate_refs {
                validate_reference(r)?;
            }
            Ok(())
        }
        DrainDisposition::PendingUpdate { reason } => validate_text(reason, MAX_FIELD_BYTES),
        DrainDisposition::RollbackRequired {
            reason,
            champion_bundle_digest,
        } => {
            validate_text(reason, MAX_FIELD_BYTES)?;
            if !valid_digest(champion_bundle_digest) {
                return Err(DeliveryError::Invalid("champion_bundle_digest"));
            }
            Ok(())
        }
    }
}

fn validate_rescue_binding(rescue: &RescueCandidateBinding) -> Result<()> {
    validate_reference(&rescue.candidate_ref)?;
    if !valid_git_oid(&rescue.source_commit_sha)
        || !valid_git_oid(&rescue.source_tree_sha)
        || rescue.source_commit_sha == rescue.source_tree_sha
        || !valid_digest(&rescue.rescue_artifact_digest)
        || !valid_rescue_ref(&rescue.rescue_ref)
    {
        return Err(DeliveryError::Invalid("rescue candidate binding"));
    }
    Ok(())
}

fn validate_actor(actor: &ResponsibleActor) -> Result<()> {
    match actor {
        ResponsibleActor::Assigned { principal_id, .. } => {
            validate_identifier(principal_id.as_str())
        }
        ResponsibleActor::RoleQueue { .. } | ResponsibleActor::Unassigned {} => Ok(()),
    }
}

fn validate_identifier(value: &str) -> Result<()> {
    if value.len() > 128 || TaskId::new(value).is_err() {
        Err(DeliveryError::Invalid("identifier"))
    } else {
        Ok(())
    }
}

fn validate_text(value: &str, max: usize) -> Result<()> {
    if value.len() > max {
        return Err(DeliveryError::Bound);
    }
    if value.trim().is_empty()
        || value.chars().any(char::is_control)
        || value.contains("${")
        || value.contains("{{")
    {
        return Err(DeliveryError::Invalid("empty, control, or template text"));
    }
    Ok(())
}

fn validate_reference(value: &str) -> Result<()> {
    validate_text(value, 1024)?;
    if value
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || value.contains(['/', '\\'])
        || value.starts_with('.')
        || value.contains("..")
        || ["javascript:", "data:", "file:", "http:", "https:", "ssh:"]
            .iter()
            .any(|prefix| value.to_ascii_lowercase().starts_with(prefix))
    {
        return Err(DeliveryError::Invalid("opaque reference"));
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && !value.bytes().all(|b| b == b'0')
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_git_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.starts_with("refs/heads/")
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.contains("//")
        && !value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
        && value
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn valid_rescue_ref(value: &str) -> bool {
    valid_git_ref(value) && value.starts_with("refs/heads/rescue/")
}

fn canonical_domain_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<String> {
    bounded_json_size(value, MAX_TYPED_PROJECTION_BYTES).map_err(|_| DeliveryError::Bound)?;
    let canonical =
        content_digest(value).map_err(|_| DeliveryError::Invalid("canonical encoding"))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(canonical.as_bytes());
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

struct JsonScanner<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
}

impl JsonScanner<'_> {
    fn ws(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn string(&mut self, is_key: bool) -> Result<&[u8]> {
        let start = self.at;
        if self.bytes.get(self.at) != Some(&b'"') {
            return Err(DeliveryError::Json);
        }
        self.at += 1;
        let max_len = if is_key { 128 + 2 } else { MAX_FIELD_BYTES + 2 };
        loop {
            let b = *self.bytes.get(self.at).ok_or(DeliveryError::Json)?;
            self.at += 1;
            if self.at - start > max_len {
                return Err(DeliveryError::Bound);
            }
            match b {
                b'"' => return Ok(&self.bytes[start..self.at]),
                b'\\' => {
                    self.at += 1;
                    if self.at > self.bytes.len() {
                        return Err(DeliveryError::Json);
                    }
                }
                0..=31 => return Err(DeliveryError::Json),
                _ => {}
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<()> {
        self.ws();
        self.nodes += 1;
        if depth > MAX_DELIVERY_JSON_DEPTH || self.nodes > MAX_DELIVERY_JSON_NODES {
            return Err(DeliveryError::Bound);
        }
        match self.bytes.get(self.at).copied() {
            Some(b'{') => {
                self.at += 1;
                self.ws();
                let mut keys = BTreeSet::new();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.ws();
                    let raw_key = self.string(true)?;
                    let key: String =
                        serde_json::from_slice(raw_key).map_err(|_| DeliveryError::Json)?;
                    if key.len() > 128 || keys.len() >= 64 {
                        return Err(DeliveryError::Bound);
                    }
                    if !keys.insert(key) {
                        return Err(DeliveryError::DuplicateKey);
                    }
                    self.ws();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return Err(DeliveryError::Json);
                    }
                    self.at += 1;
                    self.value(depth + 1)?;
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err(DeliveryError::Json),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                self.ws();
                let mut count = 0usize;
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.ws();
                    self.value(depth + 1)?;
                    count += 1;
                    if count > 1000 {
                        return Err(DeliveryError::Bound);
                    }
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err(DeliveryError::Json),
                    }
                }
            }
            Some(b'"') => {
                self.string(false)?;
            }
            Some(_) => {
                let start = self.at;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
                {
                    self.at += 1;
                    if self.at - start > 128 {
                        return Err(DeliveryError::Bound);
                    }
                }
                if start == self.at {
                    return Err(DeliveryError::Json);
                }
            }
            None => return Err(DeliveryError::Json),
        }
        Ok(())
    }
}

fn scan_json_bounds(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_DELIVERY_JSON_BYTES {
        return Err(DeliveryError::InputTooLarge {
            limit: MAX_DELIVERY_JSON_BYTES,
        });
    }
    std::str::from_utf8(bytes).map_err(|_| DeliveryError::Utf8)?;
    let mut scanner = JsonScanner {
        bytes,
        at: 0,
        nodes: 0,
    };
    scanner.value(0)?;
    scanner.ws();
    if scanner.at != bytes.len() {
        return Err(DeliveryError::Json);
    }
    Ok(())
}
