//! Durable storage for the pure question lifecycle reducer.
//!
//! All writes use the caller-owned SQLite writer connection and one IMMEDIATE
//! transaction. Replay trusts only the bounded event journal, never a stored
//! state blob or the host clock.

use crate::stored_text;
use admiralty_core::{
    MessageId, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract, TaskId, TaskView,
    coordination::questions::{
        EscalationProposal, MAX_QUESTION_EVENT_BYTES, MAX_QUESTION_EVENTS, QuestionError,
        QuestionEvent, QuestionScope, QuestionState, QuestionStatus, reduce_question,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_HISTORY: usize = MAX_QUESTION_EVENTS;
const MAX_QUESTIONS_PER_ROOT: usize = 1024;
const MAX_COMMANDS_PER_QUESTION: usize = MAX_HISTORY * 2;
const MAX_SCOPE_BYTES: usize = 4096;
const MAX_PROJECTION_BYTES: usize = 64 * 1024;
const GENESIS: &str = "genesis";
pub const MAX_QUESTION_COMMAND_BYTES: usize = 32 * 1024;
const MAX_PROVENANCE_BYTES: usize = 8 * 1024;
const MAX_CONTRACT_BYTES: usize = 64 * 1024;
const MAX_TOTAL_QUESTIONS: i64 = 4096;
const MAX_TOTAL_COMMANDS: i64 = 8192;
const MAX_TOTAL_TEXT_BYTES: i64 = 32 * 1024 * 1024;
const MAX_EVIDENCE_REFS: usize = 16;

/// Claims are recorded inputs, never authentication. Only the independently
/// installed writer authority can permit the exact command and current task.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionProvenance {
    pub actor: PrincipalId,
    pub source: SourceIdentity,
    pub expected_contract_revision: u32,
    pub expected_contract_digest: String,
    pub policy_digest: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionCommand {
    pub scope: JournalScope,
    pub command_id: MessageId,
    pub event: QuestionEvent,
    pub binding: QuestionProvenance,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionRead {
    pub scope: JournalScope,
    pub actor: PrincipalId,
}

/// `current_task` is supplied by the Store's sole-writer projection, never by
/// the request. Callbacks must be bounded, side-effect-free and perform no I/O
/// or re-entrant Store calls. Returned permission records information only;
/// answers and escalation proposals grant no decision or delivery authority.
pub struct QuestionCommandBinding<'a> {
    pub command: &'a QuestionCommand,
    pub command_digest: &'a str,
    pub current_task: &'a TaskView,
}

pub trait QuestionAuthority: Send + Sync {
    fn authorize_command(&self, binding: &QuestionCommandBinding<'_>) -> bool;
    fn authorize_read(&self, request: &QuestionRead, current_task: &TaskView) -> bool;
}

fn sha256_ref(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn actor(event: &QuestionEvent) -> &PrincipalId {
    match event {
        QuestionEvent::Open { asker, .. } => asker,
        QuestionEvent::Route { actor, .. }
        | QuestionEvent::Answer { actor, .. }
        | QuestionEvent::Supersede { actor, .. }
        | QuestionEvent::Cancel { actor, .. }
        | QuestionEvent::EvaluateDeadline { actor, .. } => actor,
    }
}

/// Streaming bounds must be checked before a Store request is cloned/queued.
/// A digest is a stable request identity, not a credential or authority grant.
pub fn validate_command(command: &QuestionCommand) -> Result<String> {
    admiralty_core::limits::bounded_json_size(command, MAX_QUESTION_COMMAND_BYTES)
        .map_err(|_| QuestionStoreError::EventTooLarge)?;
    validate_scope(&command.scope)?;
    validate_event_scope(&command.scope, &command.event)?;
    admiralty_core::limits::bounded_json_size(&command.event, MAX_QUESTION_EVENT_BYTES)
        .map_err(|_| QuestionStoreError::EventTooLarge)?;
    let binding = &command.binding;
    admiralty_core::limits::bounded_json_size(binding, MAX_PROVENANCE_BYTES)
        .map_err(|_| QuestionStoreError::EventTooLarge)?;
    if actor(&command.event) != &binding.actor
        || binding.expected_contract_revision == 0
        || !sha256_ref(&binding.expected_contract_digest)
        || !sha256_ref(&binding.policy_digest)
        || binding.evidence_refs.is_empty()
        || binding.evidence_refs.len() > MAX_EVIDENCE_REFS
        || binding.evidence_refs.iter().any(|r| {
            r.is_empty() || r.len() > 512 || r.chars().any(|c| c.is_control() || c.is_whitespace())
        })
        || binding
            .evidence_refs
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != binding.evidence_refs.len()
        || binding.source.validate().is_err()
        || [&binding.source.producer_id, &binding.source.incarnation]
            .iter()
            .any(|s| {
                s.is_empty()
                    || s.len() > 128
                    || s.chars().any(|c| c.is_control() || c.is_whitespace())
            })
    {
        return Err(QuestionStoreError::Invalid(
            "invalid exact question provenance".into(),
        ));
    }
    admiralty_core::content_digest(command).map_err(Into::into)
}

pub fn validate_read_request(request: &QuestionRead) -> Result<()> {
    admiralty_core::limits::bounded_json_size(request, MAX_SCOPE_BYTES + 256)
        .map_err(|_| QuestionStoreError::ProjectionTooLarge)?;
    validate_scope(&request.scope)
}

pub type Result<T> = std::result::Result<T, QuestionStoreError>;

#[derive(Debug, thiserror::Error)]
pub enum QuestionStoreError {
    #[error("question SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("question JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("question reducer rejected event: {0}")]
    Reducer(#[from] QuestionError),
    #[error("invalid durable question input: {0}")]
    Invalid(String),
    #[error("question journal history is full; no event was evicted")]
    HistoryFull,
    #[error("bounded question or receipt capacity is full; no record was evicted")]
    CapacityFull,
    #[error("question or command identity was reused with different scope/content")]
    IdentityCollision,
    #[error("question scope does not match the durable root/project/task/authority")]
    ScopeMismatch,
    #[error("question journal integrity check failed: {0}")]
    Integrity(String),
    #[error("question event exceeds its serialized size bound")]
    EventTooLarge,
    #[error("question scope or public projection exceeds its serialized size bound")]
    ProjectionTooLarge,
    #[error("question receipt counter overflow")]
    Overflow,
    #[error("question action or read is not currently authorized")]
    AccessDenied,
    #[error("question belongs to a historical task contract; current action/read is stale")]
    StaleContract,
}

/// Root is bound independently because the pure reducer scope has no root ID.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JournalScope {
    pub root_id: RootId,
    pub question: QuestionScope,
    pub authority_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionReceipt {
    pub command_id: MessageId,
    /// Canonical decimal event position, safe for JavaScript consumers.
    pub event_seq: String,
    /// Canonical decimal reducer revision, safe for JavaScript consumers.
    pub revision: String,
    pub duplicate: bool,
    pub escalation_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableQuestion {
    pub scope: JournalScope,
    pub state: QuestionState,
    pub event_count: String,
    pub head_digest: String,
    pub escalation_ref: Option<String>,
}

#[derive(Serialize)]
struct PublicProjection<'a> {
    scope: &'a JournalScope,
    asker: &'a PrincipalId,
    recipient: &'a PrincipalId,
    requirement: &'a str,
    question: &'a str,
    options: &'a [String],
    deadline_utc: Option<String>,
    revision: String,
    status: PublicStatus<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PublicStatus<'a> {
    Open,
    Routed,
    Answered { answer: &'a str },
    Superseded { by: &'a MessageId },
    Expired { observed_utc: String },
    Cancelled,
}

#[derive(Serialize)]
struct EscalationView<'a> {
    root_id: &'a RootId,
    project_id: &'a ProjectId,
    task_id: &'a TaskId,
    question_id: &'a MessageId,
    authority_ref: &'a str,
    requested_by: &'a PrincipalId,
    current_recipient: &'a PrincipalId,
    expired_at_utc: String,
}

#[derive(Serialize)]
struct CommandContent<'a> {
    scope: &'a JournalScope,
    event: &'a QuestionEvent,
}

fn bounded_json<T: Serialize>(value: &T, maximum: usize) -> Result<Vec<u8>> {
    admiralty_core::limits::bounded_json_size(value, maximum)
        .map_err(|_| QuestionStoreError::EventTooLarge)?;
    Ok(serde_json::to_vec(value)?)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn sql_length(value: usize) -> Result<i64> {
    i64::try_from(value).map_err(|_| QuestionStoreError::Overflow)
}

fn event_id(event: &QuestionEvent) -> &MessageId {
    match event {
        QuestionEvent::Open { event_id, .. }
        | QuestionEvent::Route { event_id, .. }
        | QuestionEvent::Answer { event_id, .. }
        | QuestionEvent::Supersede { event_id, .. }
        | QuestionEvent::Cancel { event_id, .. }
        | QuestionEvent::EvaluateDeadline { event_id, .. } => event_id,
    }
}

fn event_scope(event: &QuestionEvent) -> &QuestionScope {
    match event {
        QuestionEvent::Open { scope, .. }
        | QuestionEvent::Route { scope, .. }
        | QuestionEvent::Answer { scope, .. }
        | QuestionEvent::Supersede { scope, .. }
        | QuestionEvent::Cancel { scope, .. }
        | QuestionEvent::EvaluateDeadline { scope, .. } => scope,
    }
}

fn event_authority(event: &QuestionEvent) -> &str {
    match event {
        QuestionEvent::Open { authority_ref, .. }
        | QuestionEvent::Route { authority_ref, .. }
        | QuestionEvent::Answer { authority_ref, .. }
        | QuestionEvent::Supersede { authority_ref, .. }
        | QuestionEvent::Cancel { authority_ref, .. }
        | QuestionEvent::EvaluateDeadline { authority_ref, .. } => authority_ref,
    }
}

fn validate_scope(scope: &JournalScope) -> Result<()> {
    let bytes =
        bounded_json(scope, MAX_SCOPE_BYTES).map_err(|_| QuestionStoreError::ProjectionTooLarge)?;
    if bytes.is_empty()
        || scope.authority_ref.is_empty()
        || scope.authority_ref.len() > 512
        || scope
            .authority_ref
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(QuestionStoreError::Invalid(
            "root and bounded opaque authority are required".into(),
        ));
    }
    Ok(())
}

fn validate_event_scope(scope: &JournalScope, event: &QuestionEvent) -> Result<()> {
    if &scope.question != event_scope(event) || scope.authority_ref != event_authority(event) {
        return Err(QuestionStoreError::ScopeMismatch);
    }
    Ok(())
}

fn scope_digest(scope: &JournalScope) -> Result<String> {
    let bytes =
        bounded_json(scope, MAX_SCOPE_BYTES).map_err(|_| QuestionStoreError::ProjectionTooLarge)?;
    Ok(digest(&bytes))
}

fn command_digest(scope: &JournalScope, event: &QuestionEvent) -> Result<String> {
    let maximum = MAX_QUESTION_EVENT_BYTES
        .checked_add(MAX_SCOPE_BYTES)
        .ok_or(QuestionStoreError::Overflow)?;
    let bytes = bounded_json(&CommandContent { scope, event }, maximum)
        .map_err(|_| QuestionStoreError::EventTooLarge)?;
    Ok(digest(&bytes))
}

fn project_status(status: &QuestionStatus) -> PublicStatus<'_> {
    match status {
        QuestionStatus::Open => PublicStatus::Open,
        QuestionStatus::Routed => PublicStatus::Routed,
        QuestionStatus::Answered { answer } => PublicStatus::Answered { answer },
        QuestionStatus::Superseded { by } => PublicStatus::Superseded { by },
        QuestionStatus::Expired { observed_utc } => PublicStatus::Expired {
            observed_utc: observed_utc.to_string(),
        },
        QuestionStatus::Cancelled => PublicStatus::Cancelled,
    }
}

fn projection_bytes(scope: &JournalScope, state: &QuestionState) -> Result<Vec<u8>> {
    let projection = PublicProjection {
        scope,
        asker: &state.asker,
        recipient: &state.recipient,
        requirement: &state.requirement,
        question: &state.question,
        options: &state.options,
        deadline_utc: state.deadline_utc.map(|value| value.to_string()),
        revision: state.revision.to_string(),
        status: project_status(&state.status),
    };
    bounded_json(&projection, MAX_PROJECTION_BYTES)
        .map_err(|_| QuestionStoreError::ProjectionTooLarge)
}

fn escalation_view<'a>(
    scope: &'a JournalScope,
    proposal: &'a EscalationProposal,
) -> EscalationView<'a> {
    EscalationView {
        root_id: &scope.root_id,
        project_id: &proposal.project_id,
        task_id: &proposal.task_id,
        question_id: &proposal.question_id,
        authority_ref: &proposal.authority_ref,
        requested_by: &proposal.requested_by,
        current_recipient: &proposal.current_recipient,
        expired_at_utc: proposal.expired_at_utc.to_string(),
    }
}

fn chain_digest(previous: &str, sequence: u64, event_digest: &str, scope_digest: &str) -> String {
    digest(format!("{previous}\n{sequence}\n{event_digest}\n{scope_digest}").as_bytes())
}

/// Create the journal tables atomically on the caller-owned connection.
pub fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS durable_questions (
             question_id TEXT PRIMARY KEY CHECK(length(CAST(question_id AS BLOB)) BETWEEN 1 AND 128),
             root_id TEXT NOT NULL CHECK(length(CAST(root_id AS BLOB)) BETWEEN 1 AND 128),
             project_id TEXT NOT NULL CHECK(length(CAST(project_id AS BLOB)) BETWEEN 1 AND 128),
             task_id TEXT NOT NULL CHECK(length(CAST(task_id AS BLOB)) BETWEEN 1 AND 128),
             authority_ref TEXT NOT NULL CHECK(length(CAST(authority_ref AS BLOB)) BETWEEN 1 AND 512),
             event_count INTEGER NOT NULL CHECK(event_count BETWEEN 0 AND 33),
             head_digest TEXT NOT NULL CHECK(length(CAST(head_digest AS BLOB)) BETWEEN 1 AND 71),
             revision TEXT NOT NULL CHECK(length(CAST(revision AS BLOB)) BETWEEN 1 AND 20),
             projection_json TEXT NOT NULL CHECK(length(CAST(projection_json AS BLOB))<=65536),
             projection_digest TEXT NOT NULL CHECK(length(CAST(projection_digest AS BLOB))=71)
         );
         CREATE TABLE IF NOT EXISTS durable_question_events (
             question_id TEXT NOT NULL REFERENCES durable_questions(question_id),
             sequence INTEGER NOT NULL CHECK(sequence BETWEEN 1 AND 33),
             command_id TEXT NOT NULL CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
             event_id TEXT NOT NULL CHECK(length(CAST(event_id AS BLOB)) BETWEEN 1 AND 128),
             event_json TEXT NOT NULL CHECK(length(CAST(event_json AS BLOB))<=16384),
             event_digest TEXT NOT NULL CHECK(length(CAST(event_digest AS BLOB))=71),
             scope_digest TEXT NOT NULL CHECK(length(CAST(scope_digest AS BLOB))=71),
             previous_digest TEXT NOT NULL CHECK(length(CAST(previous_digest AS BLOB)) BETWEEN 1 AND 71),
             chain_digest TEXT NOT NULL CHECK(length(CAST(chain_digest AS BLOB))=71),
             revision TEXT NOT NULL CHECK(length(CAST(revision AS BLOB)) BETWEEN 1 AND 20),
             PRIMARY KEY(question_id, sequence),
             UNIQUE(question_id, event_id)
         );
         CREATE TABLE IF NOT EXISTS durable_question_commands (
             command_id TEXT PRIMARY KEY CHECK(length(CAST(command_id AS BLOB)) BETWEEN 1 AND 128),
             question_id TEXT NOT NULL CHECK(length(CAST(question_id AS BLOB)) BETWEEN 1 AND 128),
             command_digest TEXT NOT NULL CHECK(length(CAST(command_digest AS BLOB))=71),
             receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB))<=65536),
             receipt_digest TEXT NOT NULL CHECK(length(CAST(receipt_digest AS BLOB))=71)
         );
         CREATE TABLE IF NOT EXISTS durable_question_escalations (
             question_id TEXT PRIMARY KEY REFERENCES durable_questions(question_id),
             escalation_ref TEXT NOT NULL UNIQUE CHECK(length(CAST(escalation_ref AS BLOB))=71),
             proposal_json TEXT NOT NULL CHECK(length(CAST(proposal_json AS BLOB))<=65536),
             proposal_digest TEXT NOT NULL CHECK(length(CAST(proposal_digest AS BLOB))=71)
         );
         CREATE TABLE IF NOT EXISTS durable_question_authorizations (
             command_id TEXT PRIMARY KEY REFERENCES durable_question_commands(command_id),
             producer_id TEXT NOT NULL CHECK(length(CAST(producer_id AS BLOB)) BETWEEN 1 AND 128),
             incarnation TEXT NOT NULL CHECK(length(CAST(incarnation AS BLOB)) BETWEEN 1 AND 128),
             source_sequence TEXT NOT NULL CHECK(length(CAST(source_sequence AS BLOB)) BETWEEN 1 AND 20),
             command_json TEXT NOT NULL CHECK(length(CAST(command_json AS BLOB))<=32768),
             command_digest TEXT NOT NULL CHECK(length(CAST(command_digest AS BLOB))=71),
             original_contract_json TEXT NOT NULL CHECK(length(CAST(original_contract_json AS BLOB))<=65536),
             original_contract_digest TEXT NOT NULL CHECK(length(CAST(original_contract_digest AS BLOB))=71),
             UNIQUE(producer_id,incarnation,source_sequence)
         );",
    )?;
    tx.commit()?;
    Ok(())
}

