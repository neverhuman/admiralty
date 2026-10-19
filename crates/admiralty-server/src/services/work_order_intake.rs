//! Default-deny, writer-linearized work-order intake service adapter.
//!
//! This module composes `admiralty_core::intake::preview_intake` with the
//! `admiralty_store::Store` sole writer (`capture_intake`, `captured_intake`,
//! `record_intake`, `get_work_order`, and `find_duplicate_candidates`).
//!
//! Security and authority boundaries:
//! - Opaque bearer credentials are held only inside `WorkOrderIntakeAuthRegistry`,
//!   never implement `Debug`, `Clone`, `Serialize`, or `Deserialize`, are
//!   compared using a bounded loop without prefix short-circuiting in source
//!   (work still varies with slice length and is not a compiler-verified
//!   constant-time primitive), and perform best-effort buffer clearing on drop
//!   (`Vec::fill(0)` + `compiler_fence`, which is not a certified cryptographic
//!   zeroization guarantee).
//! - `AuthenticatedIntakeActor`, `IntakeScopeGrant`, `WorkOrderIntakeAuthRegistry`,
//!   and `WorkOrderIntakeAdmissionBridge` never implement `Deserialize`.
//! - Authenticated actor identity (`AuthenticatedIntakeActor`) is strictly
//!   separate from public `ScopeSnapshot` fields and `RequesterAttribution`
//!   claims. Inbox `ReadPending` / `AcknowledgeApplication` grants never
//!   authorize any intake operation.
//! - Pre-queue checks are admission filters only and do not by themselves claim
//!   linearizability with Store commit. `WorkOrderIntakeAdmissionBridge`
//!   implements both `IntakeCaptureAuthority` and `IntakeAuthority` and rechecks
//!   current credential revocation, `authority_generation`, scope, and the
//!   independent `WorkOrderIntakePolicy` synchronously on the Store writer turn
//!   (`AuthorizationStage::WriterTurn`) for every command and historical retry,
//!   re-resolving and verifying exact grant/scope/policy identity after
//!   `WorkOrderIntakePolicy::authorize_current` returns `true`. External
//!   credential/grant or policy changes occurring after that final check still
//!   require issuer synchronization; no stronger global linearization guarantee
//!   is claimed.
//! - No startup issuer, HTTP route, automatic decomposition, model/provider
//!   effect, check execution, task acceptance, or outbox dispatch is installed
//!   by this module.

use admiralty_core::{
    CommandId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
    intake::{
        self, AuthorizedIntakeScope, DuplicatePage, DuplicateQuery, IntakeError, IntakeFormat,
        IntakePolicy, IntakePreview, RecordIntakeRequest, RequesterAttribution, ScopeSnapshot,
        SourcePayload, SourceProvenance,
    },
    limits::bounded_json_size,
    workplans::WorkplanError,
};
use admiralty_store::{
    Store, StoreError,
    intake::{
        AuthorizedIntakeContext, IntakeAuthority, IntakeRecordResult, IntakeStoreError,
        RegisteredWorkOrder,
    },
    intake_capture::{
        CaptureError, CaptureIntakeRequest, CaptureResult, CapturedIntake, IntakeCaptureAuthority,
    },
    workplans::WorkplanStoreError,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{Ordering, compiler_fence},
    },
    time::{Duration, Instant},
};

