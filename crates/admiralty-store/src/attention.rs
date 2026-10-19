//! Durable, bounded attention-policy replay on Store's sole writer connection.
//!
//! Both observations AND delivery requests are immutable journal commands: the
//! latter consume the pure governor's clock/budget too. Wake proposals and new
//! outbox intents commit in one transaction. Recording a wake is neither an
//! authenticated grant nor transport, provider, or application delivery.
//! Source authority/evidence references are consistency data. Protected Store
//! admission additionally rechecks independently installed current policy and an
//! opaque authenticated actor proof on the sole writer, including retries. Raw
//! Connection compatibility functions are not protected Store admission APIs.
//! No history/configuration/epoch is replaced or evicted to reset a budget.
//! Unkeyed integrity digests do not authenticate a database owner who rewrites all
//! evidence. Scope/authentication, transport reconciliation and safe retirement
//! remain external. The JSON byte cap is not a SQLite/WAL/disk-growth guarantee.
use crate::context_renders::{
    self, ContextNamespace, ContextScope, RenderRef, StoredContextRender,
};
use crate::outbox::{self, EnqueueResult, OutboxIntent, PreparedIntent};
use admiralty_core::{
    PrincipalId, ProjectId, RootId, content_digest,
    coordination::context::{ProtectedTaskFacts, RecipientRole, RoleFacts, decode_role_capsule},
    governor::{
        AttentionGovernor, AttentionUpdate, DeliveryBatch, DeliveryDecision, GovernorConfig,
        GovernorError, Observation, Recipient, Role,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};

pub const MAX_JOURNALS: usize = 64;
pub const MAX_COMMANDS: usize = 4096;
/// Aggregate accepted command + immutable receipt JSON bytes across all journals.
pub const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DEFINITION_BYTES: usize = 8192;
pub const MAX_COMMAND_BYTES: usize = 32768;
pub const MAX_RECEIPT_BYTES: usize = 32768;
pub const MAX_CALLER_PAYLOAD_BYTES: usize = 4096;
type Result<T> = std::result::Result<T, AttentionError>;

#[derive(Debug, thiserror::Error)]
pub enum AttentionError {
    #[error("attention SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("attention JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Governor(#[from] GovernorError),
    #[error(transparent)]
    Outbox(#[from] outbox::OutboxError),
    #[error("invalid or oversized attention request")]
    Invalid,
    #[error("attention identity has conflicting immutable content")]
    Collision,
    #[error("attention prior-state compare-and-swap is stale")]
    StaleCursor,
    #[error("attention journal is not registered in the exact supplied scope")]
    MissingJournal,
    #[error("attention history capacity is full; nothing was evicted or reset")]
    Capacity,
    #[error("stored attention history is inconsistent")]
    Integrity,
    #[error("current attention recording authority is absent or denies this exact request")]
    AccessDenied,
    #[error("attention context/evidence does not match the exact stored capsule")]
    ContextBinding,
    #[error(transparent)]
    Context(#[from] context_renders::ContextRenderError),
}

/// One project/root and an explicitly declared set of independently scoped
/// recipients. Core updates carry no root: their root binding comes ONLY from
/// this caller-owned namespace, never from a source's authority label.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionScope {
    pub journal_id: String,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub epoch: String,
    /// Sorted unique (role, principal) pairs, at most 16.
    pub recipients: Vec<Recipient>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionDefinition {
    pub scope: AttentionScope,
    pub config: GovernorConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionRegistration {
    pub definition_digest: String,
    pub config_digest: String,
    pub replayed: bool,
    pub authority_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionCursor {
    pub revision: String,
    /// Digest of the full deterministic command/result prefix, not an auth MAC.
    pub state_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionOperation {
    Observe {
        update: Box<AttentionUpdate>,
    },
    /// `intent.payload` is bounded caller metadata. On a Wake it is wrapped with
    /// the actual scope, command ID and governor batch. The resulting complete
    /// envelope must still fit the existing 16 KiB outbox limit, without truncation.
    TakeDelivery {
        recipient: Recipient,
        intent: OutboxIntent,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionCommand {
    pub scope: AttentionScope,
    pub config_digest: String,
    pub command_id: String,
    pub expected_prior: Option<AttentionCursor>,
    /// Trusted recorded logical policy tick; None is rejected, never zero-filled.
    pub logical_tick: Option<u64>,
    pub operation: AttentionOperation,
}

/// Claimed immutable references, never authentication, delivery, baseline or ACK.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionContextPin {
    pub scope: ContextScope,
    pub render: RenderRef,
    pub facts_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionRequestBinding {
    pub actor: PrincipalId,
    pub credential_ref: String,
    pub policy_digest: String,
    pub context: AttentionContextPin,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedAttentionCommand {
    pub command: AttentionCommand,
    pub binding: AttentionRequestBinding,
}

/// Transported ONLY on the in-process writer queue. There is no deserializer,
/// credential byte storage, or proof constructor here. The installed authority
/// must downcast its own private authenticated identity and validate CURRENT
/// credentials/policy. An arbitrary `Any` value or matching labels is not proof.
pub type AttentionProof = Arc<dyn Any + Send + Sync>;

pub struct AttentionDefinitionBinding<'a> {
    pub definition: &'a AttentionDefinition,
    pub definition_digest: &'a str,
    pub config_digest: &'a str,
}
pub struct AttentionCommandBinding<'a> {
    pub definition: &'a AttentionDefinition,
    pub definition_digest: &'a str,
    pub request: &'a AuthorizedAttentionCommand,
    pub request_digest: &'a str,
    pub stored_context: &'a StoredContextRender,
    /// None before replay/execution; Some before the transaction commits. This
    /// does NOT assert pending batch refs were compiled into request_context.
    pub outcome: Option<&'a AttentionOutcome>,
}

/// Startup policy only, never installed by deserializing a request. Callbacks
/// must be bounded, side-effect-free, and do no I/O or reentrant Store calls.
/// Definition/status checks are startup configuration checks, NOT caller read
/// authentication. Command checks must validate opaque proof/current actor and
/// exact scope/role/epoch/source/evidence/context/request/policy bindings.
pub trait AttentionAuthority: Send + Sync {
    fn authorize_definition(&self, binding: &AttentionDefinitionBinding<'_>) -> bool;
    fn authorize_command(
        &self,
        binding: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool;
    /// Independent source-context read permission. Recording permission alone
    /// must never disclose retained source capsules. Checked on the writer both
    /// before loading sources and immediately before returning the snapshot.
    /// Called for the original Wake (outcome Some) AND each original Observe
    /// source (outcome None). Proof identifies the CURRENT reader, which need
    /// not be the source's historical credential. Check each actual full source
    /// namespace, not just membership of the wake's project/root.
    fn authorize_delivery_context(
        &self,
        _binding: &AttentionCommandBinding<'_>,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StoredAuthorizedCommand {
    protocol_version: u8,
    request: AuthorizedAttentionCommand,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum StoredCommand {
    Authorized(Box<StoredAuthorizedCommand>),
    Legacy(Box<AttentionCommand>),
}
impl StoredCommand {
    fn command(&self) -> &AttentionCommand {
        match self {
            Self::Authorized(v) => &v.request.command,
            Self::Legacy(v) => v,
        }
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::Authorized(v) if v.protocol_version == 2 => {
                validate_authorized_command(&v.request)?
            }
            Self::Authorized(_) => return Err(AttentionError::Invalid),
            Self::Legacy(v) => validate_command(v)?,
        }
        json(self, MAX_COMMAND_BYTES)?;
        Ok(())
    }
    fn prefix(&self) -> &'static str {
        match self {
            Self::Authorized(_) => "attention-prefix-v2",
            Self::Legacy(_) => "attention-prefix-v1",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttentionOutcome {
    Observation { observation: Observation },
    Delivery { decision: DeliveryDecision },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionEffect {
    pub outbox_id: String,
    pub idempotency_key: String,
    pub payload_digest: String,
    pub intent_digest: String,
    pub not_before: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionReceipt {
    pub cursor: AttentionCursor,
    pub logical_tick: String,
    pub outcome: AttentionOutcome,
    pub effect: Option<AttentionEffect>,
    pub recorded_wake: bool,
    pub delivered: bool,
    pub application_acknowledged: bool,
    pub authority_granted: bool,
}
/// Replay changes this outer flag only. Original receipt bytes, digest, logical
/// tick, batch, cursor and outbox identity remain immutable historical evidence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionResult {
    pub receipt: AttentionReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionStatus {
    pub scope: AttentionScope,
    pub config_digest: String,
    pub cursor: Option<AttentionCursor>,
    pub pending_refs: usize,
    pub topics: usize,
    pub authority_granted: bool,
}

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn json<T: Serialize>(value: &T, max: usize) -> Result<String> {
    admiralty_core::limits::bounded_json_size(value, max).map_err(|_| AttentionError::Invalid)?;
    Ok(serde_json::to_string(value)?)
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    content_digest(value).map_err(|_| AttentionError::Invalid)
}
pub(crate) fn validate_scope(scope: &AttentionScope) -> Result<()> {
    if !id(&scope.journal_id)
        || !id(&scope.epoch)
        || scope.recipients.is_empty()
        || scope.recipients.len() > 16
        || scope.recipients.windows(2).any(|w| w[0] >= w[1])
    {
        return Err(AttentionError::Invalid);
    }
    json(scope, MAX_DEFINITION_BYTES)?;
    Ok(())
}
pub(crate) fn validate_definition(definition: &AttentionDefinition) -> Result<()> {
    validate_scope(&definition.scope)?;
    json(definition, MAX_DEFINITION_BYTES)?;
    AttentionGovernor::new(definition.config.clone())?;
    Ok(())
}
pub(crate) fn validate_command(command: &AttentionCommand) -> Result<()> {
    validate_scope(&command.scope)?;
    if !id(&command.command_id)
        || !digest(&command.config_digest)
        || command.expected_prior.as_ref().is_some_and(|c| {
            !digest(&c.state_digest)
                || c.revision
                    .parse::<u64>()
                    .ok()
                    .is_none_or(|n| n == 0 || n.to_string() != c.revision)
        })
    {
        return Err(AttentionError::Invalid);
    }
    match &command.operation {
        AttentionOperation::Observe { update } => {
            update.validate()?;
            if update.scope.project_id != command.scope.project_id
                || !command.scope.recipients.contains(&update.recipient)
            {
                return Err(AttentionError::Invalid);
            }
        }
        AttentionOperation::TakeDelivery { recipient, intent } => {
            if !command.scope.recipients.contains(recipient) {
                return Err(AttentionError::Invalid);
            }
            // Structural depth/width validation must precede recursive JSON serialization.
            outbox::prepare_intent(intent)?;
            json(&intent.payload, MAX_CALLER_PAYLOAD_BYTES)?;
        }
    }
    json(command, MAX_COMMAND_BYTES)?;
    Ok(())
}

pub(crate) fn validate_authorized_command(request: &AuthorizedAttentionCommand) -> Result<()> {
    validate_command(&request.command)?;
    json(request, MAX_COMMAND_BYTES)?;
    let binding = &request.binding;
    let pin = &binding.context;
    if !id(&binding.credential_ref)
        || !digest(&binding.policy_digest)
        || !digest(&pin.facts_digest)
        || !digest(&pin.render.record_digest)
    {
        return Err(AttentionError::Invalid);
    }
    context_renders::validate_get_request(&pin.scope, &pin.render.key)?;
    let recipient = match &request.command.operation {
        AttentionOperation::Observe { update } => &update.recipient,
        AttentionOperation::TakeDelivery { recipient, .. } => recipient,
    };
    let role = match recipient.role {
        Role::TaskWorker => RecipientRole::Worker,
        Role::Captain => RecipientRole::Commodore,
        Role::SuperCaptain => RecipientRole::Admiral,
    };
    if binding.actor != recipient.principal_id
        || pin.scope.recipient != recipient.principal_id
        || pin.scope.role != role
    {
        return Err(AttentionError::ContextBinding);
    }
    let scope = &request.command.scope;
    let includes = match &pin.scope.namespace {
        ContextNamespace::Project {
            project_id,
            root_id,
        } => project_id == &scope.project_id && root_id == &scope.root_id,
        ContextNamespace::Fleet { project_roots, .. } => project_roots
            .iter()
            .any(|r| r.project_id == scope.project_id && r.root_id == scope.root_id),
    };
    if !includes {
        return Err(AttentionError::ContextBinding);
    }
    if let AttentionOperation::Observe { update } = &request.command.operation
        && update.evidence_ref != pin.render.record_digest
    {
        return Err(AttentionError::ContextBinding);
    }
    Ok(())
}

/// Exact request identity from actual immutable definition + all request data.
/// This is an unkeyed consistency digest, not an authentication token.
pub fn authorized_request_digest(
    definition_digest: &str,
    request: &AuthorizedAttentionCommand,
) -> Result<String> {
    if !digest(definition_digest) {
        return Err(AttentionError::Invalid);
    }
    validate_authorized_command(request)?;
    let value = &("attention-request-v2", definition_digest, request);
    json(value, MAX_COMMAND_BYTES)?;
    hash(value)
}

fn checked_context(
    connection: &Connection,
    request: &AuthorizedAttentionCommand,
) -> Result<StoredContextRender> {
    validate_authorized_command(request)?;
    let pin = &request.binding.context;
    let stored = context_renders::get(connection, &pin.scope, &pin.render.key)?
        .ok_or(AttentionError::ContextBinding)?;
    if stored.capsule.context_epoch != request.command.scope.epoch
        || stored.capsule.facts_digest != pin.facts_digest
        || hash(&stored)? != pin.render.record_digest
    {
        return Err(AttentionError::ContextBinding);
    }
    let facts = decode_role_capsule(
        &stored.capsule.text,
        pin.scope.role,
        &request.command.scope.epoch,
        &pin.facts_digest,
    )
    .map_err(|_| AttentionError::ContextBinding)?;
    if let AttentionOperation::Observe { update } = &request.command.operation {
        let matches = |task: &ProtectedTaskFacts| {
            task.contract.project_id == update.scope.project_id
                && task.contract.root_id == request.command.scope.root_id
                && task.contract.task_id == update.scope.task_id
                && task.contract.revision == update.scope.revision
                && task.contract.authority_ref == update.authority_ref
        };
        let present = match facts {
            RoleFacts::Decision(f) => matches(&f.task),
            RoleFacts::Handoff(f) => matches(&f.task),
            RoleFacts::ProjectExceptions(f) => f.exceptions.iter().any(matches),
            RoleFacts::FleetExceptions(f) => {
                f.projects.iter().any(|p| p.exceptions.iter().any(matches))
            }
        };
        if !present {
            return Err(AttentionError::ContextBinding);
        }
    }
    Ok(stored)
}

fn authorize_definition(
    authority: Option<&dyn AttentionAuthority>,
    definition: &AttentionDefinition,
) -> Result<()> {
    let authority = authority.ok_or(AttentionError::AccessDenied)?;
    let definition_digest = hash(definition)?;
    let config_digest = hash(&definition.config)?;
    if !authority.authorize_definition(&AttentionDefinitionBinding {
        definition,
        definition_digest: &definition_digest,
        config_digest: &config_digest,
    }) {
        return Err(AttentionError::AccessDenied);
    }
    Ok(())
}
fn authorize_request(
    authority: &dyn AttentionAuthority,
    def: &DefinitionRow,
    request: &AuthorizedAttentionCommand,
    context: &StoredContextRender,
    proof: &(dyn Any + Send + Sync),
    outcome: Option<&AttentionOutcome>,
) -> Result<()> {
    authorize_definition(Some(authority), &def.definition)?;
    let request_digest = authorized_request_digest(&def.definition_digest, request)?;
    if !authority.authorize_command(
        &AttentionCommandBinding {
            definition: &def.definition,
            definition_digest: &def.definition_digest,
            request,
            request_digest: &request_digest,
            stored_context: context,
            outcome,
        },
        proof,
    ) {
        return Err(AttentionError::AccessDenied);
    }
    Ok(())
}

fn bounds(connection: &Connection) -> Result<(i64, i64, i64)> {
    let (journals, definitions): (i64, i64) = connection.query_row(
        "SELECT count(*),COALESCE(sum(length(CAST(definition_json AS BLOB))),0) FROM main.attention_journals", [],
        |r| Ok((r.get(0)?,r.get(1)?)))?;
    let (commands, bytes): (i64, i64) = connection.query_row(
        "SELECT count(*),COALESCE(sum(length(CAST(command_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM main.attention_commands", [],
        |r| Ok((r.get(0)?,r.get(1)?)))?;
    if journals > MAX_JOURNALS as i64
        || definitions > (MAX_JOURNALS * MAX_DEFINITION_BYTES) as i64
        || commands > MAX_COMMANDS as i64
        || bytes > MAX_HISTORY_BYTES as i64
    {
        return Err(AttentionError::Capacity);
    }
    Ok((journals, commands, bytes))
}
pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0005_attention_journal.sql"))?;
    tx.commit()?;
    validate_all(connection)
}
struct DefinitionRow {
    definition: AttentionDefinition,
    definition_digest: String,
    head: Option<AttentionCursor>,
}
fn definition(connection: &Connection, journal_id: &str) -> Result<Option<DefinitionRow>> {
    let raw = connection.query_row(
        "SELECT journal_id,definition_json,definition_digest,head_sequence,head_digest FROM main.attention_journals WHERE journal_id=?1", [journal_id], |r| {
            Ok((crate::stored_text(r,0,128)?, crate::stored_text(r,1,MAX_DEFINITION_BYTES)?,
                crate::stored_text(r,2,71)?, r.get::<_,i64>(3)?,
                if matches!(r.get_ref(4)?, rusqlite::types::ValueRef::Null) { None } else { Some(crate::stored_text(r,4,71)?) }))
        }).optional()?;
    raw.map(|(id, bytes, expected, sequence, state_digest)| {
        let definition: AttentionDefinition = serde_json::from_str(&bytes)?;
        validate_definition(&definition).map_err(|_| AttentionError::Integrity)?;
        if definition.scope.journal_id != id
            || id != journal_id
            || hash(&definition)? != expected
            || json(&definition, MAX_DEFINITION_BYTES)? != bytes
            || sequence < 0
            || (sequence == 0) != state_digest.is_none()
            || state_digest.as_ref().is_some_and(|d| !digest(d))
        {
            return Err(AttentionError::Integrity);
        }
        Ok(DefinitionRow {
            definition,
            definition_digest: expected,
            head: state_digest.map(|state_digest| AttentionCursor {
                revision: sequence.to_string(),
                state_digest,
            }),
        })
    })
    .transpose()
}
/// Legacy caller-owned Connection compatibility. Real Store registration MUST
/// use `register_authorized`; possessing this data is never recording authority.
pub fn register(
    connection: &mut Connection,
    requested: AttentionDefinition,
) -> Result<AttentionRegistration> {
    register_inner(connection, requested, None, false)
}
pub(crate) fn register_authorized(
    connection: &mut Connection,
    requested: AttentionDefinition,
    authority: Option<&dyn AttentionAuthority>,
) -> Result<AttentionRegistration> {
    register_inner(connection, requested, authority, true)
}
fn register_inner(
    connection: &mut Connection,
    requested: AttentionDefinition,
    authority: Option<&dyn AttentionAuthority>,
    protected: bool,
) -> Result<AttentionRegistration> {
    validate_definition(&requested)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if protected {
        authorize_definition(authority, &requested)?;
    }
    let (count, _, _) = bounds(&tx)?;
    let existing = definition(&tx, &requested.scope.journal_id)?;
    let replayed = existing.is_some();
    if let Some(original) = existing {
        if original.definition != requested {
            return Err(AttentionError::Collision);
        }
        replay(&tx, &original, None)?;
    } else {
        if count >= MAX_JOURNALS as i64 {
            return Err(AttentionError::Capacity);
        }
        tx.execute("INSERT INTO main.attention_journals(journal_id,definition_json,definition_digest) VALUES(?1,?2,?3)",
            params![requested.scope.journal_id,json(&requested,MAX_DEFINITION_BYTES)?,hash(&requested)?])?;
    }
    if protected {
        authorize_definition(authority, &requested)?;
    }
    tx.commit()?;
    Ok(AttentionRegistration {
        definition_digest: hash(&requested)?,
        config_digest: hash(&requested.config)?,
        replayed,
        authority_granted: false,
    })
}

fn execute(
    governor: &mut AttentionGovernor,
    command: &AttentionCommand,
) -> Result<(AttentionOutcome, Option<PreparedIntent>)> {
    match &command.operation {
        AttentionOperation::Observe { update } => Ok((
            AttentionOutcome::Observation {
                observation: governor.observe((**update).clone(), command.logical_tick)?,
            },
            None,
        )),
        AttentionOperation::TakeDelivery { recipient, intent } => {
            let decision = governor.take_delivery(recipient, command.logical_tick)?;
            let prepared = if let DeliveryDecision::Wake { delivery } = &decision {
                // All source values were bounded before cloning/constructing this
                // envelope. prepare_intent bounds the WHOLE envelope again.
                #[derive(Serialize)]
                struct WakePayload<'a> {
                    schema_version: u8,
                    kind: &'static str,
                    scope: &'a AttentionScope,
                    config_digest: &'a str,
                    command_id: &'a str,
                    delivery: &'a DeliveryBatch,
                    payload: &'a serde_json::Value,
                }
                let envelope = WakePayload {
                    schema_version: 1,
                    kind: "attention_wake",
                    scope: &command.scope,
                    config_digest: &command.config_digest,
                    command_id: &command.command_id,
                    delivery,
                    payload: &intent.payload,
                };
                admiralty_core::limits::bounded_json_size(&envelope, outbox::MAX_PAYLOAD_BYTES)
                    .map_err(|_| AttentionError::Invalid)?;
                let payload = serde_json::to_value(&envelope)?;
                Some(outbox::prepare_intent(&OutboxIntent {
                    idempotency_key: intent.idempotency_key.clone(),
                    payload,
                    not_before: intent.not_before,
                })?)
            } else {
                None
            };
            Ok((AttentionOutcome::Delivery { decision }, prepared))
        }
    }
}
fn effect(id: i64, prepared: &PreparedIntent) -> AttentionEffect {
    AttentionEffect {
        outbox_id: id.to_string(),
        idempotency_key: prepared.idempotency_key.clone(),
        payload_digest: prepared.payload_digest.clone(),
        intent_digest: prepared.intent_digest.clone(),
        not_before: prepared.not_before.to_string(),
    }
}
fn receipt(
    def: &DefinitionRow,
    stored_command: &StoredCommand,
    sequence: i64,
    outcome: AttentionOutcome,
    effect: Option<AttentionEffect>,
) -> Result<AttentionReceipt> {
    let command = stored_command.command();
    let state_digest = hash(&(
        stored_command.prefix(),
        &def.definition_digest,
        sequence.to_string(),
        &command.expected_prior,
        hash(stored_command)?,
        &outcome,
        &effect,
    ))?;
    Ok(AttentionReceipt {
        cursor: AttentionCursor {
            revision: sequence.to_string(),
            state_digest,
        },
        logical_tick: command
            .logical_tick
            .ok_or(GovernorError::UnknownClock)?
            .to_string(),
        recorded_wake: effect.is_some(),
        outcome,
        effect,
        delivered: false,
        application_acknowledged: false,
        authority_granted: false,
    })
}
fn validate_effect(connection: &Connection, id: i64, prepared: &PreparedIntent) -> Result<()> {
    let raw = connection.query_row("SELECT idempotency_key,payload,payload_digest,not_before FROM main.outbox_intents WHERE id=?1",[id],|r| {
        Ok((crate::stored_text(r,0,256)?,crate::stored_text(r,1,outbox::MAX_PAYLOAD_BYTES)?,
            crate::stored_text(r,2,71)?,r.get::<_,i64>(3)?))
    }).optional()?.ok_or(AttentionError::Integrity)?;
    if id <= 0
        || raw.0 != prepared.idempotency_key
        || raw.1 != prepared.payload
        || raw.2 != prepared.payload_digest
        || raw.3 != prepared.not_before
    {
        return Err(AttentionError::Integrity);
    }
    // Mutable pending/claimed/unknown/completed status is deliberately neither
    // rewritten nor interpreted as delivered. No re-enqueue occurs on replay.
    Ok(())
}
struct Replayed {
    governor: AttentionGovernor,
    cursor: Option<AttentionCursor>,
    matching: Option<(StoredCommand, AttentionResult)>,
    count: i64,
}
fn replay(connection: &Connection, def: &DefinitionRow, find_id: Option<&str>) -> Result<Replayed> {
    bounds(connection)?;
    let mut governor = AttentionGovernor::new(def.definition.config.clone())?;
    let mut statement = connection.prepare("SELECT command_id,sequence,command_json,command_digest,receipt_json,receipt_digest,outbox_id FROM main.attention_commands WHERE journal_id=?1 ORDER BY sequence LIMIT 4097")?;
    let mut rows = statement.query([&def.definition.scope.journal_id])?;
    let mut cursor = None;
    let mut matching = None;
    let mut count = 0i64;
    let mut bytes = 0usize;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_COMMANDS as i64 {
            return Err(AttentionError::Capacity);
        }
        let id = crate::stored_text(row, 0, 128)?;
        let sequence: i64 = row.get(1)?;
        let command_json = crate::stored_text(row, 2, MAX_COMMAND_BYTES)?;
        let command_digest = crate::stored_text(row, 3, 71)?;
        let receipt_json = crate::stored_text(row, 4, MAX_RECEIPT_BYTES)?;
        let receipt_digest = crate::stored_text(row, 5, 71)?;
        let outbox_id: Option<i64> = row.get(6)?;
        bytes = bytes
            .checked_add(command_json.len() + receipt_json.len())
            .ok_or(AttentionError::Capacity)?;
        if bytes > MAX_HISTORY_BYTES {
            return Err(AttentionError::Capacity);
        }
        let stored_command: StoredCommand = serde_json::from_str(&command_json)?;
        stored_command
            .validate()
            .map_err(|_| AttentionError::Integrity)?;
        if let StoredCommand::Authorized(authorized) = &stored_command {
            // Historical consistency only: current revocation must not erase
            // valid history or prevent reopen/rebuild/backup of older records.
            checked_context(connection, &authorized.request)
                .map_err(|_| AttentionError::Integrity)?;
        }
        let command = stored_command.command();
        let stored: AttentionReceipt = serde_json::from_str(&receipt_json)?;
        validate_command(command).map_err(|_| AttentionError::Integrity)?;
        if command.scope != def.definition.scope
            || command.config_digest != hash(&def.definition.config)?
            || command.command_id != id
            || command.expected_prior != cursor
            || sequence != count
            || hash(&stored_command)? != command_digest
            || json(&stored_command, MAX_COMMAND_BYTES)? != command_json
            || hash(&stored)? != receipt_digest
            || json(&stored, MAX_RECEIPT_BYTES)? != receipt_json
        {
            return Err(AttentionError::Integrity);
        }
        let (outcome, prepared) =
            execute(&mut governor, command).map_err(|_| AttentionError::Integrity)?;
        let association = match (outbox_id, prepared) {
            (Some(id), Some(prepared)) => {
                validate_effect(connection, id, &prepared)?;
                Some(effect(id, &prepared))
            }
            (None, None) => None,
            _ => return Err(AttentionError::Integrity),
        };
        let expected = receipt(def, &stored_command, sequence, outcome, association)?;
        if stored != expected {
            return Err(AttentionError::Integrity);
        }
        cursor = Some(expected.cursor.clone());
        if find_id == Some(id.as_str()) {
            matching = Some((
                stored_command,
                AttentionResult {
                    receipt: stored,
                    receipt_digest,
                    replayed: true,
                },
            ));
        }
    }
    if cursor != def.head {
        return Err(AttentionError::Integrity);
    }
    Ok(Replayed {
        governor,
        cursor,
        matching,
        count,
    })
}
/// Legacy caller-owned Connection compatibility, preserving original bytes and
/// receipts. This function is NOT a protected Store admission route.
pub fn apply(connection: &mut Connection, command: AttentionCommand) -> Result<AttentionResult> {
    apply_inner(connection, StoredCommand::Legacy(Box::new(command)), None)
}
pub(crate) fn apply_authorized(
    connection: &mut Connection,
    request: AuthorizedAttentionCommand,
    proof: &(dyn Any + Send + Sync),
    authority: Option<&dyn AttentionAuthority>,
) -> Result<AttentionResult> {
    let authority = authority.ok_or(AttentionError::AccessDenied)?;
    apply_inner(
        connection,
        StoredCommand::Authorized(Box::new(StoredAuthorizedCommand {
            protocol_version: 2,
            request,
        })),
        Some((authority, proof)),
    )
}
fn apply_inner(
    connection: &mut Connection,
    stored_command: StoredCommand,
    protected: Option<(&dyn AttentionAuthority, &(dyn Any + Send + Sync))>,
) -> Result<AttentionResult> {
    stored_command.validate()?;
    let command = stored_command.command();
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let def = definition(&tx, &command.scope.journal_id)?.ok_or(AttentionError::MissingJournal)?;
    if def.definition.scope != command.scope
        || hash(&def.definition.config)? != command.config_digest
    {
        return Err(AttentionError::Collision);
    }
    let checked = match (protected, &stored_command) {
        (Some((authority, proof)), StoredCommand::Authorized(authorized)) => {
            let context = checked_context(&tx, &authorized.request)?;
            authorize_request(authority, &def, &authorized.request, &context, proof, None)?;
            Some(context)
        }
        (None, StoredCommand::Legacy(_)) => None,
        _ => return Err(AttentionError::AccessDenied),
    };
    let mut state = replay(&tx, &def, Some(&command.command_id))?;
    if let Some((original, result)) = state.matching {
        if original != stored_command {
            return Err(AttentionError::Collision);
        }
        if let (Some((authority, proof)), StoredCommand::Authorized(authorized), Some(context)) =
            (protected, &stored_command, checked.as_ref())
        {
            authorize_request(
                authority,
                &def,
                &authorized.request,
                context,
                proof,
                Some(&result.receipt.outcome),
            )?;
        }
        tx.commit()?;
        return Ok(result);
    }
    if command.expected_prior != state.cursor {
        return Err(AttentionError::StaleCursor);
    }
    let (_, count, bytes) = bounds(&tx)?;
    if count >= MAX_COMMANDS as i64 {
        return Err(AttentionError::Capacity);
    }
    let (outcome, prepared) = execute(&mut state.governor, command)?;
    let association = if let Some(prepared) = prepared {
        match outbox::enqueue_prepared_in_transaction(&tx, &prepared)? {
            EnqueueResult::Inserted { id } => Some(effect(id, &prepared)),
            // Even an existing exact *foreign* row cannot be associated with a
            // new decision/budget consumption. RAII rolls back the whole tx.
            EnqueueResult::ExistingExact { .. } | EnqueueResult::Collision => {
                return Err(AttentionError::Collision);
            }
        }
    } else {
        None
    };
    let receipt = receipt(&def, &stored_command, state.count + 1, outcome, association)?;
    let command_json = json(&stored_command, MAX_COMMAND_BYTES)?;
    let receipt_json = json(&receipt, MAX_RECEIPT_BYTES)?;
    if bytes
        .checked_add((command_json.len() + receipt_json.len()) as i64)
        .is_none_or(|n| n > MAX_HISTORY_BYTES as i64)
    {
        return Err(AttentionError::Capacity);
    }
    let receipt_digest = hash(&receipt)?;
    let outbox_id = receipt
        .effect
        .as_ref()
        .map(|e| e.outbox_id.parse::<i64>())
        .transpose()
        .map_err(|_| AttentionError::Integrity)?;
    tx.execute("INSERT INTO main.attention_commands(journal_id,command_id,sequence,command_json,command_digest,receipt_json,receipt_digest,outbox_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![command.scope.journal_id,command.command_id,state.count+1,command_json,hash(&stored_command)?,receipt_json,receipt_digest,outbox_id])?;
    tx.execute(
        "UPDATE main.attention_journals SET head_sequence=?2,head_digest=?3 WHERE journal_id=?1",
        params![
            command.scope.journal_id,
            state.count + 1,
            receipt.cursor.state_digest
        ],
    )?;
    // This check is on the actual writer after SQL effects are staged and before
    // commit. Denial rolls back outbox, receipt and cursor together. The external
    // authority still owns its own revocation synchronization; no callback here
    // can promise atomicity with an unrelated policy store after it returns.
    if let (Some((authority, proof)), StoredCommand::Authorized(authorized), Some(context)) =
        (protected, &stored_command, checked.as_ref())
    {
        authorize_request(
            authority,
            &def,
            &authorized.request,
            context,
            proof,
            Some(&receipt.outcome),
        )?;
    }
    tx.commit()?;
    Ok(AttentionResult {
        receipt,
        receipt_digest,
        replayed: false,
    })
}
pub(crate) fn status_authorized(
    connection: &Connection,
    scope: &AttentionScope,
    authority: Option<&dyn AttentionAuthority>,
) -> Result<Option<AttentionStatus>> {
    validate_scope(scope)?;
    let Some(def) = definition(connection, &scope.journal_id)? else {
        // Do not leak existence through a missing startup policy.
        authority.ok_or(AttentionError::AccessDenied)?;
        return Ok(None);
    };
    if &def.definition.scope != scope {
        return Err(AttentionError::MissingJournal);
    }
    authorize_definition(authority, &def.definition)?;
    let result = status(connection, scope)?;
    authorize_definition(authority, &def.definition)?;
    Ok(result)
}
pub(crate) fn status(
    connection: &Connection,
    scope: &AttentionScope,
) -> Result<Option<AttentionStatus>> {
    validate_scope(scope)?;
    let Some(def) = definition(connection, &scope.journal_id)? else {
        return Ok(None);
    };
    if &def.definition.scope != scope {
        return Err(AttentionError::MissingJournal);
    }
    let state = replay(connection, &def, None)?;
    Ok(Some(AttentionStatus {
        scope: scope.clone(),
        config_digest: hash(&def.definition.config)?,
        cursor: state.cursor,
        pending_refs: state.governor.pending_refs(),
        topics: state.governor.topic_count(),
        authority_granted: false,
    }))
}
pub(crate) fn validate_all(connection: &Connection) -> Result<()> {
    bounds(connection)?;
    let orphan: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM main.attention_commands c LEFT JOIN main.attention_journals j USING(journal_id) WHERE j.journal_id IS NULL)",[],|r|r.get(0))?;
    if orphan {
        return Err(AttentionError::Integrity);
    }
    let mut statement = connection
        .prepare("SELECT journal_id FROM main.attention_journals ORDER BY journal_id LIMIT 65")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_JOURNALS {
            return Err(AttentionError::Capacity);
        }
        let id = crate::stored_text(row, 0, 128)?;
        let def = definition(connection, &id)?.ok_or(AttentionError::Integrity)?;
        replay(connection, &def, None)?;
    }
    Ok(())
}

pub const MAX_DELIVERY_CONTEXT_SOURCES: usize = 16;
pub const MAX_DELIVERY_CONTEXT_BYTES: usize = 1024 * 1024;

/// Trusted in-process scoped read request, never an authentication grant.
/// The original durable wake and its original opaque actor must still be
/// authorized by independently installed current recording AND read policies.
#[derive(Clone, Debug, Serialize)]
pub struct LoadDeliveryContext {
    pub scope: AttentionScope,
    pub command_id: String,
    pub receipt_digest: String,
}
pub(crate) fn validate_load_delivery_context(request: &LoadDeliveryContext) -> Result<()> {
    validate_scope(&request.scope)?;
    if !id(&request.command_id) || !digest(&request.receipt_digest) {
        return Err(AttentionError::Invalid);
    }
    json(request, MAX_COMMAND_BYTES)?;
    Ok(())
}

/// Actual immutable source selected from the replay-validated original journal.
/// Private fields/no decoder prevent request JSON from manufacturing this read
/// capability. These are fidelity/custody facts, never current execution grants.
#[derive(Debug, Serialize)]
pub struct DeliveryContextSource {
    reference: admiralty_core::governor::DeliveryRef,
    observe_request: AuthorizedAttentionCommand,
    render: StoredContextRender,
    current_contract: admiralty_core::TaskContract,
}
impl DeliveryContextSource {
    pub fn reference(&self) -> &admiralty_core::governor::DeliveryRef {
        &self.reference
    }
    pub fn observe_request(&self) -> &AuthorizedAttentionCommand {
        &self.observe_request
    }
    pub fn render(&self) -> &StoredContextRender {
        &self.render
    }
    pub fn current_contract(&self) -> &admiralty_core::TaskContract {
        &self.current_contract
    }
}

/// One Store writer/read-transaction snapshot. It includes complete original
/// source renders rather than interpreting a caller's evidence label as custody.
#[derive(Debug, Serialize)]
pub struct AuthorizedDeliveryContext {
    wake_request: AuthorizedAttentionCommand,
    result: AttentionResult,
    request_render: StoredContextRender,
    baseline: Option<context_renders::ContextBaseline>,
    sources: Vec<DeliveryContextSource>,
}
impl AuthorizedDeliveryContext {
    pub fn wake_request(&self) -> &AuthorizedAttentionCommand {
        &self.wake_request
    }
    pub fn result(&self) -> &AttentionResult {
        &self.result
    }
    pub fn request_render(&self) -> &StoredContextRender {
        &self.request_render
    }
    pub fn baseline(&self) -> Option<&context_renders::ContextBaseline> {
        self.baseline.as_ref()
    }
    pub fn sources(&self) -> &[DeliveryContextSource] {
        &self.sources
    }
}

fn authorize_context_read(
    authority: &dyn AttentionAuthority,
    def: &DefinitionRow,
    request: &AuthorizedAttentionCommand,
    context: &StoredContextRender,
    proof: &(dyn Any + Send + Sync),
    outcome: &AttentionOutcome,
) -> Result<()> {
    authorize_request(authority, def, request, context, proof, Some(outcome))?;
    let request_digest = authorized_request_digest(&def.definition_digest, request)?;
    if !authority.authorize_delivery_context(
        &AttentionCommandBinding {
            definition: &def.definition,
            definition_digest: &def.definition_digest,
            request,
            request_digest: &request_digest,
            stored_context: context,
            outcome: Some(outcome),
        },
        proof,
    ) {
        return Err(AttentionError::AccessDenied);
    }
    Ok(())
}

fn authorize_source_read(
    authority: &dyn AttentionAuthority,
    def: &DefinitionRow,
    request: &AuthorizedAttentionCommand,
    context: &StoredContextRender,
    proof: &(dyn Any + Send + Sync),
) -> Result<()> {
    authorize_definition(Some(authority), &def.definition)?;
    let request_digest = authorized_request_digest(&def.definition_digest, request)?;
    if !authority.authorize_delivery_context(
        &AttentionCommandBinding {
            definition: &def.definition,
            definition_digest: &def.definition_digest,
            request,
            request_digest: &request_digest,
            stored_context: context,
            outcome: None,
        },
        proof,
    ) {
        return Err(AttentionError::AccessDenied);
    }
    Ok(())
}

fn update_tag(
    update: &admiralty_core::governor::UpdateKind,
) -> admiralty_core::governor::UpdateTag {
    use admiralty_core::governor::{UpdateKind, UpdateTag};
    match update {
        UpdateKind::Progress { .. } => UpdateTag::Progress,
        UpdateKind::Blocked { .. } => UpdateTag::Blocked,
        UpdateKind::Question { .. } => UpdateTag::Question,
        UpdateKind::Candidate { .. } => UpdateTag::Candidate,
        UpdateKind::Decision { .. } => UpdateTag::Decision,
        UpdateKind::Health { .. } => UpdateTag::Health,
    }
}

fn current_source_contract(
    projection: &admiralty_core::Projection,
    request: &AuthorizedAttentionCommand,
    render: &StoredContextRender,
) -> Result<admiralty_core::TaskContract> {
    let AttentionOperation::Observe { update } = &request.command.operation else {
        return Err(AttentionError::Integrity);
    };
    let current = &projection
        .tasks
        .get(&update.scope.task_id)
        .ok_or(AttentionError::ContextBinding)?
        .contract;
    if current.project_id != request.command.scope.project_id
        || current.root_id != request.command.scope.root_id
        || current.revision != update.scope.revision
        || current.authority_ref != update.authority_ref
    {
        return Err(AttentionError::ContextBinding);
    }
    let facts = decode_role_capsule(
        &render.capsule.text,
        render.scope.role,
        &request.command.scope.epoch,
        &request.binding.context.facts_digest,
    )
    .map_err(|_| AttentionError::ContextBinding)?;
    let matches = |task: &ProtectedTaskFacts| &task.contract == current;
    let present = match facts {
        RoleFacts::Decision(f) => matches(&f.task),
        RoleFacts::Handoff(f) => matches(&f.task),
        RoleFacts::ProjectExceptions(f) => f.exceptions.iter().any(matches),
        RoleFacts::FleetExceptions(f) => {
            f.projects.iter().any(|p| p.exceptions.iter().any(matches))
        }
    };
    if !present {
        return Err(AttentionError::ContextBinding);
    }
    admiralty_core::limits::bounded_json_size(current, MAX_COMMAND_BYTES)
        .map_err(|_| AttentionError::Invalid)?;
    Ok(current.clone())
}

/// Bounded exact read on the existing Store writer. No new journal, outbox,
/// baseline, delivery or authority is created. The caller must retain the
/// independently expected receipt digest. Legacy unbound observations/wakes
/// cannot produce this capability. Current contracts are looked up individually
/// from the writer-owned projection, never cloned from a fleet-wide snapshot.
pub(crate) fn load_authorized_delivery_context(
    connection: &mut Connection,
    projection: &admiralty_core::Projection,
    requested: LoadDeliveryContext,
    authority: Option<&dyn AttentionAuthority>,
    proof: &(dyn Any + Send + Sync),
) -> Result<AuthorizedDeliveryContext> {
    validate_load_delivery_context(&requested)?;
    let authority = authority.ok_or(AttentionError::AccessDenied)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let def =
        definition(&tx, &requested.scope.journal_id)?.ok_or(AttentionError::MissingJournal)?;
    if def.definition.scope != requested.scope {
        return Err(AttentionError::MissingJournal);
    }
    let replayed = replay(&tx, &def, Some(&requested.command_id))?;
    let (stored, result) = replayed.matching.ok_or(AttentionError::MissingJournal)?;
    let StoredCommand::Authorized(wake) = stored else {
        return Err(AttentionError::AccessDenied);
    };
    let request = wake.request;
    if !matches!(
        request.command.operation,
        AttentionOperation::TakeDelivery { .. }
    ) || result.receipt_digest != requested.receipt_digest
    {
        return Err(AttentionError::ContextBinding);
    }
    let AttentionOutcome::Delivery {
        decision: DeliveryDecision::Wake { delivery },
    } = &result.receipt.outcome
    else {
        return Err(AttentionError::ContextBinding);
    };
    if delivery.refs.is_empty() || delivery.refs.len() > MAX_DELIVERY_CONTEXT_SOURCES {
        return Err(AttentionError::Capacity);
    }
    let context = checked_context(&tx, &request)?;
    authorize_context_read(
        authority,
        &def,
        &request,
        &context,
        proof,
        &result.receipt.outcome,
    )?;
    let baseline = context_renders::baseline(&tx, &request.binding.context.scope)?;
    if baseline
        .as_ref()
        .is_some_and(|b| b.context_epoch != requested.scope.epoch)
    {
        return Err(AttentionError::ContextBinding);
    }
    let mut selected: Vec<Option<DeliveryContextSource>> =
        (0..delivery.refs.len()).map(|_| None).collect();
    let mut selected_bytes = 0usize;
    let mut statement = tx.prepare("SELECT command_json FROM main.attention_commands WHERE journal_id=?1 ORDER BY sequence LIMIT 4097")?;
    let mut rows = statement.query([&requested.scope.journal_id])?;
    let mut scanned = 0;
    while let Some(row) = rows.next()? {
        scanned += 1;
        if scanned > MAX_COMMANDS {
            return Err(AttentionError::Capacity);
        }
        let bytes = crate::stored_text(row, 0, MAX_COMMAND_BYTES)?;
        let command: StoredCommand = serde_json::from_str(&bytes)?;
        let AttentionOperation::Observe { update } = &command.command().operation else {
            continue;
        };
        for (index, reference) in delivery.refs.iter().enumerate() {
            if selected[index].is_some() || update.source != reference.source {
                continue;
            }
            let StoredCommand::Authorized(original) = &command else {
                // The first source observation owns ingestion identity. A later
                // authorized duplicate cannot retroactively grant it custody.
                return Err(AttentionError::AccessDenied);
            };
            if update.recipient != delivery.recipient
                || update.scope != reference.scope
                || update.authority_ref != reference.authority_ref
                || update.evidence_ref != reference.evidence_ref
                || update_tag(&update.update) != reference.kind
                || hash(update)? != reference.update_digest
                || original.request.command.scope != requested.scope
            {
                return Err(AttentionError::Integrity);
            }
            let render = checked_context(&tx, &original.request)?;
            authorize_source_read(authority, &def, &original.request, &render, proof)?;
            let current_contract = current_source_contract(projection, &original.request, &render)?;
            let source = DeliveryContextSource {
                reference: reference.clone(),
                observe_request: original.request.clone(),
                render,
                current_contract,
            };
            selected_bytes = selected_bytes
                .checked_add(
                    admiralty_core::limits::bounded_json_size(&source, MAX_DELIVERY_CONTEXT_BYTES)
                        .map_err(|_| AttentionError::Capacity)?,
                )
                .ok_or(AttentionError::Capacity)?;
            if selected_bytes > MAX_DELIVERY_CONTEXT_BYTES {
                return Err(AttentionError::Capacity);
            }
            selected[index] = Some(source);
        }
    }
    drop(rows);
    drop(statement);
    let sources = selected
        .into_iter()
        .map(|source| source.ok_or(AttentionError::Integrity))
        .collect::<Result<Vec<_>>>()?;
    let output = AuthorizedDeliveryContext {
        wake_request: request,
        result,
        request_render: context,
        baseline,
        sources,
    };
    json(&output, MAX_DELIVERY_CONTEXT_BYTES)?;
    for source in &output.sources {
        authorize_source_read(
            authority,
            &def,
            &source.observe_request,
            &source.render,
            proof,
        )?;
    }
    authorize_context_read(
        authority,
        &def,
        &output.wake_request,
        &output.request_render,
        proof,
        &output.result.receipt.outcome,
    )?;
    tx.commit()?;
    Ok(output)
}