fn read_stored_scope(connection: &Connection, question_id: &str) -> Result<Option<JournalScope>> {
    let row = connection
        .query_row(
            "SELECT substr(CAST(root_id AS BLOB),1,129),length(CAST(root_id AS BLOB)),substr(CAST(project_id AS BLOB),1,129),length(CAST(project_id AS BLOB)),substr(CAST(task_id AS BLOB),1,129),length(CAST(task_id AS BLOB)),substr(CAST(authority_ref AS BLOB),1,513),length(CAST(authority_ref AS BLOB)) FROM durable_questions WHERE question_id=?1",
            [question_id],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?, row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?, row.get::<_, i64>(5)?,
                    row.get::<_, Vec<u8>>(6)?, row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(root, root_len, project, project_len, task, task_len, authority, authority_len)| {
            let bounded_text = |bytes: Vec<u8>, actual_len: i64, max: usize, label: &str| {
                if usize::try_from(actual_len).map_err(|_| QuestionStoreError::Overflow)? > max {
                    return Err(QuestionStoreError::Integrity(format!(
                        "stored {label} exceeds bound"
                    )));
                }
                String::from_utf8(bytes)
                    .map_err(|error| QuestionStoreError::Integrity(error.to_string()))
            };
            let root = bounded_text(root, root_len, 128, "root ID")?;
            let project = bounded_text(project, project_len, 128, "project ID")?;
            let task = bounded_text(task, task_len, 128, "task ID")?;
            let authority = bounded_text(authority, authority_len, 512, "authority reference")?;
            Ok(JournalScope {
                root_id: RootId::new(root)
                    .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?,
                question: QuestionScope {
                    question_id: MessageId::new(question_id)
                        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?,
                    project_id: ProjectId::new(project)
                        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?,
                    task_id: TaskId::new(task)
                        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?,
                },
                authority_ref: authority,
            })
        },
    )
    .transpose()
}