pub const MAX_INTAKE_CREDENTIALS: usize = 128;
pub const MAX_GRANTS_PER_CREDENTIAL: usize = 64;
pub const MAX_TOTAL_INTAKE_GRANTS: usize = 1024;
pub const MIN_INTAKE_TOKEN_BYTES: usize = 32;
pub const MAX_INTAKE_TOKEN_BYTES: usize = 4096;
pub const MAX_CREDENTIAL_ID_BYTES: usize = 128;
pub const MAX_INFLIGHT_INTAKE_ADMISSIONS: usize = 64;
pub const DEFAULT_INFLIGHT_TTL: Duration = Duration::from_secs(30);
pub const MAX_CAPTURE_REQUEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORD_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_DUPLICATE_QUERY_BYTES: usize = 16 * 1024;
const MAX_CAPTURE_REVISIONS: u32 = 32;
const MAX_WORK_ORDER_REVISIONS: u32 = 64;
const MAX_REFERENCE_BYTES: usize = 1024;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_CLAIM_REQUESTER_BYTES: usize = 512;
const MAX_CAPTURED_AT_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkOrderIntakeAction {
    PreviewRaw,
    CaptureRaw,
    RecordEnriched,
    ReadCaptured,
    ReadWorkOrder,
    QueryDuplicates,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationStage {
    ServicePreQueue,
    WriterTurn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntakeMutationKind {
    CaptureRaw,
    RecordEnriched,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkOrderIntakeError {
    AccessDenied,
    InvalidInput(&'static str),
    ArtifactClaimUnavailable,
    UnresolvedPlainTextEnrichment,
    SelectionMismatch,
    Collision,
    ConcurrentInflightCollision,
    StaleCurrent,
    CapacityExceeded,
    IntegrityFailure,
    StorageUnavailable,
}

impl std::fmt::Display for WorkOrderIntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccessDenied => f.write_str("work-order intake action is not authorized"),
            Self::InvalidInput(reason) => {
                write!(f, "work-order intake input is invalid: {reason}")
            }
            Self::ArtifactClaimUnavailable => {
                f.write_str("verified artifact custody and interlocked pin composition is unavailable")
            }
            Self::UnresolvedPlainTextEnrichment => {
                f.write_str("raw plain-text input remains unresolved and cannot be recorded as an enriched work order")
            }
            Self::SelectionMismatch => {
                f.write_str("explicit outcome, task, or source-span selection does not match preview")
            }
            Self::Collision => {
                f.write_str("immutable intake command or source identity has conflicting content")
            }
            Self::ConcurrentInflightCollision => {
                f.write_str("an inflight intake command with the same identity is already active")
            }
            Self::StaleCurrent => {
                f.write_str("work-order, capture, or workplan compare-and-swap revision is stale")
            }
            Self::CapacityExceeded => {
                f.write_str("work-order intake request or history exceeds bounded capacity")
            }
            Self::IntegrityFailure => f.write_str("stored work-order intake consistency check failed"),
            Self::StorageUnavailable => f.write_str("work-order intake storage is unavailable"),
        }
    }
}

impl std::error::Error for WorkOrderIntakeError {}

/// Monotonic process-local clock seam for bounded inflight admission TTLs.
/// Never accepts caller-supplied wall-clock timestamps.
pub trait MonotonicClock: Send + Sync {
    fn now_nanos(&self) -> u64;
}

/// Process-local monotonic clock anchored to construction `Instant`.
pub struct SteadyMonotonicClock {
    origin: Instant,
}

impl SteadyMonotonicClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SteadyMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SteadyMonotonicClock {
    fn now_nanos(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

struct ProtectedToken(Vec<u8>);

impl ProtectedToken {
    fn new(raw: String) -> Self {
        Self(raw.into_bytes())
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// Best-effort in-process overwrite on drop (`Vec::fill(0)` + `compiler_fence`).
/// This provides hygiene against accidental retention in ordinary buffers, not
/// a compiler-certified cryptographic zeroization guarantee.
impl Drop for ProtectedToken {
    fn drop(&mut self) {
        self.0.fill(0);
        compiler_fence(Ordering::SeqCst);
    }
}

/// Bounded byte comparison without prefix early-return in source.
/// Work still varies with slice length and this is not a compiler-verified
/// constant-time cryptographic primitive.
fn bounded_no_prefix_short_circuit_equal(expected: &[u8], candidate: &[u8]) -> bool {
    let max_len = expected.len().max(candidate.len());
    let mut diff = expected.len() ^ candidate.len();
    for idx in 0..max_len {
        let a = expected.get(idx).copied().unwrap_or(0);
        let b = candidate.get(idx).copied().unwrap_or(0);
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

fn valid_opaque_token_bytes(bytes: &[u8]) -> bool {
    (MIN_INTAKE_TOKEN_BYTES..=MAX_INTAKE_TOKEN_BYTES).contains(&bytes.len())
        && bytes.iter().all(|byte| (b'!'..=b'~').contains(byte))
}

fn valid_credential_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CREDENTIAL_ID_BYTES
        && id.trim() == id
        && !id.chars().any(char::is_control)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES && TaskId::new(value).is_ok()
}

fn valid_opaque_reference(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || value.trim().is_empty()
        || value.contains('\0')
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || value.contains(['/', '\\'])
        || value.starts_with('.')
        || value.contains("..")
    {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    !["javascript:", "data:", "file:", "http:", "https:", "ssh:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn same_policy(a: &IntakePolicy, b: &IntakePolicy) -> bool {
    a.revision == b.revision
        && a.max_source_bytes == b.max_source_bytes
        && a.max_requests == b.max_requests
        && a.max_outcomes == b.max_outcomes
        && a.max_json_item_text_bytes == b.max_json_item_text_bytes
}

fn same_authorized_scope(a: &AuthorizedIntakeScope, b: &AuthorizedIntakeScope) -> bool {
    a.principal == b.principal
        && a.project_id == b.project_id
        && a.root_id == b.root_id
        && a.authority_ref == b.authority_ref
        && a.authority_generation == b.authority_generation
}

/// Constructed only by authenticating a configured opaque token against
/// `WorkOrderIntakeAuthRegistry`. Never deserialized or forged from labels.
#[derive(Clone)]
pub struct AuthenticatedIntakeActor {
    credential_id: String,
    principal: PrincipalId,
    incarnation: Arc<()>,
}

impl PartialEq for AuthenticatedIntakeActor {
    fn eq(&self, other: &Self) -> bool {
        self.credential_id == other.credential_id
            && self.principal == other.principal
            && Arc::ptr_eq(&self.incarnation, &other.incarnation)
    }
}

impl Eq for AuthenticatedIntakeActor {}

impl AuthenticatedIntakeActor {
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }

    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }
}

/// Service-local grant bound to an opaque credential in trusted server memory.
/// Never deserialized from request payloads.
#[derive(Clone, Debug)]
pub struct IntakeScopeGrant {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub authority_ref: String,
    pub authority_generation: u64,
    pub policy: IntakePolicy,
    pub allowed_actions: BTreeSet<WorkOrderIntakeAction>,
}

impl PartialEq for IntakeScopeGrant {
    fn eq(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.root_id == other.root_id
            && self.authority_ref == other.authority_ref
            && self.authority_generation == other.authority_generation
            && same_policy(&self.policy, &other.policy)
            && self.allowed_actions == other.allowed_actions
    }
}

impl Eq for IntakeScopeGrant {}

impl IntakeScopeGrant {
    fn validate_and_digest(&self) -> Result<String, WorkOrderIntakeError> {
        if !valid_identifier(self.project_id.as_str())
            || !valid_identifier(self.root_id.as_str())
            || !valid_opaque_reference(&self.authority_ref)
            || self.authority_generation == 0
            || self.allowed_actions.is_empty()
            || self.allowed_actions.len() > 6
        {
            return Err(WorkOrderIntakeError::InvalidInput(
                "intake scope grant fields are outside bounds",
            ));
        }
        self.policy.identity_digest().map_err(|_| {
            WorkOrderIntakeError::InvalidInput("intake policy limits or revision are invalid")
        })
    }

    fn authorized_scope(&self, principal: &PrincipalId) -> AuthorizedIntakeScope {
        AuthorizedIntakeScope {
            principal: principal.clone(),
            project_id: Some(self.project_id.clone()),
            root_id: Some(self.root_id.clone()),
            authority_ref: self.authority_ref.clone(),
            authority_generation: self.authority_generation,
        }
    }
}

/// Configured credential input for `WorkOrderIntakeAuthRegistry`.
/// Holds the raw token inside a non-Debug/non-Clone buffer that performs
/// best-effort byte clearing on drop.
pub struct WorkOrderIntakeCredential {
    pub credential_id: String,
    pub actor: PrincipalId,
    pub grants: Vec<IntakeScopeGrant>,
    token: ProtectedToken,
}

impl WorkOrderIntakeCredential {
    pub fn configured(
        credential_id: String,
        token: String,
        actor: PrincipalId,
        grants: Vec<IntakeScopeGrant>,
    ) -> Self {
        Self {
            credential_id,
            actor,
            grants,
            token: ProtectedToken::new(token),
        }
    }
}

#[derive(Clone, Debug)]
struct ValidatedGrant {
    grant: IntakeScopeGrant,
    policy_digest: String,
}

struct CredentialEntry {
    credential_id: String,
    token: ProtectedToken,
    actor: PrincipalId,
    grants: BTreeMap<(ProjectId, RootId), ValidatedGrant>,
    revoked: bool,
}

#[derive(Clone, Debug)]
struct ResolvedGrant {
    scope: AuthorizedIntakeScope,
    policy: IntakePolicy,
    policy_digest: String,
}

fn same_resolved_grant(a: &ResolvedGrant, b: &ResolvedGrant) -> bool {
    same_authorized_scope(&a.scope, &b.scope)
        && same_policy(&a.policy, &b.policy)
        && a.policy_digest == b.policy_digest
}

/// In-memory credential & scope-grant registry for work-order intake.
/// Deliberately has no `Debug` or `Serialize`/`Deserialize` implementation.
pub struct WorkOrderIntakeAuthRegistry {
    credentials: RwLock<Vec<CredentialEntry>>,
    incarnation: Arc<()>,
}

impl WorkOrderIntakeAuthRegistry {
    pub fn new(credentials: Vec<WorkOrderIntakeCredential>) -> Result<Self, WorkOrderIntakeError> {
        if credentials.len() > MAX_INTAKE_CREDENTIALS {
            return Err(WorkOrderIntakeError::CapacityExceeded);
        }
        let mut seen_ids = BTreeSet::new();
        let mut total_grants = 0_usize;
        let mut entries: Vec<CredentialEntry> = Vec::with_capacity(credentials.len());

        for credential in credentials {
            let actor_str = credential.actor.as_str();
            if !valid_credential_id(&credential.credential_id)
                || actor_str.is_empty()
                || actor_str.len() > MAX_IDENTIFIER_BYTES
                || actor_str.chars().any(char::is_control)
                || !valid_opaque_token_bytes(credential.token.as_slice())
                || credential.grants.is_empty()
                || credential.grants.len() > MAX_GRANTS_PER_CREDENTIAL
                || !seen_ids.insert(credential.credential_id.clone())
            {
                return Err(WorkOrderIntakeError::InvalidInput(
                    "intake credential configuration is invalid",
                ));
            }
            let mut duplicate_token = false;
            for prior in &entries {
                if bounded_no_prefix_short_circuit_equal(
                    prior.token.as_slice(),
                    credential.token.as_slice(),
                ) {
                    duplicate_token = true;
                }
            }
            if duplicate_token {
                return Err(WorkOrderIntakeError::InvalidInput(
                    "duplicate intake credential token",
                ));
            }
            total_grants = total_grants
                .checked_add(credential.grants.len())
                .ok_or(WorkOrderIntakeError::CapacityExceeded)?;
            if total_grants > MAX_TOTAL_INTAKE_GRANTS {
                return Err(WorkOrderIntakeError::CapacityExceeded);
            }

            let mut grant_map = BTreeMap::new();
            for grant in credential.grants {
                let policy_digest = grant.validate_and_digest()?;
                let key = (grant.project_id.clone(), grant.root_id.clone());
                if grant_map.contains_key(&key) {
                    return Err(WorkOrderIntakeError::InvalidInput(
                        "duplicate project/root grant in credential",
                    ));
                }
                grant_map.insert(
                    key,
                    ValidatedGrant {
                        grant,
                        policy_digest,
                    },
                );
            }
            entries.push(CredentialEntry {
                credential_id: credential.credential_id,
                token: credential.token,
                actor: credential.actor,
                grants: grant_map,
                revoked: false,
            });
        }

        Ok(Self {
            credentials: RwLock::new(entries),
            incarnation: Arc::new(()),
        })
    }

    /// Authenticate an opaque bearer token using a bounded comparison loop
    /// without prefix short-circuiting across configured credentials (work still
    /// scales with slice length and is not a certified constant-time primitive).
    pub fn authenticate(
        &self,
        token: &str,
    ) -> Result<AuthenticatedIntakeActor, WorkOrderIntakeError> {
        let candidate = token.as_bytes();
        if !valid_opaque_token_bytes(candidate) {
            return Err(WorkOrderIntakeError::InvalidInput(
                "bearer token format is invalid",
            ));
        }
        let guard = self
            .credentials
            .read()
            .map_err(|_| WorkOrderIntakeError::AccessDenied)?;
        let mut matched: Option<(&str, &PrincipalId)> = None;
        for entry in guard.iter() {
            let token_matches =
                bounded_no_prefix_short_circuit_equal(entry.token.as_slice(), candidate);
            if token_matches && !entry.revoked {
                matched = Some((&entry.credential_id, &entry.actor));
            }
        }
        let Some((credential_id, principal)) = matched else {
            return Err(WorkOrderIntakeError::AccessDenied);
        };
        Ok(AuthenticatedIntakeActor {
            credential_id: credential_id.to_owned(),
            principal: principal.clone(),
            incarnation: Arc::clone(&self.incarnation),
        })
    }

    /// Immediately revoke a credential. Takes effect on all subsequent service
    /// calls and synchronous Store writer turns, including queued commands and
    /// historical retries.
    pub fn revoke(&self, credential_id: &str) -> Result<(), WorkOrderIntakeError> {
        if !valid_credential_id(credential_id) {
            return Err(WorkOrderIntakeError::InvalidInput(
                "credential_id is outside bounds",
            ));
        }
        let mut guard = self
            .credentials
            .write()
            .map_err(|_| WorkOrderIntakeError::AccessDenied)?;
        let entry = guard
            .iter_mut()
            .find(|entry| entry.credential_id == credential_id)
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        entry.revoked = true;
        Ok(())
    }

    /// Update the authoritative generation and/or policy for a configured
    /// `(credential_id, project_id, root_id)` grant in-place.
    pub fn update_grant(
        &self,
        credential_id: &str,
        project_id: &ProjectId,
        root_id: &RootId,
        authority_generation: u64,
        policy: IntakePolicy,
    ) -> Result<(), WorkOrderIntakeError> {
        if !valid_credential_id(credential_id) || authority_generation == 0 {
            return Err(WorkOrderIntakeError::InvalidInput(
                "grant update parameters are invalid",
            ));
        }
        let policy_digest = policy
            .identity_digest()
            .map_err(|_| WorkOrderIntakeError::InvalidInput("updated intake policy is invalid"))?;
        let mut guard = self
            .credentials
            .write()
            .map_err(|_| WorkOrderIntakeError::AccessDenied)?;
        let entry = guard
            .iter_mut()
            .find(|entry| entry.credential_id == credential_id && !entry.revoked)
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        let slot = entry
            .grants
            .get_mut(&(project_id.clone(), root_id.clone()))
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        slot.grant.authority_generation = authority_generation;
        slot.grant.policy = policy;
        slot.policy_digest = policy_digest;
        Ok(())
    }

    fn resolve_current_grant(
        &self,
        actor: &AuthenticatedIntakeActor,
        project_id: &ProjectId,
        root_id: &RootId,
        action: WorkOrderIntakeAction,
    ) -> Result<ResolvedGrant, WorkOrderIntakeError> {
        if !Arc::ptr_eq(&self.incarnation, &actor.incarnation) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        let guard = self
            .credentials
            .read()
            .map_err(|_| WorkOrderIntakeError::AccessDenied)?;
        let entry = guard
            .iter()
            .find(|entry| {
                entry.credential_id == actor.credential_id
                    && !entry.revoked
                    && entry.actor == actor.principal
            })
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        let validated = entry
            .grants
            .get(&(project_id.clone(), root_id.clone()))
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        if !validated.grant.allowed_actions.contains(&action) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        Ok(ResolvedGrant {
            scope: validated.grant.authorized_scope(&entry.actor),
            policy: validated.grant.policy.clone(),
            policy_digest: validated.policy_digest.clone(),
        })
    }
}

/// Complete request binding evaluated at both `AuthorizationStage::ServicePreQueue`
/// and `AuthorizationStage::WriterTurn`.
pub struct WorkOrderIntakeBinding<'a> {
    pub stage: AuthorizationStage,
    pub action: WorkOrderIntakeAction,
    pub credential_id: &'a str,
    pub authenticated_actor: &'a PrincipalId,
    pub attributed_requester: Option<&'a RequesterAttribution>,
    pub scope: &'a AuthorizedIntakeScope,
    pub policy: &'a IntakePolicy,
    pub policy_digest: &'a str,
    pub command_id: Option<&'a CommandId>,
    pub target_id: Option<&'a str>,
    pub revision: Option<u32>,
    pub request_digest: &'a str,
}

/// Default-deny independent action policy. If absent (`None`), all operations
/// fail closed with `WorkOrderIntakeError::AccessDenied`.
pub trait WorkOrderIntakePolicy: Send + Sync {
    fn authorize_current(&self, binding: &WorkOrderIntakeBinding<'_>) -> bool;
}

#[derive(Clone)]
struct InflightTicket {
    ticket_nonce: u64,
    mutation_kind: IntakeMutationKind,
    command_id: CommandId,
    request_digest: String,
    actor: AuthenticatedIntakeActor,
    project_id: ProjectId,
    root_id: RootId,
    pre_queue_grant: ResolvedGrant,
    expires_at_nanos: u64,
}

struct InflightTable {
    next_nonce: u64,
    entries: BTreeMap<(IntakeMutationKind, CommandId), InflightTicket>,
}

impl InflightTable {
    fn new() -> Self {
        Self {
            next_nonce: 1,
            entries: BTreeMap::new(),
        }
    }

    fn sweep_expired(&mut self, now_nanos: u64) {
        self.entries
            .retain(|_, ticket| now_nanos < ticket.expires_at_nanos);
    }
}

/// Shared in-process policy and writer-turn admission bridge implementing both
/// `IntakeCaptureAuthority` and `IntakeAuthority`.
pub struct WorkOrderIntakeAdmissionBridge {
    registry: Arc<WorkOrderIntakeAuthRegistry>,
    policy: Option<Arc<dyn WorkOrderIntakePolicy>>,
    clock: Arc<dyn MonotonicClock>,
    ttl_nanos: u64,
    max_inflight: usize,
    inflight: Mutex<InflightTable>,
}

pub struct InflightAdmissionGuard<'a> {
    bridge: &'a WorkOrderIntakeAdmissionBridge,
    key: (IntakeMutationKind, CommandId),
    ticket_nonce: u64,
}

impl Drop for InflightAdmissionGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut table) = self.bridge.inflight.lock()
            && table
                .entries
                .get(&self.key)
                .is_some_and(|ticket| ticket.ticket_nonce == self.ticket_nonce)
        {
            table.entries.remove(&self.key);
        }
    }
}

impl WorkOrderIntakeAdmissionBridge {
    pub fn new(
        registry: Arc<WorkOrderIntakeAuthRegistry>,
        policy: Option<Arc<dyn WorkOrderIntakePolicy>>,
    ) -> Self {
        Self::with_clock_and_limits(
            registry,
            policy,
            Arc::new(SteadyMonotonicClock::new()),
            DEFAULT_INFLIGHT_TTL,
            MAX_INFLIGHT_INTAKE_ADMISSIONS,
        )
    }

    pub fn with_clock_and_limits(
        registry: Arc<WorkOrderIntakeAuthRegistry>,
        policy: Option<Arc<dyn WorkOrderIntakePolicy>>,
        clock: Arc<dyn MonotonicClock>,
        ttl: Duration,
        max_inflight: usize,
    ) -> Self {
        let ttl_nanos = u64::try_from(ttl.as_nanos()).unwrap_or(u64::MAX).max(1);
        let max_inflight = max_inflight.clamp(1, MAX_INFLIGHT_INTAKE_ADMISSIONS);
        Self {
            registry,
            policy,
            clock,
            ttl_nanos,
            max_inflight,
            inflight: Mutex::new(InflightTable::new()),
        }
    }

    pub fn active_inflight_count(&self) -> Result<usize, WorkOrderIntakeError> {
        let now = self.clock.now_nanos();
        let mut table = self
            .inflight
            .lock()
            .map_err(|_| WorkOrderIntakeError::StorageUnavailable)?;
        table.sweep_expired(now);
        Ok(table.entries.len())
    }

    #[allow(clippy::too_many_arguments)]
    fn authorize_at_stage(
        &self,
        stage: AuthorizationStage,
        action: WorkOrderIntakeAction,
        actor: &AuthenticatedIntakeActor,
        project_id: &ProjectId,
        root_id: &RootId,
        attributed_requester: Option<&RequesterAttribution>,
        command_id: Option<&CommandId>,
        target_id: Option<&str>,
        revision: Option<u32>,
        request_digest: &str,
    ) -> Result<ResolvedGrant, WorkOrderIntakeError> {
        let policy = self
            .policy
            .as_ref()
            .ok_or(WorkOrderIntakeError::AccessDenied)?;
        let resolved_before = self
            .registry
            .resolve_current_grant(actor, project_id, root_id, action)?;
        let binding = WorkOrderIntakeBinding {
            stage,
            action,
            credential_id: actor.credential_id(),
            authenticated_actor: actor.principal(),
            attributed_requester,
            scope: &resolved_before.scope,
            policy: &resolved_before.policy,
            policy_digest: &resolved_before.policy_digest,
            command_id,
            target_id,
            revision,
            request_digest,
        };
        if !policy.authorize_current(&binding) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        let resolved_after = self
            .registry
            .resolve_current_grant(actor, project_id, root_id, action)?;
        if !same_resolved_grant(&resolved_before, &resolved_after) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        Ok(resolved_after)
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_inflight(
        &self,
        mutation_kind: IntakeMutationKind,
        command_id: CommandId,
        request_digest: String,
        actor: &AuthenticatedIntakeActor,
        project_id: ProjectId,
        root_id: RootId,
        pre_queue_grant: ResolvedGrant,
    ) -> Result<InflightAdmissionGuard<'_>, WorkOrderIntakeError> {
        let now = self.clock.now_nanos();
        let expires_at_nanos = now.saturating_add(self.ttl_nanos);
        let mut table = self
            .inflight
            .lock()
            .map_err(|_| WorkOrderIntakeError::StorageUnavailable)?;
        table.sweep_expired(now);
        let key = (mutation_kind, command_id.clone());
        if table.entries.contains_key(&key) {
            return Err(WorkOrderIntakeError::ConcurrentInflightCollision);
        }
        if table.entries.len() >= self.max_inflight {
            return Err(WorkOrderIntakeError::CapacityExceeded);
        }
        let ticket_nonce = table.next_nonce;
        table.next_nonce = table
            .next_nonce
            .checked_add(1)
            .ok_or(WorkOrderIntakeError::CapacityExceeded)?;
        table.entries.insert(
            key.clone(),
            InflightTicket {
                ticket_nonce,
                mutation_kind,
                command_id,
                request_digest,
                actor: actor.clone(),
                project_id,
                root_id,
                pre_queue_grant,
                expires_at_nanos,
            },
        );
        Ok(InflightAdmissionGuard {
            bridge: self,
            key,
            ticket_nonce,
        })
    }

    fn take_matching_inflight(
        &self,
        mutation_kind: IntakeMutationKind,
        command_id: &CommandId,
        request_digest: &str,
    ) -> Option<InflightTicket> {
        let now = self.clock.now_nanos();
        let mut table = self.inflight.lock().ok()?;
        table.sweep_expired(now);
        let key = (mutation_kind, command_id.clone());
        let matches = table.entries.get(&key).is_some_and(|ticket| {
            ticket.mutation_kind == mutation_kind
                && &ticket.command_id == command_id
                && ticket.request_digest == request_digest
        });
        if matches {
            table.entries.remove(&key)
        } else {
            None
        }
    }
}

impl IntakeCaptureAuthority for WorkOrderIntakeAdmissionBridge {
    fn authorize(
        &self,
        request: &CaptureIntakeRequest,
    ) -> Result<AuthorizedIntakeContext, CaptureError> {
        validate_capture_request_pre_hash(request).map_err(map_service_to_capture_error)?;
        let digest = capture_request_digest(request).map_err(|_| CaptureError::Denied)?;
        let ticket = self
            .take_matching_inflight(IntakeMutationKind::CaptureRaw, &request.command_id, &digest)
            .ok_or(CaptureError::Denied)?;

        let resolved = self
            .authorize_at_stage(
                AuthorizationStage::WriterTurn,
                WorkOrderIntakeAction::CaptureRaw,
                &ticket.actor,
                &ticket.project_id,
                &ticket.root_id,
                Some(&request.provenance.requester),
                Some(&request.command_id),
                Some(&request.capture_id),
                Some(request.revision),
                &digest,
            )
            .map_err(|_| CaptureError::Denied)?;

        if !same_resolved_grant(&ticket.pre_queue_grant, &resolved)
            || !scope_snapshot_matches(&request.scope, &resolved.scope)
            || request.policy_revision != resolved.policy.revision
            || request.policy_digest != resolved.policy_digest
        {
            return Err(CaptureError::Denied);
        }
        Ok(AuthorizedIntakeContext {
            scope: resolved.scope,
            policy: resolved.policy,
        })
    }
}

impl IntakeAuthority for WorkOrderIntakeAdmissionBridge {
    fn authorize(
        &self,
        request: &RecordIntakeRequest,
    ) -> Result<AuthorizedIntakeContext, IntakeStoreError> {
        validate_record_request_pre_hash(request).map_err(map_service_to_record_error)?;
        let digest = record_request_digest(request).map_err(|_| IntakeStoreError::Denied)?;
        let ticket = self
            .take_matching_inflight(
                IntakeMutationKind::RecordEnriched,
                &request.command_id,
                &digest,
            )
            .ok_or(IntakeStoreError::Denied)?;

        let resolved = self
            .authorize_at_stage(
                AuthorizationStage::WriterTurn,
                WorkOrderIntakeAction::RecordEnriched,
                &ticket.actor,
                &ticket.project_id,
                &ticket.root_id,
                Some(&request.work_order.provenance.requester),
                Some(&request.command_id),
                Some(&request.work_order.id),
                Some(request.work_order.revision),
                &digest,
            )
            .map_err(|_| IntakeStoreError::Denied)?;

        if !same_resolved_grant(&ticket.pre_queue_grant, &resolved)
            || !scope_snapshot_matches(&request.work_order.scope, &resolved.scope)
            || request.authority_generation != resolved.scope.authority_generation
            || request.policy_revision != resolved.policy.revision
            || request.policy_digest != resolved.policy_digest
        {
            return Err(IntakeStoreError::Denied);
        }
        Ok(AuthorizedIntakeContext {
            scope: resolved.scope,
            policy: resolved.policy,
        })
    }
}

fn scope_snapshot_matches(snapshot: &ScopeSnapshot, scope: &AuthorizedIntakeScope) -> bool {
    snapshot.principal == scope.principal
        && snapshot.project_id == scope.project_id
        && snapshot.root_id == scope.root_id
        && snapshot.authority_ref == scope.authority_ref
        && snapshot.authority_generation == scope.authority_generation
}

fn scope_to_snapshot(scope: &AuthorizedIntakeScope) -> ScopeSnapshot {
    ScopeSnapshot {
        principal: scope.principal.clone(),
        project_id: scope.project_id.clone(),
        root_id: scope.root_id.clone(),
        authority_ref: scope.authority_ref.clone(),
        authority_generation: scope.authority_generation,
    }
}

fn validate_provenance_bounds(provenance: &SourceProvenance) -> Result<(), WorkOrderIntakeError> {
    if provenance.schema_version != intake::INTAKE_VERSION
        || !valid_identifier(&provenance.source_id)
        || !valid_opaque_reference(&provenance.revision)
        || provenance.captured_at.trim().is_empty()
        || provenance.captured_at.len() > MAX_CAPTURED_AT_BYTES
        || provenance.captured_at.contains('\0')
    {
        return Err(WorkOrderIntakeError::InvalidInput(
            "source provenance fields are invalid",
        ));
    }
    if let RequesterAttribution::Claimed { requester } = &provenance.requester
        && (requester.trim().is_empty()
            || requester.len() > MAX_CLAIM_REQUESTER_BYTES
            || requester.contains('\0'))
    {
        return Err(WorkOrderIntakeError::InvalidInput(
            "claimed requester attribution is outside bounds",
        ));
    }
    if let Some(origin) = &provenance.origin
        && (!valid_identifier(&origin.system)
            || !valid_opaque_reference(&origin.record_id)
            || !valid_opaque_reference(&origin.revision))
    {
        return Err(WorkOrderIntakeError::InvalidInput(
            "source origin reference is invalid",
        ));
    }
    Ok(())
}

fn extract_inline_source(payload: &SourcePayload) -> Result<&str, WorkOrderIntakeError> {
    match payload {
        SourcePayload::InlineUtf8 { text } => {
            if text.trim().is_empty() || text.contains('\0') {
                return Err(WorkOrderIntakeError::InvalidInput(
                    "inline UTF-8 source text must be non-empty and free of NUL bytes",
                ));
            }
            if text.len() > intake::MAX_SOURCE_BYTES {
                return Err(WorkOrderIntakeError::CapacityExceeded);
            }
            Ok(text)
        }
        SourcePayload::ArtifactClaim {
            raw_digest,
            size_bytes,
            artifact_ref,
            pin_receipt_ref,
        } => {
            if !valid_sha256_digest(raw_digest)
                || *size_bytes == 0
                || *size_bytes as usize > intake::MAX_SOURCE_BYTES
                || !valid_opaque_reference(artifact_ref)
                || !valid_opaque_reference(pin_receipt_ref)
            {
                return Err(WorkOrderIntakeError::InvalidInput(
                    "artifact claim metadata is invalid",
                ));
            }
            Err(WorkOrderIntakeError::ArtifactClaimUnavailable)
        }
    }
}

fn validate_capture_request_pre_hash(
    request: &CaptureIntakeRequest,
) -> Result<(&ProjectId, &RootId), WorkOrderIntakeError> {
    let _inline = extract_inline_source(&request.source_payload)?;
    if request.schema_version != intake::INTAKE_VERSION
        || request.revision == 0
        || request.revision > MAX_CAPTURE_REVISIONS
        || request.expected_revision.checked_add(1) != Some(request.revision)
        || (request.expected_revision == 0) != request.parent_receipt_digest.is_none()
        || request
            .parent_receipt_digest
            .as_ref()
            .is_some_and(|d| !valid_sha256_digest(d))
        || !valid_sha256_digest(&request.preview_manifest_digest)
        || !valid_sha256_digest(&request.policy_digest)
        || !valid_identifier(&request.capture_id)
        || !valid_opaque_reference(&request.scope.authority_ref)
        || !valid_opaque_reference(&request.policy_revision)
        || request.scope.authority_generation == 0
    {
        return Err(WorkOrderIntakeError::InvalidInput(
            "capture request envelope or lineage is invalid",
        ));
    }
    let (Some(project_id), Some(root_id)) = (&request.scope.project_id, &request.scope.root_id)
    else {
        return Err(WorkOrderIntakeError::AccessDenied);
    };
    if !valid_identifier(project_id.as_str()) || !valid_identifier(root_id.as_str()) {
        return Err(WorkOrderIntakeError::InvalidInput(
            "capture scope project_id or root_id is invalid",
        ));
    }
    validate_provenance_bounds(&request.provenance)?;
    bounded_json_size(request, MAX_CAPTURE_REQUEST_BYTES)
        .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;
    Ok((project_id, root_id))
}

fn validate_record_request_pre_hash(
    request: &RecordIntakeRequest,
) -> Result<(&ProjectId, &RootId), WorkOrderIntakeError> {
    let _inline = extract_inline_source(&request.work_order.source_payload)?;
    if request.work_order.format == IntakeFormat::PlainText {
        return Err(WorkOrderIntakeError::UnresolvedPlainTextEnrichment);
    }
    if request.work_order.revision == 0 || request.work_order.revision > MAX_WORK_ORDER_REVISIONS {
        return Err(WorkOrderIntakeError::CapacityExceeded);
    }
    let (Some(project_id), Some(root_id)) = (
        &request.work_order.scope.project_id,
        &request.work_order.scope.root_id,
    ) else {
        return Err(WorkOrderIntakeError::AccessDenied);
    };
    if !valid_identifier(project_id.as_str()) || !valid_identifier(root_id.as_str()) {
        return Err(WorkOrderIntakeError::InvalidInput(
            "work-order scope project_id or root_id is invalid",
        ));
    }
    request.validate().map_err(map_intake_error)?;
    bounded_json_size(request, MAX_RECORD_REQUEST_BYTES)
        .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;
    Ok((project_id, root_id))
}

fn capture_request_digest(request: &CaptureIntakeRequest) -> Result<String, WorkOrderIntakeError> {
    bounded_json_size(request, MAX_CAPTURE_REQUEST_BYTES)
        .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;
    content_digest(&("admiralty.wo_service.capture/v1", request))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest capture request"))
}

fn record_request_digest(request: &RecordIntakeRequest) -> Result<String, WorkOrderIntakeError> {
    bounded_json_size(request, MAX_RECORD_REQUEST_BYTES)
        .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;
    content_digest(&("admiralty.wo_service.record/v1", request))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest record request"))
}

fn map_service_to_capture_error(err: WorkOrderIntakeError) -> CaptureError {
    match err {
        WorkOrderIntakeError::AccessDenied => CaptureError::Denied,
        WorkOrderIntakeError::ArtifactClaimUnavailable => CaptureError::UnsupportedSource,
        WorkOrderIntakeError::CapacityExceeded => CaptureError::Capacity,
        WorkOrderIntakeError::Collision | WorkOrderIntakeError::ConcurrentInflightCollision => {
            CaptureError::Collision
        }
        WorkOrderIntakeError::StaleCurrent => CaptureError::StaleCurrent,
        WorkOrderIntakeError::IntegrityFailure => CaptureError::Integrity,
        WorkOrderIntakeError::InvalidInput(_)
        | WorkOrderIntakeError::UnresolvedPlainTextEnrichment
        | WorkOrderIntakeError::SelectionMismatch
        | WorkOrderIntakeError::StorageUnavailable => CaptureError::Invalid,
    }
}

fn map_service_to_record_error(err: WorkOrderIntakeError) -> IntakeStoreError {
    match err {
        WorkOrderIntakeError::AccessDenied => IntakeStoreError::Denied,
        WorkOrderIntakeError::ArtifactClaimUnavailable => IntakeStoreError::UnsupportedSource,
        WorkOrderIntakeError::CapacityExceeded => IntakeStoreError::Capacity,
        WorkOrderIntakeError::Collision | WorkOrderIntakeError::ConcurrentInflightCollision => {
            IntakeStoreError::Collision
        }
        WorkOrderIntakeError::StaleCurrent => IntakeStoreError::StaleCurrent,
        WorkOrderIntakeError::IntegrityFailure => IntakeStoreError::Integrity,
        WorkOrderIntakeError::InvalidInput(_)
        | WorkOrderIntakeError::UnresolvedPlainTextEnrichment
        | WorkOrderIntakeError::SelectionMismatch
        | WorkOrderIntakeError::StorageUnavailable => IntakeStoreError::Selection,
    }
}

fn map_intake_error(err: IntakeError) -> WorkOrderIntakeError {
    match err {
        IntakeError::SourceTooLarge { .. }
        | IntakeError::TextTooLarge { .. }
        | IntakeError::Bound => WorkOrderIntakeError::CapacityExceeded,
        IntakeError::Version
        | IntakeError::Utf8
        | IntakeError::Json
        | IntakeError::DuplicateKey
        | IntakeError::Invalid(_)
        | IntakeError::Span
        | IntakeError::TaskContract
        | IntakeError::Binding => {
            WorkOrderIntakeError::InvalidInput("intake contract validation failed")
        }
    }
}

fn map_capture_store_error(err: CaptureError) -> WorkOrderIntakeError {
    match err {
        CaptureError::Denied => WorkOrderIntakeError::AccessDenied,
        CaptureError::UnsupportedSource => WorkOrderIntakeError::ArtifactClaimUnavailable,
        CaptureError::Collision => WorkOrderIntakeError::Collision,
        CaptureError::StaleCurrent => WorkOrderIntakeError::StaleCurrent,
        CaptureError::Capacity => WorkOrderIntakeError::CapacityExceeded,
        CaptureError::Integrity => WorkOrderIntakeError::IntegrityFailure,
        CaptureError::Invalid => {
            WorkOrderIntakeError::InvalidInput("capture input or lineage is invalid")
        }
        CaptureError::Contract(inner) => map_intake_error(inner),
        CaptureError::Sqlite(_) | CaptureError::Json(_) => WorkOrderIntakeError::StorageUnavailable,
    }
}

fn map_record_store_error(err: IntakeStoreError) -> WorkOrderIntakeError {
    match err {
        IntakeStoreError::Denied => WorkOrderIntakeError::AccessDenied,
        IntakeStoreError::UnsupportedSource => WorkOrderIntakeError::ArtifactClaimUnavailable,
        IntakeStoreError::Selection => WorkOrderIntakeError::SelectionMismatch,
        IntakeStoreError::Collision => WorkOrderIntakeError::Collision,
        IntakeStoreError::StaleCurrent => WorkOrderIntakeError::StaleCurrent,
        IntakeStoreError::Capacity => WorkOrderIntakeError::CapacityExceeded,
        IntakeStoreError::Integrity => WorkOrderIntakeError::IntegrityFailure,
        IntakeStoreError::Contract(inner) => map_intake_error(inner),
        IntakeStoreError::Workplan(workplan_err) => match workplan_err {
            WorkplanStoreError::IdentityCollision
            | WorkplanStoreError::Parser(WorkplanError::TaskIdentityConflict(_))
            | WorkplanStoreError::Parser(WorkplanError::DuplicateTaskId(_)) => {
                WorkOrderIntakeError::Collision
            }
            WorkplanStoreError::StaleCurrent | WorkplanStoreError::RevisionSequence => {
                WorkOrderIntakeError::StaleCurrent
            }
            WorkplanStoreError::TooLarge
            | WorkplanStoreError::Capacity
            | WorkplanStoreError::Parser(WorkplanError::TooLarge)
            | WorkplanStoreError::Parser(WorkplanError::TooManyTasks) => {
                WorkOrderIntakeError::CapacityExceeded
            }
            WorkplanStoreError::Integrity(_) | WorkplanStoreError::Schema => {
                WorkOrderIntakeError::IntegrityFailure
            }
            WorkplanStoreError::Invalid(_)
            | WorkplanStoreError::ScopeMismatch
            | WorkplanStoreError::Parser(_) => {
                WorkOrderIntakeError::InvalidInput("workplan validation failed")
            }
            WorkplanStoreError::Sqlite(_) | WorkplanStoreError::Json(_) => {
                WorkOrderIntakeError::StorageUnavailable
            }
        },
        IntakeStoreError::Sqlite(_) | IntakeStoreError::Json(_) => {
            WorkOrderIntakeError::StorageUnavailable
        }
    }
}

fn map_store_error(err: StoreError) -> WorkOrderIntakeError {
    match err {
        StoreError::IntakeCapture(inner) => map_capture_store_error(inner),
        StoreError::Intake(inner) => map_record_store_error(inner),
        _ => WorkOrderIntakeError::StorageUnavailable,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewIntakeInput {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub format: IntakeFormat,
    pub provenance: SourceProvenance,
    pub source_payload: SourcePayload,
}

/// Scoped, default-deny work-order intake service adapter.
pub struct WorkOrderIntakeService {
    store: Store,
    bridge: Arc<WorkOrderIntakeAdmissionBridge>,
}

impl WorkOrderIntakeService {
    /// Trusted composition only. Installing `bridge` on `StoreAuthorities` at
    /// Store startup ensures every `capture_intake` and `record_intake` turn is
    /// authorized on the Store writer thread.
    pub fn new(store: Store, bridge: Arc<WorkOrderIntakeAdmissionBridge>) -> Self {
        Self { store, bridge }
    }

    pub fn admission_bridge(&self) -> &Arc<WorkOrderIntakeAdmissionBridge> {
        &self.bridge
    }

    /// Pure bounded preview under current independent credential + `PreviewRaw`
    /// action policy. Raw `PlainText` stays one request with explicit
    /// `UnresolvedFact`s. `SourcePayload::ArtifactClaim` fails closed with
    /// `WorkOrderIntakeError::ArtifactClaimUnavailable`.
    pub fn preview_raw(
        &self,
        actor: &AuthenticatedIntakeActor,
        input: &PreviewIntakeInput,
    ) -> Result<IntakePreview, WorkOrderIntakeError> {
        if !valid_identifier(input.project_id.as_str()) || !valid_identifier(input.root_id.as_str())
        {
            return Err(WorkOrderIntakeError::InvalidInput(
                "preview project_id or root_id is invalid",
            ));
        }
        validate_provenance_bounds(&input.provenance)?;
        let raw_text = extract_inline_source(&input.source_payload)?;
        bounded_json_size(
            &(
                &input.project_id,
                &input.root_id,
                input.format,
                &input.provenance,
                &input.source_payload,
            ),
            MAX_CAPTURE_REQUEST_BYTES,
        )
        .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;

        let initial_resolved = self.bridge.registry.resolve_current_grant(
            actor,
            &input.project_id,
            &input.root_id,
            WorkOrderIntakeAction::PreviewRaw,
        )?;
        if raw_text.len() > initial_resolved.policy.max_source_bytes {
            return Err(WorkOrderIntakeError::CapacityExceeded);
        }
        let scope_snapshot = scope_to_snapshot(&initial_resolved.scope);
        let preview_digest = content_digest(&(
            "admiralty.wo_service.preview/v1",
            &input.project_id,
            &input.root_id,
            input.format,
            &input.provenance,
            &input.source_payload,
            raw_text.as_bytes(),
            &scope_snapshot,
            &initial_resolved.policy.revision,
            &initial_resolved.policy_digest,
        ))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest preview input"))?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::PreviewRaw,
            actor,
            &input.project_id,
            &input.root_id,
            Some(&input.provenance.requester),
            None,
            Some(&input.provenance.source_id),
            None,
            &preview_digest,
        )?;
        if !same_resolved_grant(&initial_resolved, &resolved) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        if raw_text.len() > resolved.policy.max_source_bytes {
            return Err(WorkOrderIntakeError::CapacityExceeded);
        }
        intake::preview_intake(
            raw_text.as_bytes(),
            input.format,
            &input.provenance,
            &resolved.scope,
            &resolved.policy,
        )
        .map_err(map_intake_error)
    }

    /// Durable original unenriched capture via `Store::capture_intake`.
    /// Rechecks current credential revocation, `authority_generation`, and
    /// `WorkOrderIntakePolicy` at `AuthorizationStage::WriterTurn` on every call
    /// and historical retry. Emits no task, plan, outbox, or provider effects.
    pub fn capture_raw(
        &self,
        actor: &AuthenticatedIntakeActor,
        request: CaptureIntakeRequest,
    ) -> Result<CaptureResult, WorkOrderIntakeError> {
        let (project_id, root_id) = validate_capture_request_pre_hash(&request)?;
        let project_id = project_id.clone();
        let root_id = root_id.clone();
        let digest = capture_request_digest(&request)?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::CaptureRaw,
            actor,
            &project_id,
            &root_id,
            Some(&request.provenance.requester),
            Some(&request.command_id),
            Some(&request.capture_id),
            Some(request.revision),
            &digest,
        )?;
        if !scope_snapshot_matches(&request.scope, &resolved.scope)
            || request.policy_revision != resolved.policy.revision
            || request.policy_digest != resolved.policy_digest
        {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        let inline_text = extract_inline_source(&request.source_payload)?;
        if inline_text.len() > resolved.policy.max_source_bytes {
            return Err(WorkOrderIntakeError::CapacityExceeded);
        }

        let _inflight_guard = self.bridge.admit_inflight(
            IntakeMutationKind::CaptureRaw,
            request.command_id.clone(),
            digest,
            actor,
            project_id,
            root_id,
            resolved,
        )?;
        self.store.capture_intake(request).map_err(map_store_error)
    }

    /// Durable atomic enriched recording via `Store::record_intake`.
    /// Rejects `SourcePayload::ArtifactClaim` (`ArtifactClaimUnavailable`) and
    /// raw unenriched `PlainText` (`UnresolvedPlainTextEnrichment`) before any
    /// write. Rechecks current credential revocation, `authority_generation`,
    /// and `WorkOrderIntakePolicy` at `AuthorizationStage::WriterTurn` on every
    /// call and historical retry.
    pub fn record_enriched(
        &self,
        actor: &AuthenticatedIntakeActor,
        request: RecordIntakeRequest,
    ) -> Result<IntakeRecordResult, WorkOrderIntakeError> {
        let (project_id, root_id) = validate_record_request_pre_hash(&request)?;
        let project_id = project_id.clone();
        let root_id = root_id.clone();
        let digest = record_request_digest(&request)?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::RecordEnriched,
            actor,
            &project_id,
            &root_id,
            Some(&request.work_order.provenance.requester),
            Some(&request.command_id),
            Some(&request.work_order.id),
            Some(request.work_order.revision),
            &digest,
        )?;
        if !scope_snapshot_matches(&request.work_order.scope, &resolved.scope)
            || request.authority_generation != resolved.scope.authority_generation
            || request.policy_revision != resolved.policy.revision
            || request.policy_digest != resolved.policy_digest
        {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        request
            .validate_with_policy(&resolved.policy)
            .map_err(map_intake_error)?;

        let _inflight_guard = self.bridge.admit_inflight(
            IntakeMutationKind::RecordEnriched,
            request.command_id.clone(),
            digest,
            actor,
            project_id,
            root_id,
            resolved,
        )?;
        self.store.record_intake(request).map_err(map_store_error)
    }

    /// Bounded scoped read of a captured raw intake revision (`Store::captured_intake`).
    pub fn get_captured_intake(
        &self,
        actor: &AuthenticatedIntakeActor,
        project_id: &ProjectId,
        root_id: &RootId,
        capture_id: &str,
        revision: Option<u32>,
    ) -> Result<Option<CapturedIntake>, WorkOrderIntakeError> {
        if !valid_identifier(project_id.as_str())
            || !valid_identifier(root_id.as_str())
            || !valid_identifier(capture_id)
            || revision.is_some_and(|r| r == 0 || r > MAX_CAPTURE_REVISIONS)
        {
            return Err(WorkOrderIntakeError::InvalidInput(
                "captured intake read parameters are invalid",
            ));
        }
        let initial_resolved = self.bridge.registry.resolve_current_grant(
            actor,
            project_id,
            root_id,
            WorkOrderIntakeAction::ReadCaptured,
        )?;
        let scope_snapshot = scope_to_snapshot(&initial_resolved.scope);
        let read_digest = content_digest(&(
            "admiralty.wo_service.read_capture/v1",
            project_id,
            root_id,
            capture_id,
            revision,
            &scope_snapshot,
            &initial_resolved.policy.revision,
            &initial_resolved.policy_digest,
        ))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest capture read"))?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::ReadCaptured,
            actor,
            project_id,
            root_id,
            None,
            None,
            Some(capture_id),
            revision,
            &read_digest,
        )?;
        if !same_resolved_grant(&initial_resolved, &resolved) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        self.store
            .captured_intake(resolved.scope, capture_id.to_owned(), revision)
            .map_err(map_store_error)
    }

    /// Bounded scoped read of a recorded work-order revision (`Store::get_work_order`).
    pub fn get_work_order(
        &self,
        actor: &AuthenticatedIntakeActor,
        project_id: &ProjectId,
        root_id: &RootId,
        work_order_id: &str,
        revision: Option<u32>,
    ) -> Result<Option<RegisteredWorkOrder>, WorkOrderIntakeError> {
        if !valid_identifier(project_id.as_str())
            || !valid_identifier(root_id.as_str())
            || !valid_identifier(work_order_id)
            || revision.is_some_and(|r| r == 0 || r > MAX_WORK_ORDER_REVISIONS)
        {
            return Err(WorkOrderIntakeError::InvalidInput(
                "work-order read parameters are invalid",
            ));
        }
        let initial_resolved = self.bridge.registry.resolve_current_grant(
            actor,
            project_id,
            root_id,
            WorkOrderIntakeAction::ReadWorkOrder,
        )?;
        let scope_snapshot = scope_to_snapshot(&initial_resolved.scope);
        let read_digest = content_digest(&(
            "admiralty.wo_service.read_work_order/v1",
            project_id,
            root_id,
            work_order_id,
            revision,
            &scope_snapshot,
            &initial_resolved.policy.revision,
            &initial_resolved.policy_digest,
        ))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest work-order read"))?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::ReadWorkOrder,
            actor,
            project_id,
            root_id,
            None,
            None,
            Some(work_order_id),
            revision,
            &read_digest,
        )?;
        if !same_resolved_grant(&initial_resolved, &resolved) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        self.store
            .get_work_order(resolved.scope, work_order_id.to_owned(), revision)
            .map_err(map_store_error)
    }

    /// Bounded duplicate candidate query (`Store::find_duplicate_candidates`).
    pub fn find_duplicate_candidates(
        &self,
        actor: &AuthenticatedIntakeActor,
        project_id: &ProjectId,
        root_id: &RootId,
        query: DuplicateQuery,
    ) -> Result<DuplicatePage, WorkOrderIntakeError> {
        if !valid_identifier(project_id.as_str()) || !valid_identifier(root_id.as_str()) {
            return Err(WorkOrderIntakeError::InvalidInput(
                "duplicate query project_id or root_id is invalid",
            ));
        }
        if &query.project_id != project_id {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        bounded_json_size(&query, MAX_DUPLICATE_QUERY_BYTES)
            .map_err(|_| WorkOrderIntakeError::CapacityExceeded)?;
        let initial_resolved = self.bridge.registry.resolve_current_grant(
            actor,
            project_id,
            root_id,
            WorkOrderIntakeAction::QueryDuplicates,
        )?;
        let raw_query_digest = query.identity_digest().map_err(map_intake_error)?;
        let scope_snapshot = scope_to_snapshot(&initial_resolved.scope);
        let query_digest = content_digest(&(
            "admiralty.wo_service.query_duplicates/v1",
            &raw_query_digest,
            &scope_snapshot,
            &initial_resolved.policy.revision,
            &initial_resolved.policy_digest,
        ))
        .map_err(|_| WorkOrderIntakeError::InvalidInput("failed to digest duplicate query"))?;

        let resolved = self.bridge.authorize_at_stage(
            AuthorizationStage::ServicePreQueue,
            WorkOrderIntakeAction::QueryDuplicates,
            actor,
            project_id,
            root_id,
            None,
            query.command_id.as_ref(),
            None,
            None,
            &query_digest,
        )?;
        if !same_resolved_grant(&initial_resolved, &resolved) {
            return Err(WorkOrderIntakeError::AccessDenied);
        }
        self.store
            .find_duplicate_candidates(resolved.scope, query)
            .map_err(map_store_error)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::sync::atomic::AtomicU64;

    struct FakeClock(AtomicU64);

    impl FakeClock {
        fn new(initial: u64) -> Self {
            Self(AtomicU64::new(initial))
        }

        fn advance(&self, delta: u64) {
            self.0.fetch_add(delta, Ordering::SeqCst);
        }
    }

    impl MonotonicClock for FakeClock {
        fn now_nanos(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct AllowAllPolicy;
    impl WorkOrderIntakePolicy for AllowAllPolicy {
        fn authorize_current(&self, _binding: &WorkOrderIntakeBinding<'_>) -> bool {
            true
        }
    }

    #[test]
    fn swept_old_inflight_guard_never_deletes_newer_same_key_ticket() {
        let principal = PrincipalId::new("operator-a").expect("principal");
        let project = ProjectId::new("project-a").expect("project");
        let root = RootId::new("root-a").expect("root");
        let grant = IntakeScopeGrant {
            project_id: project.clone(),
            root_id: root.clone(),
            authority_ref: "grant:intake-a".into(),
            authority_generation: 1,
            policy: IntakePolicy::default(),
            allowed_actions: BTreeSet::from([WorkOrderIntakeAction::CaptureRaw]),
        };
        let registry = Arc::new(
            WorkOrderIntakeAuthRegistry::new(vec![WorkOrderIntakeCredential::configured(
                "cred-1".into(),
                "0123456789abcdef0123456789abcdef".into(),
                principal,
                vec![grant],
            )])
            .expect("registry"),
        );
        let actor = registry
            .authenticate("0123456789abcdef0123456789abcdef")
            .expect("authenticate");
        let resolved_grant = registry
            .resolve_current_grant(&actor, &project, &root, WorkOrderIntakeAction::CaptureRaw)
            .expect("resolve grant");
        let clock = Arc::new(FakeClock::new(100));
        let bridge = WorkOrderIntakeAdmissionBridge::with_clock_and_limits(
            registry,
            Some(Arc::new(AllowAllPolicy)),
            clock.clone(),
            Duration::from_nanos(50),
            2,
        );
        let cmd = CommandId::new("cmd-same-key").expect("command id");

        let guard1 = bridge
            .admit_inflight(
                IntakeMutationKind::CaptureRaw,
                cmd.clone(),
                "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
                &actor,
                project.clone(),
                root.clone(),
                resolved_grant.clone(),
            )
            .expect("first ticket admitted");

        // Concurrent identical active call while guard1 is still within TTL must fail.
        let collision = bridge.admit_inflight(
            IntakeMutationKind::CaptureRaw,
            cmd.clone(),
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".into(),
            &actor,
            project.clone(),
            root.clone(),
            resolved_grant.clone(),
        );
        assert!(matches!(
            collision,
            Err(WorkOrderIntakeError::ConcurrentInflightCollision)
        ));

        // Advance monotonic clock past guard1 TTL and admit a new ticket for the same key.
        clock.advance(60);
        let guard2 = bridge
            .admit_inflight(
                IntakeMutationKind::CaptureRaw,
                cmd.clone(),
                "sha256:2222222222222222222222222222222222222222222222222222222222222222".into(),
                &actor,
                project,
                root,
                resolved_grant,
            )
            .expect("second ticket admitted after TTL sweep");

        // Dropping the expired guard1 must NOT remove guard2's active ticket.
        drop(guard1);
        assert_eq!(bridge.active_inflight_count().expect("count"), 1);

        let taken = bridge
            .take_matching_inflight(
                IntakeMutationKind::CaptureRaw,
                &cmd,
                "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            )
            .expect("guard2 ticket still present");
        assert_eq!(taken.ticket_nonce, 2);
        drop(guard2);
        assert_eq!(bridge.active_inflight_count().expect("count"), 0);
    }
}
