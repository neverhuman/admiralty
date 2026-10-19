//! WO1: bounded, inert request contracts and deterministic previews.
//! No I/O, inference, clocks, persistence, execution or authority issuance.
//! Raw spans always index the original UTF-8 bytes, including JSON syntax and
//! escapes. Canonical identities are advisory content identities; only a writer
//! can revalidate current scope, source custody, exact command replay and CAS.
use crate::{
    CommandId, PrincipalId, ProjectId, RootId, TaskContract, TaskId, content_digest,
    limits::bounded_json_size,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const INTAKE_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_REQUESTS: usize = 256;
pub const MAX_OUTCOMES: usize = 1000;
pub const MAX_JSON_ITEM_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_SPANS: usize = 64;
pub const MAX_TARGETS: usize = 32;
pub const MAX_PAGE_ITEMS: usize = 100;
pub const MAX_JSON_DEPTH: usize = 32;
pub const MAX_JSON_NODES: usize = 32_768;
// Output includes escaped original text and bounded contract metadata. This is
// not an increase to the source/document admission limit above.
const MAX_TYPED_BYTES: usize = 8 * MAX_SOURCE_BYTES;
const MAX_FIELD_BYTES: usize = 4096;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum IntakeError {
    #[error("unsupported intake schema version")]
    Version,
    #[error("source exceeds {limit} byte policy limit")]
    SourceTooLarge { limit: usize },
    #[error("text exceeds {limit} byte format/policy limit")]
    TextTooLarge { limit: usize },
    #[error("invalid UTF-8 source")]
    Utf8,
    #[error("invalid or non-closed JSON input")]
    Json,
    #[error("duplicate JSON object key")]
    DuplicateKey,
    #[error("input exceeds nesting, item, field or page bound")]
    Bound,
    #[error("invalid intake field: {0}")]
    Invalid(&'static str),
    #[error("source span is not a nonempty raw UTF-8 byte range inside its item")]
    Span,
    #[error("invalid unchanged v1 task contract")]
    TaskContract,
    #[error("immutable task/delivery binding differs")]
    Binding,
}
type Result<T> = std::result::Result<T, IntakeError>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntakeFormat {
    PlainText,
    JsonBatch,
}

/// Caller-owned current authorization context, NEVER decoded from imported text.
/// This is a trusted composition input, not an independently issued capability.
#[derive(Clone, Debug)]
pub struct AuthorizedIntakeScope {
    pub principal: PrincipalId,
    pub project_id: Option<ProjectId>,
    pub root_id: Option<RootId>,
    pub authority_ref: String,
    pub authority_generation: u64,
}
/// Trusted caller policy can tighten these caps, never exceed public maxima.
#[derive(Clone, Debug)]
pub struct IntakePolicy {
    pub revision: String,
    pub max_source_bytes: usize,
    pub max_requests: usize,
    pub max_outcomes: usize,
    pub max_json_item_text_bytes: usize,
}
impl Default for IntakePolicy {
    fn default() -> Self {
        Self {
            revision: "wo1-v1".into(),
            max_source_bytes: MAX_SOURCE_BYTES,
            max_requests: MAX_REQUESTS,
            max_outcomes: MAX_OUTCOMES,
            max_json_item_text_bytes: MAX_JSON_ITEM_TEXT_BYTES,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeSnapshot {
    pub principal: PrincipalId,
    pub project_id: Option<ProjectId>,
    pub root_id: Option<RootId>,
    pub authority_ref: String,
    pub authority_generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequesterAttribution {
    Unknown {},
    Claimed { requester: String },
}
/// Opaque lookup data only: no fetching, path traversal or authorization.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OriginReference {
    pub system: String,
    pub record_id: String,
    pub revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub schema_version: u32,
    pub source_id: String,
    pub revision: String,
    /// Supplied observation time; no wall-clock lookup or freshness claim.
    pub captured_at: String,
    pub requester: RequesterAttribution,
    pub origin: Option<OriginReference>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
}
impl SourceSpan {
    pub fn validate(&self, raw: &str, within: Self) -> Result<()> {
        let (start, end) = (self.start as usize, self.end as usize);
        if within.start >= within.end
            || within.end as usize > raw.len()
            || !raw.is_char_boundary(within.start as usize)
            || !raw.is_char_boundary(within.end as usize)
            || start >= end
            || self.start < within.start
            || self.end > within.end
            || end > raw.len()
            || !raw.is_char_boundary(start)
            || !raw.is_char_boundary(end)
        {
            return Err(IntakeError::Span);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeInput {
    pub id: String,
    pub description: String,
    /// Raw source byte offsets, NOT decoded `text` offsets. Empty is unresolved.
    #[serde(default)]
    pub source_spans: Vec<SourceSpan>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub task: Option<TaskContract>,
    pub delivery: Option<DeliveryRequirement>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntakeItem {
    pub text: String,
    pub title: Option<String>,
    pub rationale: Option<String>,
    pub requested_timing: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub claimed_project_id: Option<ProjectId>,
    #[serde(default)]
    pub outcomes: Vec<OutcomeInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntakeBatch {
    pub schema_version: u32,
    pub items: Vec<IntakeItem>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedFact {
    ScopeUnknown,
    ClaimedScopeMismatch,
    AttributionUnknown,
    OutcomeUnknown,
    TaskContractMissing,
    CriteriaUnknown,
    SourceSpansUnknown,
    TaskScopeMismatch,
    DeliveryUnknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomePreview {
    pub outcome: OutcomeInput,
    /// Domain-separated exact outcome including its raw provenance spans.
    pub outcome_digest: String,
    pub contract_digest: Option<String>,
    pub unresolved: Vec<UnresolvedFact>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestPreview {
    pub index: u16,
    /// Whole plain-text input or exact raw JSON item object, including syntax.
    pub raw_span: SourceSpan,
    pub text: String,
    pub title: Option<String>,
    pub rationale: Option<String>,
    pub requested_timing: Option<String>,
    pub constraints: Vec<String>,
    pub claimed_project_id: Option<ProjectId>,
    pub outcomes: Vec<OutcomePreview>,
    pub unresolved: Vec<UnresolvedFact>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntakePreview {
    pub schema_version: u32,
    pub parser_revision: String,
    pub policy_revision: String,
    pub format: IntakeFormat,
    pub raw_digest: String,
    pub canonical_digest: String,
    pub raw_byte_length: u32,
    pub provenance: SourceProvenance,
    pub authorized_scope: ScopeSnapshot,
    pub requests: Vec<RequestPreview>,
    /// Binds all source spans, provenance, policy and authorized scope too.
    pub manifest_digest: String,
}
impl IntakePreview {
    /// Recompute all identities/spans/diagnostics from the original bytes under
    /// independently supplied current scope/policy. A serialized preview is
    /// data, never an authorization or a durable custody receipt.
    pub fn verify_source(
        &self,
        source: &[u8],
        scope: &AuthorizedIntakeScope,
        policy: &IntakePolicy,
    ) -> Result<()> {
        let expected = preview_intake(source, self.format, &self.provenance, scope, policy)?;
        if self == &expected {
            Ok(())
        } else {
            Err(IntakeError::Binding)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage {
    Candidate,
    Verified,
    Merged,
    Packaged,
    Staged,
    Installed,
    Running,
    Released,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTargetKind {
    Repository,
    Deployment,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetRequirement {
    Unknown {},
    Required { stage: DeliveryStage },
    Optional { stage: DeliveryStage },
    NotApplicable { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryTarget {
    pub id: String,
    pub target_kind: DeliveryTargetKind,
    pub requirement: TargetRequirement,
}
/// Outside TaskContractv1. A generation is a new immutable requirement, never
/// a rewrite of prior evidence. Unknown is never optional/not-applicable.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequirement {
    pub schema_version: u32,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub generation: u32,
    pub targets: Vec<DeliveryTarget>,
}
impl DeliveryRequirement {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        if self.contract_revision == 0
            || self.generation == 0
            || !digest(&self.contract_digest)
            || self.targets.is_empty()
            || self.targets.len() > MAX_TARGETS
        {
            return Err(IntakeError::Binding);
        }
        let mut seen = BTreeSet::new();
        for target in &self.targets {
            identifier(&target.id)?;
            if !seen.insert(&target.id) {
                return Err(IntakeError::Invalid("duplicate delivery target"));
            }
            if let TargetRequirement::NotApplicable { reason } = &target.requirement {
                text(reason, MAX_FIELD_BYTES)?;
            }
        }
        Ok(())
    }
    pub fn binds(&self, task: &TaskContract) -> Result<bool> {
        self.validate()?;
        validate_task(task)?;
        Ok(self.task_id == task.task_id
            && self.contract_revision == task.revision
            && self.contract_digest == content_digest(task).map_err(|_| IntakeError::Binding)?)
    }
}

/// Source references are claims until the Store verifies bytes and an
/// interlocked lifetime pin. Merely decoding an ArtifactClaim grants no custody.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourcePayload {
    InlineUtf8 {
        text: String,
    },
    ArtifactClaim {
        raw_digest: String,
        size_bytes: u32,
        artifact_ref: String,
        pin_receipt_ref: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkOrderRevision {
    pub schema_version: u32,
    pub id: String,
    pub revision: u32,
    pub scope: ScopeSnapshot,
    pub provenance: SourceProvenance,
    pub title: Option<String>,
    pub outcome: Option<String>,
    pub rationale: Option<String>,
    pub constraints: Vec<String>,
    pub requested_timing: Option<String>,
    pub raw_digest: String,
    pub canonical_digest: String,
    pub parser_revision: String,
    pub source_payload: SourcePayload,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkOrderTaskBinding {
    pub work_order_id: String,
    pub work_order_revision: u32,
    pub request_index: u16,
    pub outcome_id: String,
    pub outcome_digest: String,
    pub source_spans: Vec<SourceSpan>,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedTask {
    pub binding: WorkOrderTaskBinding,
    pub task: TaskContract,
    pub delivery: DeliveryRequirement,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordIntakeRequest {
    pub schema_version: u32,
    pub command_id: CommandId,
    pub expected_work_order_revision: u32,
    pub plan_id: String,
    pub expected_plan_revision: u32,
    pub preview_manifest_digest: String,
    pub policy_revision: String,
    pub authority_generation: u64,
    pub work_order: WorkOrderRevision,
    pub selected_tasks: Vec<SelectedTask>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordIntakeReceipt {
    pub schema_version: u32,
    pub command_id: CommandId,
    pub request_digest: String,
    pub work_order_id: String,
    pub work_order_revision: u32,
    pub plan_id: String,
    pub plan_revision: u32,
    pub preview_manifest_digest: String,
    pub task_bindings: Vec<WorkOrderTaskBinding>,
    /// True only when the writer returns an earlier identical command receipt.
    pub duplicate: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateReason {
    ExactCommandIdentity,
    ExactSourceIdentity,
    ExactCommandCollision,
    ExactSourceCollision,
    AdvisoryRawContent,
    AdvisoryCanonicalContent,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DuplicateQuery {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub raw_digest: String,
    pub canonical_digest: String,
    pub origin: Option<OriginReference>,
    pub command_id: Option<CommandId>,
    pub cursor: Option<String>,
    pub limit: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DuplicateCandidate {
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub reason: DuplicateReason,
    pub lifecycle: String,
    pub freshness: String,
    pub evidence_refs: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DuplicateCoverage {
    Complete {},
    Partial { continuation: String },
    Incomplete { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DuplicatePage {
    pub schema_version: u32,
    pub query_digest: String,
    pub candidates: Vec<DuplicateCandidate>,
    pub coverage: DuplicateCoverage,
}
impl DuplicateQuery {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        if self.limit == 0
            || usize::from(self.limit) > MAX_PAGE_ITEMS
            || !digest(&self.raw_digest)
            || !digest(&self.canonical_digest)
        {
            return Err(IntakeError::Bound);
        }
        if let Some(cursor) = &self.cursor {
            reference(cursor)?;
        }
        if let Some(origin) = &self.origin {
            validate_origin(origin)?;
        }
        Ok(())
    }
    pub fn identity_digest(&self) -> Result<String> {
        self.validate()?;
        canonical_hash(b"admiralty.intake.duplicate-query/v1\0", self)
    }
}
impl DuplicatePage {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        if self.candidates.len() > MAX_PAGE_ITEMS || !digest(&self.query_digest) {
            return Err(IntakeError::Bound);
        }
        for candidate in &self.candidates {
            if candidate.contract_revision == 0
                || !digest(&candidate.contract_digest)
                || candidate.evidence_refs.len() > MAX_SPANS
            {
                return Err(IntakeError::Bound);
            }
            text(&candidate.lifecycle, 128)?;
            text(&candidate.freshness, 128)?;
            for evidence in &candidate.evidence_refs {
                reference(evidence)?;
            }
        }
        match &self.coverage {
            DuplicateCoverage::Complete {} => {}
            DuplicateCoverage::Partial { continuation } => reference(continuation)?,
            DuplicateCoverage::Incomplete { reason } => text(reason, MAX_FIELD_BYTES)?,
        }
        bounded(self)
    }
    pub fn validate_for(&self, query: &DuplicateQuery) -> Result<()> {
        self.validate()?;
        query.validate()?;
        if self.query_digest != query.identity_digest()?
            || self.candidates.len() > usize::from(query.limit)
        {
            return Err(IntakeError::Binding);
        }
        Ok(())
    }
}

fn version(value: u32) -> Result<()> {
    if value == INTAKE_VERSION {
        Ok(())
    } else {
        Err(IntakeError::Version)
    }
}
fn identifier(value: &str) -> Result<()> {
    if value.len() > 128 || TaskId::new(value).is_err() {
        Err(IntakeError::Invalid("identifier"))
    } else {
        Ok(())
    }
}
fn text(value: &str, max: usize) -> Result<()> {
    if value.len() > max {
        return Err(IntakeError::TextTooLarge { limit: max });
    }
    if value.trim().is_empty() || value.contains('\0') {
        return Err(IntakeError::Invalid("empty/NUL text"));
    }
    Ok(())
}
fn reference(value: &str) -> Result<()> {
    text(value, 1024)?;
    // These are opaque references, not URLs/paths/commands. No resolution occurs.
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
        return Err(IntakeError::Invalid("opaque reference"));
    }
    Ok(())
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn bounded<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    bounded_json_size(value, MAX_TYPED_BYTES)
        .map(|_| ())
        .map_err(|_| IntakeError::Bound)
}
fn domain_hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}
fn canonical_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<String> {
    bounded(value)?;
    let canonical =
        content_digest(value).map_err(|_| IntakeError::Invalid("canonical encoding"))?;
    Ok(domain_hash(domain, canonical.as_bytes()))
}
fn validate_origin(origin: &OriginReference) -> Result<()> {
    identifier(&origin.system)?;
    reference(&origin.record_id)?;
    reference(&origin.revision)
}
fn validate_provenance(p: &SourceProvenance) -> Result<()> {
    version(p.schema_version)?;
    identifier(&p.source_id)?;
    reference(&p.revision)?;
    text(&p.captured_at, 128)?;
    if let RequesterAttribution::Claimed { requester } = &p.requester {
        text(requester, 512)?;
    }
    if let Some(origin) = &p.origin {
        validate_origin(origin)?;
    }
    Ok(())
}
fn validate_scope(s: &ScopeSnapshot) -> Result<()> {
    reference(&s.authority_ref)?;
    if s.authority_generation == 0 || (s.root_id.is_some() && s.project_id.is_none()) {
        return Err(IntakeError::Invalid("scope generation/project"));
    }
    Ok(())
}
fn validate_task(task: &TaskContract) -> Result<()> {
    // Bound all caller-owned collection work before serializing/cloning/hashing.
    if task.non_goals.len() > 64
        || task.owned_paths.len() > 128
        || task.forbidden_paths.len() > 128
        || task.read_first.len() > 128
        || task.dependencies.len() > MAX_OUTCOMES
        || task.criteria.len() > 128
        || task.checks.len() > 128
    {
        return Err(IntakeError::Bound);
    }
    for check in &task.checks {
        if check.args.len() > 128 {
            return Err(IntakeError::Bound);
        }
    }
    bounded_json_size(task, MAX_JSON_ITEM_TEXT_BYTES).map_err(|_| IntakeError::Bound)?;
    if task
        .owned_paths
        .iter()
        .chain(&task.forbidden_paths)
        .chain(&task.read_first)
        .chain(task.checks.iter().map(|check| &check.cwd))
        .any(|path| path.contains(':'))
    {
        return Err(IntakeError::TaskContract);
    }
    task.validate().map_err(|_| IntakeError::TaskContract)
}

/// Pure preview. Raw input/policy/provenance are bounded before clone/hash/parse.
/// PlainText accepts the full source-byte policy budget and never splits text.
/// JsonBatch has an explicit tighter per-item text limit in IntakePolicy.
pub fn preview_intake(
    source: &[u8],
    format: IntakeFormat,
    provenance: &SourceProvenance,
    scope: &AuthorizedIntakeScope,
    policy: &IntakePolicy,
) -> Result<IntakePreview> {
    if policy.max_source_bytes == 0
        || policy.max_source_bytes > MAX_SOURCE_BYTES
        || policy.max_requests == 0
        || policy.max_requests > MAX_REQUESTS
        || policy.max_outcomes == 0
        || policy.max_outcomes > MAX_OUTCOMES
        || policy.max_json_item_text_bytes == 0
        || policy.max_json_item_text_bytes > MAX_JSON_ITEM_TEXT_BYTES
    {
        return Err(IntakeError::Bound);
    }
    if source.len() > policy.max_source_bytes {
        return Err(IntakeError::SourceTooLarge {
            limit: policy.max_source_bytes,
        });
    }
    let raw = std::str::from_utf8(source).map_err(|_| IntakeError::Utf8)?;
    text(raw, policy.max_source_bytes)?;
    validate_provenance(provenance)?;
    reference(&policy.revision)?;
    reference(&scope.authority_ref)?;
    if scope.authority_generation == 0 || (scope.root_id.is_some() && scope.project_id.is_none()) {
        return Err(IntakeError::Invalid("scope"));
    }
    let (items, spans) = match format {
        IntakeFormat::PlainText => (
            vec![IntakeItem {
                text: raw.into(),
                title: None,
                rationale: None,
                requested_timing: None,
                constraints: vec![],
                claimed_project_id: None,
                outcomes: vec![],
            }],
            vec![SourceSpan {
                start: 0,
                end: source.len() as u32,
            }],
        ),
        IntakeFormat::JsonBatch => {
            let spans = scan_json(source)?;
            let batch: IntakeBatch =
                serde_json::from_slice(source).map_err(|_| IntakeError::Json)?;
            version(batch.schema_version)?;
            if batch.items.is_empty()
                || batch.items.len() > policy.max_requests
                || spans.len() != batch.items.len()
            {
                return Err(IntakeError::Bound);
            }
            (batch.items, spans)
        }
    };
    let mut total = 0_usize;
    let mut requests = Vec::with_capacity(items.len());
    for (index, (item, span)) in items.into_iter().zip(spans).enumerate() {
        text(
            &item.text,
            if format == IntakeFormat::PlainText {
                policy.max_source_bytes
            } else {
                policy.max_json_item_text_bytes
            },
        )?;
        if item.outcomes.len() > policy.max_outcomes {
            return Err(IntakeError::Bound);
        }
        total = total
            .checked_add(item.outcomes.len())
            .ok_or(IntakeError::Bound)?;
        if total > policy.max_outcomes {
            return Err(IntakeError::Bound);
        }
        if item.constraints.len() > 32 {
            return Err(IntakeError::Bound);
        }
        for constraint in &item.constraints {
            text(constraint, MAX_FIELD_BYTES)?;
        }
        for field in [&item.title, &item.rationale, &item.requested_timing]
            .into_iter()
            .flatten()
        {
            text(field, MAX_FIELD_BYTES)?;
        }
        let mut unresolved = Vec::new();
        if scope.project_id.is_none() || scope.root_id.is_none() {
            unresolved.push(UnresolvedFact::ScopeUnknown);
        }
        if item
            .claimed_project_id
            .as_ref()
            .is_some_and(|p| Some(p) != scope.project_id.as_ref())
        {
            unresolved.push(UnresolvedFact::ClaimedScopeMismatch);
        }
        if matches!(provenance.requester, RequesterAttribution::Unknown {}) {
            unresolved.push(UnresolvedFact::AttributionUnknown);
        }
        if item.outcomes.is_empty() {
            unresolved.extend([
                UnresolvedFact::OutcomeUnknown,
                UnresolvedFact::CriteriaUnknown,
            ]);
        }
        let mut outcomes = Vec::with_capacity(item.outcomes.len());
        let mut ids = BTreeSet::new();
        for outcome in item.outcomes {
            identifier(&outcome.id)?;
            text(&outcome.description, MAX_FIELD_BYTES)?;
            if !ids.insert(outcome.id.clone()) {
                return Err(IntakeError::Invalid("duplicate outcome ID"));
            }
            if outcome.source_spans.len() > MAX_SPANS || outcome.constraints.len() > 32 {
                return Err(IntakeError::Bound);
            }
            for s in &outcome.source_spans {
                s.validate(raw, span)?;
            }
            for constraint in &outcome.constraints {
                text(constraint, MAX_FIELD_BYTES)?;
            }
            let mut missing = Vec::new();
            if outcome.source_spans.is_empty() {
                missing.push(UnresolvedFact::SourceSpansUnknown);
            }
            let contract_digest = if let Some(task) = &outcome.task {
                validate_task(task)?;
                if Some(&task.project_id) != scope.project_id.as_ref()
                    || Some(&task.root_id) != scope.root_id.as_ref()
                {
                    missing.push(UnresolvedFact::TaskScopeMismatch);
                }
                Some(content_digest(task).map_err(|_| IntakeError::TaskContract)?)
            } else {
                missing.extend([
                    UnresolvedFact::TaskContractMissing,
                    UnresolvedFact::CriteriaUnknown,
                ]);
                None
            };
            if let Some(delivery) = &outcome.delivery {
                delivery.validate()?;
                if let Some(task) = &outcome.task
                    && !delivery.binds(task)?
                {
                    return Err(IntakeError::Binding);
                }
                if delivery
                    .targets
                    .iter()
                    .any(|t| matches!(t.requirement, TargetRequirement::Unknown {}))
                {
                    missing.push(UnresolvedFact::DeliveryUnknown);
                }
            } else {
                missing.push(UnresolvedFact::DeliveryUnknown);
            }
            outcomes.push(OutcomePreview {
                outcome_digest: canonical_hash(b"admiralty.intake.outcome/v1\0", &outcome)?,
                outcome,
                contract_digest,
                unresolved: missing,
            });
        }
        requests.push(RequestPreview {
            index: index as u16,
            raw_span: span,
            text: item.text,
            title: item.title,
            rationale: item.rationale,
            requested_timing: item.requested_timing,
            constraints: item.constraints,
            claimed_project_id: item.claimed_project_id,
            outcomes,
            unresolved,
        });
    }
    // Semantic identity excludes raw offsets/JSON whitespace but retains exact
    // decoded text, constraints, all task criteria/checks and delivery facts.
    let semantic: Vec<_> = requests
        .iter()
        .map(|r| {
            (
                &r.text,
                &r.title,
                &r.rationale,
                &r.requested_timing,
                &r.constraints,
                &r.claimed_project_id,
                r.outcomes
                    .iter()
                    .map(|o| {
                        (
                            &o.outcome.id,
                            &o.outcome.description,
                            &o.outcome.constraints,
                            &o.outcome.task,
                            &o.outcome.delivery,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let canonical_digest = canonical_hash(b"admiralty.intake.canonical/v1\0", &(format, semantic))?;
    let mut preview = IntakePreview {
        schema_version: INTAKE_VERSION,
        parser_revision: "wo1-json-raw-spans-v1".into(),
        policy_revision: policy.revision.clone(),
        format,
        raw_digest: domain_hash(b"admiralty.intake.raw/v1\0", source),
        canonical_digest,
        raw_byte_length: source.len() as u32,
        provenance: provenance.clone(),
        authorized_scope: ScopeSnapshot {
            principal: scope.principal.clone(),
            project_id: scope.project_id.clone(),
            root_id: scope.root_id.clone(),
            authority_ref: scope.authority_ref.clone(),
            authority_generation: scope.authority_generation,
        },
        requests,
        manifest_digest: String::new(),
    };
    preview.manifest_digest = canonical_hash(b"admiralty.intake.manifest/v1\0", &preview)?;
    Ok(preview)
}

// Preflight before serde allocation: bounded raw bytes, depth, strings,
// containers and node count; rejects duplicate keys before any Value could lose
// them. Exact raw top-level item ranges are collected before string unescaping.
struct Scanner<'a> {
    bytes: &'a [u8],
    max_string_bytes: usize,
    at: usize,
    nodes: usize,
    items: Vec<SourceSpan>,
}
impl Scanner<'_> {
    fn ws(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn string(&mut self, key: bool) -> Result<&[u8]> {
        let start = self.at;
        if self.bytes.get(self.at) != Some(&b'"') {
            return Err(IntakeError::Json);
        }
        self.at += 1;
        loop {
            let b = *self.bytes.get(self.at).ok_or(IntakeError::Json)?;
            self.at += 1;
            if self.at - start > if key { 770 } else { self.max_string_bytes + 2 } {
                return Err(if key {
                    IntakeError::Bound
                } else {
                    IntakeError::TextTooLarge {
                        limit: self.max_string_bytes,
                    }
                });
            }
            match b {
                b'"' => return Ok(&self.bytes[start..self.at]),
                b'\\' => {
                    self.at += 1;
                    if self.at > self.bytes.len() {
                        return Err(IntakeError::Json);
                    }
                }
                0..=31 => return Err(IntakeError::Json),
                _ => {}
            }
        }
    }
    fn value(&mut self, depth: usize, collect: bool) -> Result<()> {
        self.ws();
        self.nodes += 1;
        if depth > MAX_JSON_DEPTH || self.nodes > MAX_JSON_NODES {
            return Err(IntakeError::Bound);
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
                    let key: String = serde_json::from_slice(self.string(true)?)
                        .map_err(|_| IntakeError::Json)?;
                    if key.len() > 128 || keys.len() >= 64 {
                        return Err(IntakeError::Bound);
                    }
                    if !keys.insert(key.clone()) {
                        return Err(IntakeError::DuplicateKey);
                    }
                    self.ws();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return Err(IntakeError::Json);
                    }
                    self.at += 1;
                    self.value(depth + 1, depth == 0 && key == "items")?;
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err(IntakeError::Json),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                self.ws();
                let mut count = 0;
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(());
                }
                loop {
                    self.ws();
                    let start = self.at;
                    self.value(depth + 1, false)?;
                    count += 1;
                    if count > if collect { MAX_REQUESTS } else { MAX_OUTCOMES } {
                        return Err(IntakeError::Bound);
                    }
                    if collect {
                        self.items.push(SourceSpan {
                            start: start as u32,
                            end: self.at as u32,
                        });
                    }
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            break;
                        }
                        _ => return Err(IntakeError::Json),
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
                        return Err(IntakeError::Bound);
                    }
                }
                if start == self.at {
                    return Err(IntakeError::Json);
                }
            }
            None => return Err(IntakeError::Json),
        }
        Ok(())
    }
}
fn scan_json(source: &[u8]) -> Result<Vec<SourceSpan>> {
    scan_json_with_string_limit(source, MAX_JSON_ITEM_TEXT_BYTES)
}
fn scan_json_with_string_limit(source: &[u8], max_string_bytes: usize) -> Result<Vec<SourceSpan>> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(IntakeError::SourceTooLarge {
            limit: MAX_SOURCE_BYTES,
        });
    }
    std::str::from_utf8(source).map_err(|_| IntakeError::Utf8)?;
    let mut scan = Scanner {
        bytes: source,
        max_string_bytes,
        at: 0,
        nodes: 0,
        items: vec![],
    };
    scan.value(0, false)?;
    scan.ws();
    if scan.at != source.len() {
        return Err(IntakeError::Json);
    }
    Ok(scan.items)
}

impl WorkOrderRevision {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        identifier(&self.id)?;
        validate_scope(&self.scope)?;
        validate_provenance(&self.provenance)?;
        if self.revision == 0 || !digest(&self.raw_digest) || !digest(&self.canonical_digest) {
            return Err(IntakeError::Binding);
        }
        reference(&self.parser_revision)?;
        for field in [&self.title, &self.rationale, &self.requested_timing]
            .into_iter()
            .flatten()
        {
            text(field, MAX_FIELD_BYTES)?;
        }
        if let Some(outcome) = &self.outcome {
            text(outcome, MAX_FIELD_BYTES)?;
        }
        if self.constraints.len() > 32 {
            return Err(IntakeError::Bound);
        }
        for constraint in &self.constraints {
            text(constraint, MAX_FIELD_BYTES)?;
        }
        match &self.source_payload {
            SourcePayload::InlineUtf8 { text: raw } => {
                text(raw, MAX_SOURCE_BYTES)?;
                if domain_hash(b"admiralty.intake.raw/v1\0", raw.as_bytes()) != self.raw_digest {
                    return Err(IntakeError::Binding);
                }
            }
            SourcePayload::ArtifactClaim {
                raw_digest,
                size_bytes,
                artifact_ref,
                pin_receipt_ref,
            } => {
                if raw_digest != &self.raw_digest
                    || *size_bytes == 0
                    || *size_bytes as usize > MAX_SOURCE_BYTES
                {
                    return Err(IntakeError::Binding);
                }
                reference(artifact_ref)?;
                reference(pin_receipt_ref)?;
            }
        }
        bounded(self)
    }
}
impl WorkOrderTaskBinding {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.work_order_id)?;
        identifier(&self.outcome_id)?;
        if self.work_order_revision == 0
            || self.contract_revision == 0
            || usize::from(self.request_index) >= MAX_REQUESTS
            || self.source_spans.is_empty()
            || self.source_spans.len() > MAX_SPANS
            || !digest(&self.outcome_digest)
            || !digest(&self.contract_digest)
        {
            return Err(IntakeError::Binding);
        }
        for span in &self.source_spans {
            if span.start >= span.end || span.end as usize > MAX_SOURCE_BYTES {
                return Err(IntakeError::Span);
            }
        }
        Ok(())
    }
}
impl RecordIntakeRequest {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        identifier(&self.plan_id)?;
        reference(&self.policy_revision)?;
        if !digest(&self.preview_manifest_digest)
            || self.selected_tasks.is_empty()
            || self.selected_tasks.len() > MAX_OUTCOMES
            || self.authority_generation != self.work_order.scope.authority_generation
            || self.expected_work_order_revision.checked_add(1) != Some(self.work_order.revision)
        {
            return Err(IntakeError::Binding);
        }
        // Aggregate caller-owned bytes are bounded before source/task hashing;
        // per-contract collection/field checks below tighten the work further.
        bounded(self)?;
        self.work_order.validate()?;
        let mut tasks = BTreeSet::new();
        let mut outcomes = BTreeSet::new();
        for selected in &self.selected_tasks {
            let b = &selected.binding;
            b.validate()?;
            validate_task(&selected.task)?;
            if b.work_order_id != self.work_order.id
                || b.work_order_revision != self.work_order.revision
                || b.task_id != selected.task.task_id
                || b.contract_revision != selected.task.revision
                || b.contract_digest
                    != content_digest(&selected.task).map_err(|_| IntakeError::Binding)?
                || !selected.delivery.binds(&selected.task)?
                || !tasks.insert(&b.task_id)
                || !outcomes.insert((b.request_index, &b.outcome_id))
            {
                return Err(IntakeError::Binding);
            }
            if Some(&selected.task.project_id) != self.work_order.scope.project_id.as_ref()
                || Some(&selected.task.root_id) != self.work_order.scope.root_id.as_ref()
            {
                return Err(IntakeError::Binding);
            }
            if let SourcePayload::InlineUtf8 { text: raw } = &self.work_order.source_payload {
                for span in &b.source_spans {
                    span.validate(
                        raw,
                        SourceSpan {
                            start: 0,
                            end: raw.len() as u32,
                        },
                    )?;
                }
            }
        }
        bounded(self)
    }
}
impl RecordIntakeReceipt {
    pub fn validate(&self) -> Result<()> {
        version(self.schema_version)?;
        identifier(&self.work_order_id)?;
        identifier(&self.plan_id)?;
        if self.work_order_revision == 0
            || self.plan_revision == 0
            || !digest(&self.request_digest)
            || !digest(&self.preview_manifest_digest)
            || self.task_bindings.is_empty()
            || self.task_bindings.len() > MAX_OUTCOMES
        {
            return Err(IntakeError::Binding);
        }
        for binding in &self.task_bindings {
            binding.validate()?;
            if binding.work_order_id != self.work_order_id
                || binding.work_order_revision != self.work_order_revision
            {
                return Err(IntakeError::Binding);
            }
        }
        bounded(self)
    }
}
/// Bound and close a record contract; the Store must still compare the exact
/// preview/outcome digests, current scope/policy and source-pin custody atomically.
pub fn parse_record_request(bytes: &[u8]) -> Result<RecordIntakeRequest> {
    // The record envelope still has a1MiB wire cap. Its inline raw payload is
    // not a JsonBatch item and does not inherit that format's64KiB text cap.
    // A source whose escaped wrapper cannot fit uses an independently verified
    // artifact+pin at the future Store custody boundary, never silent truncation.
    scan_json_with_string_limit(bytes, MAX_SOURCE_BYTES)?;
    let request: RecordIntakeRequest =
        serde_json::from_slice(bytes).map_err(|_| IntakeError::Json)?;
    request.validate()?;
    Ok(request)
}