fn read_escalation(
    connection: &Connection,
    question_id: &str,
) -> Result<Option<(String, String, String)>> {
    let row = connection
        .query_row(
            "SELECT escalation_ref,substr(CAST(proposal_json AS BLOB),1,?2),length(CAST(proposal_json AS BLOB)),proposal_digest FROM durable_question_escalations WHERE question_id=?1",
            params![question_id, sql_length(MAX_PROJECTION_BYTES + 1)?],
            |row| Ok((stored_text(row,0,71)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,i64>(2)?,stored_text(row,3,71)?)),
        )
        .optional()?;
    row.map(|(reference, bytes, length, digest)| {
        if usize::try_from(length).map_err(|_| QuestionStoreError::Overflow)? > MAX_PROJECTION_BYTES
        {
            return Err(QuestionStoreError::Integrity(
                "stored escalation proposal exceeds bound".into(),
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
        Ok((reference, text, digest))
    })
    .transpose()
}

fn reconstruct_inner(
    connection: &Connection,
    requested_scope: &JournalScope,
    current_task_check: bool,
) -> Result<Option<DurableQuestion>> {
    validate_scope(requested_scope)?;
    if current_task_check {
        validate_store_task(connection, requested_scope)?;
    }
    let Some(stored_scope) =
        read_stored_scope(connection, requested_scope.question.question_id.as_str())?
    else {
        return Ok(None);
    };
    if &stored_scope != requested_scope {
        return Err(QuestionStoreError::ScopeMismatch);
    }
    let metadata: (i64, String, String, Vec<u8>, i64, String) = connection.query_row(
        "SELECT event_count,head_digest,revision,substr(CAST(projection_json AS BLOB),1,?2),length(CAST(projection_json AS BLOB)),projection_digest FROM durable_questions WHERE question_id=?1",
        params![
            requested_scope.question.question_id.as_str(),
            sql_length(MAX_PROJECTION_BYTES + 1)?
        ],
        |row| Ok((row.get(0)?,stored_text(row,1,71)?,stored_text(row,2,20)?,row.get(3)?,row.get(4)?,stored_text(row,5,71)?)),
    )?;
    let count = usize::try_from(metadata.0)
        .map_err(|_| QuestionStoreError::Integrity("negative event count".into()))?;
    if count == 0 || count > MAX_HISTORY {
        return Err(QuestionStoreError::Integrity(
            "event history count outside bound".into(),
        ));
    }
    if usize::try_from(metadata.4).map_err(|_| QuestionStoreError::Overflow)? > MAX_PROJECTION_BYTES
    {
        return Err(QuestionStoreError::Integrity(
            "stored public projection exceeds bound".into(),
        ));
    }
    let expected_scope_digest = scope_digest(&stored_scope)?;
    let mut statement = connection.prepare(
        "SELECT sequence,event_id,substr(CAST(event_json AS BLOB),1,?2),length(CAST(event_json AS BLOB)),event_digest,scope_digest,previous_digest,chain_digest,revision FROM durable_question_events WHERE question_id=?1 ORDER BY sequence LIMIT 34",
    )?;
    let mut rows = statement.query(params![
        requested_scope.question.question_id.as_str(),
        sql_length(MAX_QUESTION_EVENT_BYTES + 1)?
    ])?;
    let mut current: Option<QuestionState> = None;
    let mut previous = GENESIS.to_owned();
    let mut expected_escalation: Option<(String, String)> = None;
    let mut sequence = 0u64;
    while let Some(row) = rows.next()? {
        sequence = sequence
            .checked_add(1)
            .ok_or(QuestionStoreError::Overflow)?;
        if usize::try_from(sequence).map_err(|_| QuestionStoreError::Overflow)? > MAX_HISTORY {
            return Err(QuestionStoreError::Integrity(
                "event history exceeds bound".into(),
            ));
        }
        let stored_sequence: i64 = row.get(0)?;
        let stored_event_id = stored_text(row, 1, 128)?;
        let event_bytes: Vec<u8> = row.get(2)?;
        let event_length: i64 = row.get(3)?;
        let stored_event_digest = stored_text(row, 4, 71)?;
        let stored_scope_digest = stored_text(row, 5, 71)?;
        let stored_previous = stored_text(row, 6, 71)?;
        let stored_chain = stored_text(row, 7, 71)?;
        let stored_revision = stored_text(row, 8, 20)?;
        let event_length =
            usize::try_from(event_length).map_err(|_| QuestionStoreError::Overflow)?;
        if stored_sequence != i64::try_from(sequence).map_err(|_| QuestionStoreError::Overflow)?
            || stored_previous != previous
            || stored_scope_digest != expected_scope_digest
            || event_length > MAX_QUESTION_EVENT_BYTES
        {
            return Err(QuestionStoreError::Integrity(
                "event sequence, scope, predecessor, or size mismatch".into(),
            ));
        }
        let event_json = String::from_utf8(event_bytes)
            .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
        let event: QuestionEvent = serde_json::from_str(&event_json)?;
        let canonical = bounded_json(&event, MAX_QUESTION_EVENT_BYTES)?;
        if canonical != event_json.as_bytes()
            || stored_event_id != event_id(&event).as_str()
            || event_digest(&canonical) != stored_event_digest
        {
            return Err(QuestionStoreError::Integrity(
                "event bytes or digest mismatch".into(),
            ));
        }
        validate_event_scope(&stored_scope, &event)?;
        let chain = chain_digest(
            &previous,
            sequence,
            &stored_event_digest,
            &stored_scope_digest,
        );
        if chain != stored_chain {
            return Err(QuestionStoreError::Integrity(
                "event chain digest mismatch".into(),
            ));
        }
        let reduction = reduce_question(&current, &event)?;
        if reduction.replayed {
            return Err(QuestionStoreError::Integrity(
                "duplicate event in canonical log".into(),
            ));
        }
        if let Some(proposal) = reduction.escalation {
            let view = escalation_view(&stored_scope, &proposal);
            let bytes = bounded_json(&view, MAX_PROJECTION_BYTES)?;
            expected_escalation = Some((
                digest(&bytes),
                String::from_utf8(bytes)
                    .map_err(|e| QuestionStoreError::Integrity(e.to_string()))?,
            ));
        }
        let state = reduction.state;
        if stored_revision != state.revision.to_string() {
            return Err(QuestionStoreError::Integrity(
                "event revision mismatch".into(),
            ));
        }
        current = Some(state);
        previous = chain;
    }
    let state = current
        .ok_or_else(|| QuestionStoreError::Integrity("question has no opening event".into()))?;
    if sequence != count as u64
        || metadata.1 != previous
        || metadata.2 != state.revision.to_string()
    {
        return Err(QuestionStoreError::Integrity(
            "journal head or revision mismatch".into(),
        ));
    }
    let public = projection_bytes(&stored_scope, &state)?;
    if public.len() != metadata.3.len() || public != metadata.3 || digest(&public) != metadata.5 {
        return Err(QuestionStoreError::Integrity(
            "public projection differs from canonical replay".into(),
        ));
    }
    let stored_escalation =
        read_escalation(connection, requested_scope.question.question_id.as_str())?;
    let escalation_ref = match (expected_escalation, stored_escalation) {
        (None, None) => None,
        (Some((expected_digest, expected_json)), Some((reference, json, stored_digest)))
            if json == expected_json
                && stored_digest == expected_digest
                && reference == expected_digest =>
        {
            Some(reference)
        }
        _ => {
            return Err(QuestionStoreError::Integrity(
                "escalation proposal reference mismatch".into(),
            ));
        }
    };
    verify_command_receipts(connection, &stored_scope, escalation_ref.as_deref())?;
    Ok(Some(DurableQuestion {
        scope: stored_scope,
        state,
        event_count: sequence.to_string(),
        head_digest: previous,
        escalation_ref,
    }))
}

// When sharing a Store database, the authoritative immutable task contract is
// verified from its event log. A standalone journal has no domain task table.
fn validate_store_task(connection: &Connection, scope: &JournalScope) -> Result<()> {
    let has_store: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='domain_events')",
        [],
        |row| row.get(0),
    )?;
    if has_store {
        let projection = super::replay(connection)
            .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
        let task = projection
            .tasks
            .get(&scope.question.task_id)
            .ok_or(QuestionStoreError::ScopeMismatch)?;
        if task.contract.project_id != scope.question.project_id
            || task.contract.root_id != scope.root_id
            || task.contract.authority_ref != scope.authority_ref
        {
            return Err(QuestionStoreError::ScopeMismatch);
        }
    }
    Ok(())
}

fn verify_command_receipts(
    connection: &Connection,
    scope: &JournalScope,
    escalation: Option<&str>,
) -> Result<()> {
    let fail = || {
        QuestionStoreError::Integrity("command receipt is not bound to its canonical event".into())
    };
    let mut statement=connection.prepare("SELECT command_id,command_digest,receipt_json,receipt_digest FROM durable_question_commands WHERE question_id=?1 ORDER BY command_id LIMIT 67")?;
    let mut rows = statement.query([scope.question.question_id.as_str()])?;
    let mut commands = std::collections::BTreeSet::new();
    while let Some(row) = rows.next()? {
        if commands.len() >= MAX_COMMANDS_PER_QUESTION {
            return Err(QuestionStoreError::CapacityFull);
        }
        let command = stored_text(row, 0, 128)?;
        let stored_digest = stored_text(row, 1, 71)?;
        let json = stored_text(row, 2, MAX_PROJECTION_BYTES)?;
        let receipt_digest = stored_text(row, 3, 71)?;
        let receipt: QuestionReceipt = serde_json::from_str(&json)?;
        let sequence = receipt
            .event_seq
            .parse::<i64>()
            .ok()
            .filter(|value| {
                *value > 0 && *value <= MAX_HISTORY as i64 && value.to_string() == receipt.event_seq
            })
            .ok_or_else(fail)?;
        let (event_json,original_command,revision):(String,String,String)=connection.query_row("SELECT event_json,command_id,revision FROM durable_question_events WHERE question_id=?1 AND sequence=?2",params![scope.question.question_id.as_str(),sequence],|row|Ok((stored_text(row,0,MAX_QUESTION_EVENT_BYTES)?,stored_text(row,1,128)?,stored_text(row,2,20)?)))?;
        let event: QuestionEvent = serde_json::from_str(&event_json)?;
        let expected_escalation = if matches!(event, QuestionEvent::EvaluateDeadline { .. }) {
            escalation
        } else {
            None
        };
        if receipt.command_id.as_str() != command
            || receipt.revision != revision
            || receipt.duplicate != (original_command != command)
            || receipt.escalation_ref.as_deref() != expected_escalation
            || digest(json.as_bytes()) != receipt_digest
            || bounded_json(&receipt, MAX_PROJECTION_BYTES)? != json.as_bytes()
            || command_digest(scope, &event)? != stored_digest
        {
            return Err(fail());
        }
        commands.insert(command);
    }
    let mut statement=connection.prepare("SELECT command_id FROM durable_question_events WHERE question_id=?1 ORDER BY sequence LIMIT 34")?;
    let mut rows = statement.query([scope.question.question_id.as_str()])?;
    while let Some(row) = rows.next()? {
        if !commands.contains(&stored_text(row, 0, 128)?) {
            return Err(fail());
        }
    }
    Ok(())
}

fn event_digest(bytes: &[u8]) -> String {
    digest(bytes)
}

/// Rebuild and verify the reducer state and public projection from canonical events.
pub fn reconstruct(
    connection: &Connection,
    scope: &JournalScope,
) -> Result<Option<DurableQuestion>> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
    let result = reconstruct_inner(&tx, scope, true)?;
    tx.commit()?;
    Ok(result)
}

fn receipt_json(receipt: &QuestionReceipt) -> Result<(String, String)> {
    let bytes = bounded_json(receipt, MAX_PROJECTION_BYTES)?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
    Ok((text, digest(&bytes)))
}

fn cached_receipt(
    connection: &Connection,
    command_id: &str,
    command_digest: &str,
) -> Result<Option<QuestionReceipt>> {
    let row = connection.query_row(
        "SELECT command_digest,substr(CAST(receipt_json AS BLOB),1,?2),length(CAST(receipt_json AS BLOB)),receipt_digest FROM durable_question_commands WHERE command_id=?1",
        params![command_id, sql_length(MAX_PROJECTION_BYTES + 1)?],
        |row| Ok((stored_text(row,0,71)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,i64>(2)?,stored_text(row,3,71)?)),
    ).optional()?;
    let Some((stored_command_digest, receipt_bytes, receipt_len, stored_receipt_digest)) = row
    else {
        return Ok(None);
    };
    if stored_command_digest != command_digest {
        return Err(QuestionStoreError::IdentityCollision);
    }
    if usize::try_from(receipt_len).map_err(|_| QuestionStoreError::Overflow)?
        > MAX_PROJECTION_BYTES
    {
        return Err(QuestionStoreError::Integrity(
            "stored command receipt exceeds bound".into(),
        ));
    }
    let json = String::from_utf8(receipt_bytes)
        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
    if digest(json.as_bytes()) != stored_receipt_digest {
        return Err(QuestionStoreError::Integrity(
            "command receipt digest mismatch".into(),
        ));
    }
    let receipt: QuestionReceipt = serde_json::from_str(&json)?;
    if receipt.command_id.as_str() != command_id
        || bounded_json(&receipt, MAX_PROJECTION_BYTES)? != json.as_bytes()
    {
        return Err(QuestionStoreError::Integrity(
            "command receipt is not canonical".into(),
        ));
    }
    Ok(Some(receipt))
}

/// Append one event using CAS semantics and persist its public projection and
/// any resulting escalation proposal in the same transaction.
pub fn append(
    connection: &mut Connection,
    scope: &JournalScope,
    command_id: &MessageId,
    event: &QuestionEvent,
) -> Result<QuestionReceipt> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let receipt = append_inner(&tx, scope, command_id, event, true)?;
    validate_global_bounds(&tx)?;
    tx.commit()?;
    Ok(receipt)
}

fn append_inner(
    tx: &Transaction<'_>,
    scope: &JournalScope,
    command_id: &MessageId,
    event: &QuestionEvent,
    current_task_check: bool,
) -> Result<QuestionReceipt> {
    validate_scope(scope)?;
    validate_event_scope(scope, event)?;
    let event_bytes = bounded_json(event, MAX_QUESTION_EVENT_BYTES)?;
    let event_text = String::from_utf8(event_bytes.clone())
        .map_err(|error| QuestionStoreError::Invalid(error.to_string()))?;
    let event_digest = event_digest(&event_bytes);
    let command_digest = command_digest(scope, event)?;
    let event_key = event_id(event).as_str();
    let scope_hash = scope_digest(scope)?;
    let cached = cached_receipt(tx, command_id.as_str(), &command_digest)?;
    let current = reconstruct_inner(tx, scope, current_task_check)?;
    if let Some(cached) = cached {
        if current.is_none() {
            return Err(QuestionStoreError::Integrity(
                "cached receipt has no canonical question".into(),
            ));
        }
        return Ok(cached);
    }

    let existing_scope = read_stored_scope(tx, scope.question.question_id.as_str())?;
    if existing_scope
        .as_ref()
        .is_some_and(|existing| existing != scope)
    {
        return Err(QuestionStoreError::ScopeMismatch);
    }
    let command_count: i64 = tx.query_row(
        "SELECT count(*) FROM durable_question_commands WHERE question_id=?1",
        [scope.question.question_id.as_str()],
        |row| row.get(0),
    )?;
    if usize::try_from(command_count)
        .map_err(|_| QuestionStoreError::Integrity("negative command count".into()))?
        >= MAX_COMMANDS_PER_QUESTION
    {
        return Err(QuestionStoreError::CapacityFull);
    }
    if existing_scope.is_none() && matches!(event, QuestionEvent::Open { .. }) {
        let question_count: i64 = tx.query_row(
            "SELECT count(*) FROM durable_questions WHERE root_id=?1",
            [scope.root_id.as_str()],
            |row| row.get(0),
        )?;
        if usize::try_from(question_count)
            .map_err(|_| QuestionStoreError::Integrity("negative root question count".into()))?
            >= MAX_QUESTIONS_PER_ROOT
        {
            return Err(QuestionStoreError::CapacityFull);
        }
    }
    let existing_event: Option<(String, i64, String)> = tx.query_row(
        "SELECT event_digest,sequence,revision FROM durable_question_events WHERE question_id=?1 AND event_id=?2",
        params![scope.question.question_id.as_str(), event_key],
        |row| Ok((stored_text(row,0,71)?,row.get(1)?,stored_text(row,2,20)?)),
    ).optional()?;
    if existing_event
        .as_ref()
        .is_some_and(|(stored_digest, _, _)| stored_digest != &event_digest)
    {
        return Err(QuestionStoreError::IdentityCollision);
    }

    let current_state = current.as_ref().map(|value| value.state.clone());
    let reduction = reduce_question(&current_state, event)?;
    let new_revision = reduction.state.revision;
    if reduction.replayed {
        let (stored_digest, sequence, revision) = existing_event.ok_or_else(|| {
            QuestionStoreError::Integrity("reducer reported replay without a journal event".into())
        })?;
        if stored_digest != event_digest {
            return Err(QuestionStoreError::IdentityCollision);
        }
        let receipt = QuestionReceipt {
            command_id: command_id.clone(),
            event_seq: u64::try_from(sequence)
                .map_err(|_| QuestionStoreError::Overflow)?
                .to_string(),
            revision,
            duplicate: true,
            escalation_ref: if matches!(event, QuestionEvent::EvaluateDeadline { .. }) {
                current
                    .as_ref()
                    .and_then(|value| value.escalation_ref.clone())
            } else {
                None
            },
        };
        insert_receipt_for_question(tx, scope, &command_digest, &receipt)?;
        return Ok(receipt);
    }
    if existing_event.is_some() {
        return Err(QuestionStoreError::IdentityCollision);
    }
    let previous_count = match current.as_ref() {
        Some(value) => value
            .event_count
            .parse::<u64>()
            .ok()
            .filter(|count| count.to_string() == value.event_count)
            .ok_or_else(|| {
                QuestionStoreError::Integrity("event count is not canonical decimal".into())
            })?,
        None => 0,
    };
    if previous_count >= MAX_HISTORY as u64 {
        return Err(QuestionStoreError::HistoryFull);
    }
    let sequence = previous_count
        .checked_add(1)
        .ok_or(QuestionStoreError::Overflow)?;
    let sequence_i64 = i64::try_from(sequence).map_err(|_| QuestionStoreError::Overflow)?;
    let previous = current
        .as_ref()
        .map_or(GENESIS, |value| value.head_digest.as_str());
    let chain = chain_digest(previous, sequence, &event_digest, &scope_hash);
    let public = projection_bytes(scope, &reduction.state)?;
    let public_text = String::from_utf8(public.clone())
        .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
    let public_digest = digest(&public);

    if current.is_none() {
        tx.execute(
            "INSERT INTO durable_questions(question_id,root_id,project_id,task_id,authority_ref,event_count,head_digest,revision,projection_json,projection_digest) VALUES(?1,?2,?3,?4,?5,0,?6,'0','{}',?7)",
            params![scope.question.question_id.as_str(),scope.root_id.as_str(),scope.question.project_id.as_str(),scope.question.task_id.as_str(),scope.authority_ref,GENESIS,digest(b"{}")],
        )?;
    }
    tx.execute(
        "INSERT INTO durable_question_events(question_id,sequence,command_id,event_id,event_json,event_digest,scope_digest,previous_digest,chain_digest,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![scope.question.question_id.as_str(),sequence_i64,command_id.as_str(),event_key,event_text,event_digest,scope_hash,previous,chain,new_revision.to_string()],
    )?;
    tx.execute(
        "UPDATE durable_questions SET event_count=?2,head_digest=?3,revision=?4,projection_json=?5,projection_digest=?6 WHERE question_id=?1",
        params![scope.question.question_id.as_str(),sequence_i64,chain,new_revision.to_string(),public_text,public_digest],
    )?;

    let escalation_ref = if let Some(proposal) = reduction.escalation.as_ref() {
        let bytes = bounded_json(&escalation_view(scope, proposal), MAX_PROJECTION_BYTES)?;
        let proposal_json = String::from_utf8(bytes.clone())
            .map_err(|error| QuestionStoreError::Integrity(error.to_string()))?;
        let proposal_digest = digest(&bytes);
        tx.execute(
            "INSERT INTO durable_question_escalations(question_id,escalation_ref,proposal_json,proposal_digest) VALUES(?1,?2,?3,?4)",
            params![scope.question.question_id.as_str(),proposal_digest,proposal_json,proposal_digest],
        )?;
        Some(proposal_digest)
    } else {
        None
    };
    let receipt = QuestionReceipt {
        command_id: command_id.clone(),
        event_seq: sequence.to_string(),
        revision: new_revision.to_string(),
        duplicate: false,
        escalation_ref,
    };
    insert_receipt_for_question(tx, scope, &command_digest, &receipt)?;
    Ok(receipt)
}

fn insert_receipt_for_question(
    tx: &Transaction<'_>,
    scope: &JournalScope,
    command_digest: &str,
    receipt: &QuestionReceipt,
) -> Result<()> {
    let (json, digest) = receipt_json(receipt)?;
    tx.execute(
        "INSERT INTO durable_question_commands(command_id,question_id,command_digest,receipt_json,receipt_digest) VALUES(?1,?2,?3,?4,?5)",
        params![receipt.command_id.as_str(),scope.question.question_id.as_str(),command_digest,json,digest],
    )?;
    Ok(())
}

/// An independently installed authority is checked inside this writer turn on
/// every call, including a historical exact retry. No request can install one.
/// The supplied TaskView must come from the sole Store writer's projection.
pub(crate) fn append_authorized(
    connection: &mut Connection,
    command: &QuestionCommand,
    authority: Option<&dyn QuestionAuthority>,
    current_task: Option<&TaskView>,
) -> Result<QuestionReceipt> {
    let full_digest = validate_command(command)?;
    let authority = authority.ok_or(QuestionStoreError::AccessDenied)?;
    let task = current_task.ok_or(QuestionStoreError::AccessDenied)?;
    validate_task_namespace(&command.scope, &task.contract)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !authority.authorize_command(&QuestionCommandBinding {
        command,
        command_digest: &full_digest,
        current_task: task,
    }) {
        return Err(QuestionStoreError::AccessDenied);
    }
    validate_global_bounds(&tx)?;
    if let Some((recorded, _, digest)) = read_authorization(&tx, &command.command_id)? {
        if recorded != *command || digest != full_digest {
            return Err(QuestionStoreError::IdentityCollision);
        }
        // Check every protected command before returning an old receipt; a
        // missing later authorization cannot downgrade the current history.
        original_contract_digest(&tx, &command.scope)?
            .ok_or_else(|| QuestionStoreError::Integrity("protected opening is absent".into()))?;
        // Reconstruct original history without replacing it with today's task
        // contract. An authorized old retry returns only its immutable receipt.
        reconstruct_inner(&tx, &command.scope, false)?.ok_or_else(|| {
            QuestionStoreError::Integrity("authorized receipt has no question".into())
        })?;
        let receipt = cached_receipt(
            &tx,
            command.command_id.as_str(),
            &command_digest(&command.scope, &command.event)?,
        )?
        .ok_or_else(|| QuestionStoreError::Integrity("authorized receipt is absent".into()))?;
        tx.commit()?;
        return Ok(receipt);
    }
    let old_command: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM durable_question_commands WHERE command_id=?1)",
        [command.command_id.as_str()],
        |r| r.get(0),
    )?;
    let source_used: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM durable_question_authorizations WHERE producer_id=?1 AND incarnation=?2 AND source_sequence=?3)",
        params![command.binding.source.producer_id,command.binding.source.incarnation,command.binding.source.sequence],
        |r| r.get(0),
    )?;
    if old_command || source_used {
        return Err(QuestionStoreError::IdentityCollision);
    }
    let contract_bytes = bounded_json(&task.contract, MAX_CONTRACT_BYTES)?;
    let contract_digest = digest(&contract_bytes);
    if task.contract.revision != command.binding.expected_contract_revision
        || contract_digest != command.binding.expected_contract_digest
        || task.contract.authority_ref != command.scope.authority_ref
    {
        return Err(QuestionStoreError::StaleContract);
    }
    if let Some(original) = original_contract_digest(&tx, &command.scope)? {
        if original != contract_digest {
            return Err(QuestionStoreError::StaleContract);
        }
    } else if !matches!(command.event, QuestionEvent::Open { .. }) {
        // Legacy component history has no original protected Store provenance.
        // It cannot silently acquire native authority on a later transition.
        return Err(QuestionStoreError::AccessDenied);
    }
    let receipt = append_inner(
        &tx,
        &command.scope,
        &command.command_id,
        &command.event,
        false,
    )?;
    let command_json = bounded_json(command, MAX_QUESTION_COMMAND_BYTES)?;
    tx.execute(
        "INSERT INTO durable_question_authorizations(command_id,producer_id,incarnation,source_sequence,command_json,command_digest,original_contract_json,original_contract_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![command.command_id.as_str(),command.binding.source.producer_id,command.binding.source.incarnation,command.binding.source.sequence,String::from_utf8(command_json).map_err(|e|QuestionStoreError::Invalid(e.to_string()))?,full_digest,String::from_utf8(contract_bytes).map_err(|e|QuestionStoreError::Invalid(e.to_string()))?,contract_digest],
    )?;
    validate_global_bounds(&tx)?;
    tx.commit()?;
    Ok(receipt)
}

/// Scoped current reads require their own current permission. Historical
/// records survive contract changes; current reads report StaleContract rather
/// than making the database unopenable or rewriting the retained question.
pub(crate) fn read_authorized(
    connection: &Connection,
    request: &QuestionRead,
    authority: Option<&dyn QuestionAuthority>,
    current_task: Option<&TaskView>,
) -> Result<Option<DurableQuestion>> {
    validate_read_request(request)?;
    let authority = authority.ok_or(QuestionStoreError::AccessDenied)?;
    let task = current_task.ok_or(QuestionStoreError::AccessDenied)?;
    validate_task_namespace(&request.scope, &task.contract)?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
    if !authority.authorize_read(request, task) {
        return Err(QuestionStoreError::AccessDenied);
    }
    validate_global_bounds(&tx)?;
    let current = reconstruct_inner(&tx, &request.scope, false)?;
    if current.is_some() {
        let original = original_contract_digest(&tx, &request.scope)?
            .ok_or(QuestionStoreError::AccessDenied)?;
        let current_digest = digest(&bounded_json(&task.contract, MAX_CONTRACT_BYTES)?);
        if original != current_digest {
            return Err(QuestionStoreError::StaleContract);
        }
    }
    tx.commit()?;
    Ok(current)
}

fn validate_task_namespace(scope: &JournalScope, contract: &TaskContract) -> Result<()> {
    if contract.project_id != scope.question.project_id
        || contract.root_id != scope.root_id
        || contract.task_id != scope.question.task_id
    {
        return Err(QuestionStoreError::ScopeMismatch);
    }
    Ok(())
}

fn read_authorization(
    connection: &Connection,
    id: &MessageId,
) -> Result<Option<(QuestionCommand, TaskContract, String)>> {
    let row = connection.query_row(
        "SELECT command_json,command_digest,original_contract_json,original_contract_digest,producer_id,incarnation,source_sequence FROM durable_question_authorizations WHERE command_id=?1",
        [id.as_str()],
        |r| Ok((stored_text(r,0,MAX_QUESTION_COMMAND_BYTES)?,stored_text(r,1,71)?,stored_text(r,2,MAX_CONTRACT_BYTES)?,stored_text(r,3,71)?,stored_text(r,4,128)?,stored_text(r,5,128)?,stored_text(r,6,20)?)),
    ).optional()?;
    let Some((
        text,
        recorded_digest,
        contract_text,
        contract_digest,
        producer,
        incarnation,
        sequence,
    )) = row
    else {
        return Ok(None);
    };
    let command: QuestionCommand = serde_json::from_str(&text)?;
    let contract: TaskContract = serde_json::from_str(&contract_text)?;
    contract
        .validate()
        .map_err(|e| QuestionStoreError::Integrity(e.to_string()))?;
    validate_task_namespace(&command.scope, &contract)?;
    if &command.command_id != id
        || validate_command(&command)? != recorded_digest
        || bounded_json(&command, MAX_QUESTION_COMMAND_BYTES)? != text.as_bytes()
        || digest(contract_text.as_bytes()) != contract_digest
        || bounded_json(&contract, MAX_CONTRACT_BYTES)? != contract_text.as_bytes()
        || contract_digest != command.binding.expected_contract_digest
        || contract.revision != command.binding.expected_contract_revision
        || contract.authority_ref != command.scope.authority_ref
        || producer != command.binding.source.producer_id
        || incarnation != command.binding.source.incarnation
        || sequence != command.binding.source.sequence
    {
        return Err(QuestionStoreError::Integrity(
            "question provenance differs from original writer binding".into(),
        ));
    }
    let parent: Option<(String, String)> = connection
        .query_row(
            "SELECT question_id,command_digest FROM durable_question_commands WHERE command_id=?1",
            [id.as_str()],
            |r| Ok((stored_text(r, 0, 128)?, stored_text(r, 1, 71)?)),
        )
        .optional()?;
    if parent
        != Some((
            command.scope.question.question_id.to_string(),
            command_digest(&command.scope, &command.event)?,
        ))
    {
        return Err(QuestionStoreError::Integrity(
            "orphaned or mismatched protected question provenance".into(),
        ));
    }
    Ok(Some((command, contract, recorded_digest)))
}

fn original_contract_digest(
    connection: &Connection,
    scope: &JournalScope,
) -> Result<Option<String>> {
    let mut query=connection.prepare("SELECT a.command_id FROM durable_question_authorizations a JOIN durable_question_commands c ON c.command_id=a.command_id WHERE c.question_id=?1 ORDER BY a.command_id LIMIT 67")?;
    let mut rows = query.query([scope.question.question_id.as_str()])?;
    let mut count = 0usize;
    let mut original: Option<String> = None;
    let mut opened = false;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_COMMANDS_PER_QUESTION {
            return Err(QuestionStoreError::CapacityFull);
        }
        let id = MessageId::new(stored_text(row, 0, 128)?)
            .map_err(|e| QuestionStoreError::Integrity(e.to_string()))?;
        let (command, _, _) = read_authorization(connection, &id)?
            .ok_or_else(|| QuestionStoreError::Integrity("missing question provenance".into()))?;
        if command.scope != *scope {
            return Err(QuestionStoreError::ScopeMismatch);
        }
        let expected = command.binding.expected_contract_digest;
        if original.as_ref().is_some_and(|d| d != &expected) {
            return Err(QuestionStoreError::Integrity(
                "question changed its original task contract".into(),
            ));
        }
        original = Some(expected);
        opened |= matches!(command.event, QuestionEvent::Open { .. });
    }
    if count > 0 {
        let commands: i64 = connection.query_row(
            "SELECT count(*) FROM durable_question_commands WHERE question_id=?1",
            [scope.question.question_id.as_str()],
            |r| r.get(0),
        )?;
        if !opened || usize::try_from(commands).ok() != Some(count) {
            return Err(QuestionStoreError::Integrity(
                "protected question has incomplete command provenance".into(),
            ));
        }
    }
    Ok(original)
}

fn validate_global_bounds(connection: &Connection) -> Result<()> {
    let mut total_bytes = 0_i64;
    for (table, cap, columns) in [
        (
            "durable_questions",
            MAX_TOTAL_QUESTIONS,
            &[
                "question_id",
                "root_id",
                "project_id",
                "task_id",
                "authority_ref",
                "head_digest",
                "revision",
                "projection_json",
                "projection_digest",
            ][..],
        ),
        (
            "durable_question_commands",
            MAX_TOTAL_COMMANDS,
            &[
                "command_id",
                "question_id",
                "command_digest",
                "receipt_json",
                "receipt_digest",
            ][..],
        ),
        (
            "durable_question_events",
            MAX_TOTAL_COMMANDS,
            &[
                "question_id",
                "command_id",
                "event_id",
                "event_json",
                "event_digest",
                "scope_digest",
                "previous_digest",
                "chain_digest",
                "revision",
            ][..],
        ),
        (
            "durable_question_escalations",
            MAX_TOTAL_QUESTIONS,
            &[
                "question_id",
                "escalation_ref",
                "proposal_json",
                "proposal_digest",
            ][..],
        ),
        (
            "durable_question_authorizations",
            MAX_TOTAL_COMMANDS,
            &[
                "command_id",
                "producer_id",
                "incarnation",
                "source_sequence",
                "command_json",
                "command_digest",
                "original_contract_json",
                "original_contract_digest",
            ][..],
        ),
    ] {
        // SQL identifiers are compile-time literals. Inspect at most cap+1
        // records before any bounded text decode; even a corrupt overfull
        // table cannot cause an unbounded history materialization.
        let count: i64 = connection.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)"),
            [cap + 1],
            |r| r.get(0),
        )?;
        if count < 0 || count > cap {
            return Err(QuestionStoreError::CapacityFull);
        }
        let lengths = columns
            .iter()
            .map(|name| format!("length(CAST({name} AS BLOB))"))
            .collect::<Vec<_>>()
            .join("+");
        let bytes: i64 = connection.query_row(
            &format!("SELECT coalesce(sum({lengths}),0) FROM {table}"),
            [],
            |r| r.get(0),
        )?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or(QuestionStoreError::CapacityFull)?;
        if bytes < 0 || total_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(QuestionStoreError::CapacityFull);
        }
    }
    let over_root: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM durable_questions GROUP BY root_id HAVING count(*)>1024)",
        [],
        |r| r.get(0),
    )?;
    if over_root {
        return Err(QuestionStoreError::CapacityFull);
    }
    Ok(())
}

/// Validate every retained question and authorization at startup/rebuild/backup.
/// No current permission, current contract revision or host clock is substituted
/// into original history. Legacy component rows remain observable history only;
/// protected Store reads/actions still require original provenance+current grant.
pub fn validate_all(connection: &Connection) -> Result<()> {
    if connection.is_autocommit() {
        let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
        validate_all_inner(&tx)?;
        tx.commit()?;
        Ok(())
    } else {
        validate_all_inner(connection)
    }
}
fn validate_all_inner(connection: &Connection) -> Result<()> {
    validate_global_bounds(connection)?;
    let orphaned:bool=connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM durable_question_events e LEFT JOIN durable_questions q ON q.question_id=e.question_id WHERE q.question_id IS NULL) OR EXISTS(SELECT 1 FROM durable_question_commands c LEFT JOIN durable_questions q ON q.question_id=c.question_id WHERE q.question_id IS NULL) OR EXISTS(SELECT 1 FROM durable_question_escalations e LEFT JOIN durable_questions q ON q.question_id=e.question_id WHERE q.question_id IS NULL) OR EXISTS(SELECT 1 FROM durable_question_authorizations a LEFT JOIN durable_question_commands c ON c.command_id=a.command_id WHERE c.command_id IS NULL)",
        [],|r|r.get(0),
    )?;
    if orphaned {
        return Err(QuestionStoreError::Integrity(
            "orphaned question history".into(),
        ));
    }
    let mut query = connection
        .prepare("SELECT question_id FROM durable_questions ORDER BY question_id LIMIT 4097")?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let id = stored_text(row, 0, 128)?;
        MessageId::new(&id).map_err(|e| QuestionStoreError::Integrity(e.to_string()))?;
        let scope = read_stored_scope(connection, &id)?.ok_or_else(|| {
            QuestionStoreError::Integrity("question disappeared during validation".into())
        })?;
        reconstruct_inner(connection, &scope, false)?.ok_or_else(|| {
            QuestionStoreError::Integrity("question disappeared during replay".into())
        })?;
        original_contract_digest(connection, &scope)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::tempdir;

    fn msg(value: &str) -> MessageId {
        MessageId::new(value).expect("fixture message ID is valid")
    }
    fn principal(value: &str) -> PrincipalId {
        PrincipalId::new(value).expect("fixture principal ID is valid")
    }
    fn scope() -> JournalScope {
        JournalScope {
            root_id: RootId::new("root-a").expect("fixture root ID is valid"),
            question: QuestionScope {
                question_id: msg("q-a"),
                project_id: ProjectId::new("project-a").expect("fixture project ID is valid"),
                task_id: TaskId::new("task-a").expect("fixture task ID is valid"),
            },
            authority_ref: "grant:task-a:r1".into(),
        }
    }
    fn open(scope: &JournalScope, id: &str, deadline: Option<u64>) -> QuestionEvent {
        QuestionEvent::Open {
            event_id: msg(id),
            scope: scope.question.clone(),
            authority_ref: scope.authority_ref.clone(),
            asker: principal("asker"),
            recipient: principal("worker"),
            allowed_recipients: vec![principal("worker"), principal("captain")],
            requirement: "Keep exact scope and preserve unknown deadline.".into(),
            question: "Which revision is current?".into(),
            options: vec!["v1".into(), "v2".into()],
            deadline_utc: deadline,
        }
    }
    fn route(scope: &JournalScope, id: &str, revision: u64) -> QuestionEvent {
        QuestionEvent::Route {
            event_id: msg(id),
            scope: scope.question.clone(),
            expected_revision: revision,
            authority_ref: scope.authority_ref.clone(),
            actor: principal("worker"),
            recipient: principal("captain"),
        }
    }
    fn answer(scope: &JournalScope, id: &str, revision: u64) -> QuestionEvent {
        QuestionEvent::Answer {
            event_id: msg(id),
            scope: scope.question.clone(),
            expected_revision: revision,
            authority_ref: scope.authority_ref.clone(),
            actor: principal("captain"),
            answer: "v2; preserve the requested boundary".into(),
        }
    }
    fn deadline(
        scope: &JournalScope,
        id: &str,
        revision: u64,
        observed_utc: Option<u64>,
    ) -> QuestionEvent {
        QuestionEvent::EvaluateDeadline {
            event_id: msg(id),
            scope: scope.question.clone(),
            expected_revision: revision,
            authority_ref: scope.authority_ref.clone(),
            actor: principal("asker"),
            observed_utc,
        }
    }
    fn db() -> (tempfile::TempDir, Connection) {
        let directory = tempdir().expect("fixture temporary directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private test directory");
        }
        let connection = Connection::open(directory.path().join("questions.sqlite3"))
            .expect("fixture database opens");
        initialize(&connection).expect("question journal initializes");
        (directory, connection)
    }

    #[test]
    fn route_answer_reopen_replays_exact_scope_authority_and_revision() {
        let (directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", None),
        )
        .expect("opening appends");
        let routed = append(
            &mut connection,
            &scope,
            &msg("cmd-route"),
            &route(&scope, "evt-route", 1),
        )
        .expect("CAS route appends");
        assert_eq!(routed.revision, "2");
        let answered = append(
            &mut connection,
            &scope,
            &msg("cmd-answer"),
            &answer(&scope, "evt-answer", 2),
        )
        .expect("current recipient answers");
        assert_eq!(answered.revision, "3");
        drop(connection);
        let reopened =
            Connection::open(directory.path().join("questions.sqlite3")).expect("database reopens");
        let question = reconstruct(&reopened, &scope)
            .expect("replay verifies")
            .expect("question exists");
        assert_eq!(question.state.revision, 3);
        assert_eq!(question.state.authority_ref, scope.authority_ref);
        assert_eq!(
            question.state.status,
            QuestionStatus::Answered {
                answer: "v2; preserve the requested boundary".into()
            }
        );
    }

    #[test]
    fn deadline_unknown_before_after_and_replay_use_recorded_observation() {
        let (_directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", Some(100)),
        )
        .expect("opening appends");
        assert!(
            append(
                &mut connection,
                &scope,
                &msg("cmd-unknown"),
                &deadline(&scope, "evt-unknown", 1, None)
            )
            .is_err()
        );
        assert!(
            append(
                &mut connection,
                &scope,
                &msg("cmd-before"),
                &deadline(&scope, "evt-before", 1, Some(100))
            )
            .is_err()
        );
        let expiry = deadline(&scope, "evt-expire", 1, Some(101));
        let first = append(&mut connection, &scope, &msg("cmd-expire"), &expiry)
            .expect("after-deadline observation appends");
        assert!(first.escalation_ref.is_some());
        assert!(
            !append(&mut connection, &scope, &msg("cmd-expire"), &expiry)
                .expect("exact command replay")
                .duplicate
        );
        let duplicate_event = append(&mut connection, &scope, &msg("cmd-expire-retry"), &expiry)
            .expect("same event under a new command ID returns its original event receipt");
        assert!(duplicate_event.duplicate);
        assert_eq!(duplicate_event.escalation_ref, first.escalation_ref);
        let escalations: i64 = connection
            .query_row(
                "SELECT count(*) FROM durable_question_escalations WHERE question_id=?1",
                [scope.question.question_id.as_str()],
                |row| row.get(0),
            )
            .expect("escalation proposal count is readable");
        assert_eq!(escalations, 1, "replay never repeats escalation proposal");
        let replayed = reconstruct(&connection, &scope)
            .expect("reopen-style replay")
            .expect("expired question exists");
        assert_eq!(
            replayed.state.status,
            QuestionStatus::Expired { observed_utc: 101 }
        );
        assert_eq!(replayed.escalation_ref, first.escalation_ref);
    }

    #[test]
    fn command_replay_is_idempotent_and_changed_content_or_foreign_root_collides() {
        let (_directory, mut connection) = db();
        let scope = scope();
        let event = open(&scope, "evt-open", None);
        let first =
            append(&mut connection, &scope, &msg("cmd-open"), &event).expect("open appends");
        assert_eq!(
            append(&mut connection, &scope, &msg("cmd-open"), &event).expect("exact command retry"),
            first
        );
        let changed = open(&scope, "evt-open", Some(500));
        assert!(matches!(
            append(&mut connection, &scope, &msg("cmd-open"), &changed),
            Err(QuestionStoreError::IdentityCollision)
        ));
        let mut foreign = scope.clone();
        foreign.root_id = RootId::new("root-b").expect("second root ID is valid");
        assert!(matches!(
            append(
                &mut connection,
                &foreign,
                &msg("cmd-foreign"),
                &open(&foreign, "evt-foreign", None)
            ),
            Err(QuestionStoreError::ScopeMismatch)
        ));
        assert!(matches!(
            append(
                &mut connection,
                &foreign,
                &msg("cmd-open"),
                &open(&foreign, "evt-open", None)
            ),
            Err(QuestionStoreError::IdentityCollision)
        ));
        assert!(matches!(
            append(
                &mut connection,
                &scope,
                &msg("evt-open"),
                &answer(&scope, "evt-answer", 1)
            ),
            Err(QuestionStoreError::IdentityCollision) | Err(QuestionStoreError::Reducer(_))
        ));
    }

    #[test]
    fn event_and_projection_tampering_fail_reconstruction() {
        let (_directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", None),
        )
        .expect("open appends");
        connection
            .execute(
                "UPDATE durable_question_events SET event_json=?2 WHERE question_id=?1",
                params![scope.question.question_id.as_str(), "{}"],
            )
            .expect("tamper fixture updates event");
        assert!(matches!(
            reconstruct(&connection, &scope),
            Err(QuestionStoreError::Json(_))
        ));
        connection.execute("UPDATE durable_question_events SET event_json=?2,event_digest=?3 WHERE question_id=?1",params![scope.question.question_id.as_str(),serde_json::to_string(&open(&scope,"evt-open",None)).expect("fixture event encodes"),event_digest(&bounded_json(&open(&scope,"evt-open",None),MAX_QUESTION_EVENT_BYTES).expect("fixture event bounded"))])
            .expect("restore event fixture");
        connection
            .execute(
                "UPDATE durable_questions SET projection_json='{}' WHERE question_id=?1",
                [scope.question.question_id.as_str()],
            )
            .expect("tamper fixture updates projection");
        assert!(matches!(
            reconstruct(&connection, &scope),
            Err(QuestionStoreError::Integrity(_))
        ));
    }

    #[test]
    fn rejected_cas_leaves_no_partial_rows_and_bounds_apply_without_eviction() {
        let (_directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", None),
        )
        .expect("open appends");
        let before: i64 = connection
            .query_row(
                "SELECT event_count FROM durable_questions WHERE question_id=?1",
                [scope.question.question_id.as_str()],
                |row| row.get(0),
            )
            .expect("count is readable");
        assert!(
            append(
                &mut connection,
                &scope,
                &msg("cmd-stale"),
                &route(&scope, "evt-stale", 99)
            )
            .is_err()
        );
        let after: i64 = connection
            .query_row(
                "SELECT event_count FROM durable_questions WHERE question_id=?1",
                [scope.question.question_id.as_str()],
                |row| row.get(0),
            )
            .expect("count remains readable");
        assert_eq!(before, after);
        let rows: i64 = connection
            .query_row(
                "SELECT count(*) FROM durable_question_commands WHERE command_id='cmd-stale'",
                [],
                |row| row.get(0),
            )
            .expect("receipt count is readable");
        assert_eq!(rows, 0, "failed CAS creates no command receipt");
        assert!(
            reconstruct(&connection, &scope)
                .expect("valid journal reconstructs")
                .is_some()
        );
    }

    #[test]
    fn full_question_history_applies_backpressure_without_eviction() {
        let (_directory, mut connection) = db();
        let scope = scope();
        let allowed: Vec<_> = (0..32)
            .map(|index| principal(&format!("person{index}")))
            .collect();
        let opening = QuestionEvent::Open {
            event_id: msg("evt-open"),
            scope: scope.question.clone(),
            authority_ref: scope.authority_ref.clone(),
            asker: principal("asker"),
            recipient: allowed[0].clone(),
            allowed_recipients: allowed.clone(),
            requirement: "Keep all receipts.".into(),
            question: "Route within the bounded list.".into(),
            options: Vec::new(),
            deadline_utc: None,
        };
        append(&mut connection, &scope, &msg("cmd-open"), &opening).expect("opening appends");
        for index in 1..allowed.len() {
            let event = QuestionEvent::Route {
                event_id: msg(&format!("evt-route-{index}")),
                scope: scope.question.clone(),
                expected_revision: index as u64,
                authority_ref: scope.authority_ref.clone(),
                actor: allowed[index - 1].clone(),
                recipient: allowed[index].clone(),
            };
            append(
                &mut connection,
                &scope,
                &msg(&format!("cmd-route-{index}")),
                &event,
            )
            .expect("allowlisted route appends");
        }
        let cancel = QuestionEvent::Cancel {
            event_id: msg("evt-cancel"),
            scope: scope.question.clone(),
            expected_revision: 32,
            authority_ref: scope.authority_ref.clone(),
            actor: principal("asker"),
        };
        append(&mut connection, &scope, &msg("cmd-cancel"), &cancel)
            .expect("terminal receipt fills the bounded history");
        let before: i64 = connection
            .query_row(
                "SELECT event_count FROM durable_questions WHERE question_id=?1",
                [scope.question.question_id.as_str()],
                |row| row.get(0),
            )
            .expect("full event count is readable");
        assert_eq!(before, MAX_HISTORY as i64);
        let after_terminal = QuestionEvent::Route {
            event_id: msg("evt-after-terminal"),
            scope: scope.question.clone(),
            expected_revision: 33,
            authority_ref: scope.authority_ref.clone(),
            actor: allowed[31].clone(),
            recipient: allowed[0].clone(),
        };
        assert!(
            append(
                &mut connection,
                &scope,
                &msg("cmd-after-terminal"),
                &after_terminal,
            )
            .is_err()
        );
        let after: i64 = connection
            .query_row(
                "SELECT event_count FROM durable_questions WHERE question_id=?1",
                [scope.question.question_id.as_str()],
                |row| row.get(0),
            )
            .expect("event count remains readable");
        assert_eq!(after, before, "bounded history is never evicted");
    }

    #[test]
    fn forged_canonical_receipts_are_rejected_even_with_recomputed_hashes() {
        for field in ["revision", "sequence", "duplicate", "escalation"] {
            let (_directory, mut connection) = db();
            let scope = scope();
            let event = open(&scope, "evt-open", None);
            let mut forged =
                append(&mut connection, &scope, &msg("cmd-open"), &event).expect("opening");
            match field {
                "revision" => forged.revision = "999".into(),
                "sequence" => forged.event_seq = "2".into(),
                "duplicate" => forged.duplicate = true,
                _ => forged.escalation_ref = Some(format!("sha256:{}", "a".repeat(64))),
            }
            let (json, hash) = receipt_json(&forged).expect("canonical forged receipt");
            connection.execute("UPDATE durable_question_commands SET receipt_json=?1,receipt_digest=?2 WHERE command_id='cmd-open'",params![json,hash]).expect("inject own forged receipt");
            assert!(
                reconstruct(&connection, &scope).is_err(),
                "reopen must reject {field} forgery"
            );
            assert!(
                append(&mut connection, &scope, &msg("cmd-open"), &event).is_err(),
                "cached return must reject {field} forgery"
            );
            let count: i64 = connection
                .query_row("SELECT count(*) FROM durable_question_events", [], |row| {
                    row.get(0)
                })
                .expect("unchanged event count");
            assert_eq!(count, 1);
        }
        let (_directory, mut connection) = db();
        let scope = scope();
        let event = open(&scope, "evt-open", None);
        append(&mut connection, &scope, &msg("cmd-open"), &event).expect("opening");
        connection
            .execute("DELETE FROM durable_question_commands", [])
            .expect("inject missing original receipt");
        assert!(reconstruct(&connection, &scope).is_err());
        assert!(
            append(&mut connection, &scope, &msg("cmd-retry"), &event).is_err(),
            "missing historical receipts are never silently recreated"
        );
    }

    #[test]
    fn replay_of_opening_after_expiry_does_not_fabricate_an_escalation_receipt() {
        let (_directory, mut connection) = db();
        let scope = scope();
        let opening = open(&scope, "evt-open", Some(100));
        let original =
            append(&mut connection, &scope, &msg("cmd-open"), &opening).expect("opening");
        let expired = append(
            &mut connection,
            &scope,
            &msg("cmd-expire"),
            &deadline(&scope, "evt-expire", 1, Some(101)),
        )
        .expect("recorded expiry");
        assert!(expired.escalation_ref.is_some());
        assert_eq!(
            append(&mut connection, &scope, &msg("cmd-open"), &opening)
                .expect("original receipt remains exact"),
            original
        );
        let replay = append(&mut connection, &scope, &msg("cmd-open-retry"), &opening)
            .expect("new command for identical old event");
        assert!(replay.duplicate);
        assert_eq!(replay.event_seq, "1");
        assert_eq!(replay.revision, "1");
        assert!(replay.escalation_ref.is_none());
        let replayed = reconstruct(&connection, &scope)
            .expect("verified history")
            .expect("question exists");
        assert_eq!(replayed.event_count, "2");
        assert_eq!(replayed.escalation_ref, expired.escalation_ref);
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM durable_question_escalations",
                [],
                |row| row.get(0),
            )
            .expect("proposal count");
        assert_eq!(count, 1);
    }

    #[test]
    fn sql_receipt_failure_rolls_back_event_projection_and_escalation_together() {
        let (_directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", Some(100)),
        )
        .expect("opening");
        let before = reconstruct(&connection, &scope)
            .expect("before fault")
            .expect("question");
        connection.execute_batch("CREATE TRIGGER reject_question_receipt BEFORE INSERT ON durable_question_commands BEGIN SELECT RAISE(ABORT,'receipt fault'); END").expect("own fault trigger");
        let expiry = deadline(&scope, "evt-expire", 1, Some(101));
        assert!(append(&mut connection, &scope, &msg("cmd-expire"), &expiry).is_err());
        assert_eq!(
            reconstruct(&connection, &scope)
                .expect("rollback reconstructs")
                .expect("question"),
            before
        );
        let counts:(i64,i64,i64)=connection.query_row("SELECT (SELECT count(*) FROM durable_question_events),(SELECT count(*) FROM durable_question_commands),(SELECT count(*) FROM durable_question_escalations)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).expect("actual SQLite counts");
        assert_eq!(counts, (1, 1, 0));
        connection
            .execute_batch("DROP TRIGGER reject_question_receipt")
            .expect("remove own fault");
        assert!(
            append(&mut connection, &scope, &msg("cmd-expire"), &expiry)
                .expect("same command after rollback")
                .escalation_ref
                .is_some()
        );
    }

    #[test]
    fn two_connections_serialize_question_revision_cas() {
        let (directory, mut connection) = db();
        let scope = scope();
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", None),
        )
        .expect("opening");
        let path = directory.path().join("questions.sqlite3");
        let start = std::sync::Barrier::new(2);
        let results = std::thread::scope(|threads| {
            let mut workers = Vec::new();
            for (command, event) in [
                ("cmd-route", route(&scope, "evt-route", 1)),
                ("cmd-answer", answer(&scope, "evt-answer", 1)),
            ] {
                let path = &path;
                let scope = &scope;
                let start = &start;
                workers.push(threads.spawn(move || {
                    let mut connection = Connection::open(path).expect("own competing connection");
                    connection
                        .busy_timeout(std::time::Duration::from_secs(2))
                        .expect("bounded busy wait");
                    initialize(&connection).expect("same explicit schema");
                    start.wait();
                    append(&mut connection, scope, &msg(command), &event)
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().expect("owned test thread finishes"))
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(QuestionStoreError::Reducer(_))))
                .count(),
            1
        );
        let question = reconstruct(&connection, &scope)
            .expect("canonical race result")
            .expect("question");
        assert_eq!(question.event_count, "2");
        assert_eq!(question.state.revision, 2);
    }

    #[test]
    fn journal_in_a_store_database_requires_its_exact_immutable_task_scope() {
        let directory = tempdir().expect("owned store fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private store fixture");
        }
        let path = directory.path().join("store.sqlite");
        let store = crate::Store::open(&path).expect("real Store");
        let scope = scope();
        let contract:admiralty_core::TaskContract=serde_json::from_value(serde_json::json!({
            "schema_version":1,"project_id":scope.question.project_id,"root_id":scope.root_id,"task_id":scope.question.task_id,"revision":7,"title":"Question scope","objective":"Bind the durable journal to this contract","non_goals":[],"owned_paths":["src/lib.rs"],"forbidden_paths":[],"read_first":[],"dependencies":[],"criteria":[{"id":"c","text":"Scope must match","check_ids":["test"]}],"checks":[{"id":"test","program":"cargo","args":["test"],"cwd":".","timeout_secs":1}],"authority_ref":scope.authority_ref,"max_attempts":1,"simulated":true
        })).expect("bounded real contract");
        store
            .append(
                admiralty_core::SourceIdentity {
                    producer_id: "scope-test".into(),
                    incarnation: "one".into(),
                    sequence: "1".into(),
                },
                admiralty_core::DomainEvent::TaskSubmitted { contract },
                "recorded".into(),
            )
            .expect("actual durable task");
        drop(store);
        let mut connection = Connection::open(&path).expect("offline owned writer connection");
        initialize(&connection).expect("explicit question storage");
        for mut foreign in [scope.clone(), scope.clone(), scope.clone(), scope.clone()]
            .into_iter()
            .enumerate()
        {
            match foreign.0 {
                0 => foreign.1.root_id = RootId::new("foreign-root").expect("valid root"),
                1 => {
                    foreign.1.question.project_id =
                        ProjectId::new("foreign-project").expect("valid project")
                }
                2 => foreign.1.question.task_id = TaskId::new("foreign-task").expect("valid task"),
                _ => foreign.1.authority_ref = "grant:foreign".into(),
            }
            assert!(matches!(
                append(
                    &mut connection,
                    &foreign.1,
                    &msg("cmd-foreign"),
                    &open(&foreign.1, "evt-foreign", None)
                ),
                Err(QuestionStoreError::ScopeMismatch)
            ));
        }
        append(
            &mut connection,
            &scope,
            &msg("cmd-open"),
            &open(&scope, "evt-open", None),
        )
        .expect("exact Store contract scope");
        assert!(
            reconstruct(&connection, &scope)
                .expect("verified task and journal")
                .is_some()
        );
    }

    #[test]
    fn command_capacity_preserves_original_receipts_without_eviction() {
        let (_directory, mut connection) = db();
        let scope = scope();
        let event = open(&scope, "evt-open", None);
        let original = append(&mut connection, &scope, &msg("cmd-open"), &event).expect("opening");
        for index in 1..MAX_COMMANDS_PER_QUESTION {
            assert!(
                append(
                    &mut connection,
                    &scope,
                    &msg(&format!("cmd-retry-{index}")),
                    &event
                )
                .expect("bounded duplicate receipt")
                .duplicate
            );
        }
        assert!(matches!(
            append(&mut connection, &scope, &msg("cmd-over-capacity"), &event),
            Err(QuestionStoreError::CapacityFull)
        ));
        assert_eq!(
            append(&mut connection, &scope, &msg("cmd-open"), &event)
                .expect("exact original retry still works at capacity"),
            original
        );
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM durable_question_commands",
                [],
                |row| row.get(0),
            )
            .expect("retained command count");
        assert_eq!(count, MAX_COMMANDS_PER_QUESTION as i64);
        assert_eq!(
            reconstruct(&connection, &scope)
                .expect("complete retained history")
                .expect("question")
                .event_count,
            "1"
        );
    }
}
