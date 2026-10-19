//! Bounded render history and explicit application-ACK baselines on Store's writer.
//! These are trusted storage seams, not authenticated routes, delivery executors,
//! provider-memory attestations or authority grants. No history is evicted.
use admiralty_core::{
    PrincipalId, ProjectId, RootId, content_digest,
    coordination::context::{
        MAX_ROLE_CAPSULE_BYTES, RecipientRole, RoleCapsule, RoleFacts, decode_role_capsule,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const MAX_RENDERS: usize = 256;
pub const MAX_RENDER_HISTORY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 192 * 1024;
pub const MAX_BASELINE_COMMANDS: usize = 1024;
pub const MAX_BASELINE_HISTORY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAGE_ITEMS: usize = 8;
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
const MAX_SCOPE_BYTES: usize = 8192;
const MAX_COMMAND_BYTES: usize = 16384;
type Result<T> = std::result::Result<T, ContextRenderError>;

#[derive(Debug, thiserror::Error)]
pub enum ContextRenderError {
    #[error("context SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("context JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid or oversized context input")]
    Invalid,
    #[error("context identity has conflicting immutable content")]
    Collision,
    #[error("context history capacity is full; no history was evicted")]
    Capacity,
    #[error("context baseline compare-and-swap is stale")]
    StaleBaseline,
    #[error("exact context render is unavailable in this scope")]
    MissingRender,
    #[error("stored context history is inconsistent")]
    Integrity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ProjectRoot {
    pub project_id: ProjectId,
    pub root_id: RootId,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextNamespace {
    Project {
        project_id: ProjectId,
        root_id: RootId,
    },
    Fleet {
        fleet_id: String,
        project_roots: Vec<ProjectRoot>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextScope {
    pub recipient: PrincipalId,
    pub role: RecipientRole,
    pub namespace: ContextNamespace,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenderKey {
    pub render_id: String,
    pub version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordContextRender {
    pub scope: ContextScope,
    pub key: RenderKey,
    pub capsule: RoleCapsule,
    /// Independently retained compiler/session binding, not taken from capsule text.
    pub expected_epoch: String,
    pub expected_facts_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoredContextRender {
    pub scope: ContextScope,
    pub key: RenderKey,
    pub capsule: RoleCapsule,
    pub rendered_bytes: String,
    pub byte_unit: String,
    pub token_unit: String,
    pub recorded: bool,
    /// Immutable creation-time observation. Current ACK/delivery observations
    /// are separate baseline commands, never provider-memory attestations.
    pub delivered: bool,
    pub application_acknowledged: bool,
    pub authority_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenderRef {
    pub key: RenderKey,
    pub record_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextRenderReceipt {
    pub render: RenderRef,
    pub duplicate: bool,
    pub authority_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenderCursor {
    pub scope_digest: String,
    pub after: RenderKey,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextRenderPage {
    pub entries: Vec<StoredContextRender>,
    pub next: Option<RenderCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineCursor {
    pub revision: String,
    pub state_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdvanceContextBaseline {
    pub scope: ContextScope,
    pub command_id: String,
    pub expected_prior: Option<BaselineCursor>,
    pub render: RenderRef,
    /// An explicit trusted-caller observation, not proof of authenticated delivery.
    pub application_ack_ref: String,
    pub delivery_ref: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextBaseline {
    pub cursor: BaselineCursor,
    pub render: RenderRef,
    pub context_epoch: String,
    pub application_ack_ref: String,
    /// None means transport delivery was not independently recorded.
    pub delivery_ref: Option<String>,
    pub authority_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineReceipt {
    pub baseline: ContextBaseline,
    pub duplicate: bool,
}

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn id(value: &str) -> bool {
    text(value, 128)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    content_digest(value).map_err(|_| ContextRenderError::Invalid)
}
fn json<T: Serialize>(value: &T, max: usize) -> Result<String> {
    admiralty_core::limits::bounded_json_size(value, max)
        .map_err(|_| ContextRenderError::Invalid)?;
    serde_json::to_string(value).map_err(Into::into)
}
fn key_valid(key: &RenderKey) -> bool {
    id(&key.render_id) && key.version > 0
}
impl ContextScope {
    fn validate(&self) -> Result<String> {
        json(self, MAX_SCOPE_BYTES)?;
        if let ContextNamespace::Fleet {
            fleet_id,
            project_roots,
        } = &self.namespace
            && (self.role != RecipientRole::Admiral
                || !id(fleet_id)
                || project_roots.is_empty()
                || project_roots.len() > 16
                || project_roots.windows(2).any(|p| p[0] >= p[1]))
        {
            return Err(ContextRenderError::Invalid);
        }
        hash(self)
    }
    fn includes(&self, project: &ProjectId, root: &RootId) -> bool {
        match &self.namespace {
            ContextNamespace::Project {
                project_id,
                root_id,
            } => project == project_id && root == root_id,
            ContextNamespace::Fleet { project_roots, .. } => project_roots
                .iter()
                .any(|p| &p.project_id == project && &p.root_id == root),
        }
    }
}
fn validate_membership(scope: &ContextScope, facts: &RoleFacts) -> Result<()> {
    let task_ok = |t: &admiralty_core::coordination::context::ProtectedTaskFacts| {
        scope.includes(&t.contract.project_id, &t.contract.root_id)
    };
    let matches = match (facts, &scope.namespace) {
        (RoleFacts::Decision(f), ContextNamespace::Project { .. }) => task_ok(&f.task),
        (RoleFacts::Handoff(f), ContextNamespace::Project { .. }) => {
            task_ok(&f.task) && f.to == scope.recipient
        }
        (RoleFacts::ProjectExceptions(f), ContextNamespace::Project { project_id, .. }) => {
            &f.project_id == project_id && f.exceptions.iter().all(task_ok)
        }
        (RoleFacts::FleetExceptions(f), ContextNamespace::Fleet { project_roots, .. }) => {
            let actual: BTreeSet<_> = f.project_scope.iter().collect();
            let expected: BTreeSet<_> = project_roots.iter().map(|p| &p.project_id).collect();
            actual == expected && f.projects.iter().all(|p| p.exceptions.iter().all(task_ok))
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(ContextRenderError::Invalid)
    }
}
pub(crate) fn validate_record_request(request: &RecordContextRender) -> Result<()> {
    json(request, MAX_RECORD_BYTES)?;
    request.scope.validate()?;
    let capsule = &request.capsule;
    if !key_valid(&request.key)
        || capsule.text.len() > MAX_ROLE_CAPSULE_BYTES
        || capsule.recipient != request.scope.role
        || capsule.context_epoch != request.expected_epoch
        || capsule.facts_digest != request.expected_facts_digest
        || !digest(&capsule.rendered_digest)
        || capsule.rendered_digest
            != format!("sha256:{:x}", Sha256::digest(capsule.text.as_bytes()))
        || capsule.estimated_tokens != capsule.text.len().div_ceil(4)
        || capsule.tokenizer != "utf8_bytes_div4_estimate"
    {
        return Err(ContextRenderError::Invalid);
    }
    let facts = decode_role_capsule(
        &capsule.text,
        request.scope.role,
        &request.expected_epoch,
        &request.expected_facts_digest,
    )
    .map_err(|_| ContextRenderError::Invalid)?;
    validate_membership(&request.scope, &facts)
}
fn make_record(request: &RecordContextRender) -> Result<StoredContextRender> {
    validate_record_request(request)?;
    Ok(StoredContextRender {
        scope: request.scope.clone(),
        key: request.key.clone(),
        capsule: request.capsule.clone(),
        rendered_bytes: request.capsule.text.len().to_string(),
        byte_unit: "utf8_bytes".into(),
        token_unit: "estimated_tokens".into(),
        recorded: true,
        delivered: false,
        application_acknowledged: false,
        authority_granted: false,
    })
}
fn validate_record(record: &StoredContextRender) -> Result<()> {
    if record.rendered_bytes != record.capsule.text.len().to_string()
        || record.byte_unit != "utf8_bytes"
        || record.token_unit != "estimated_tokens"
        || !record.recorded
        || record.delivered
        || record.application_acknowledged
        || record.authority_granted
    {
        return Err(ContextRenderError::Integrity);
    }
    validate_record_request(&RecordContextRender {
        scope: record.scope.clone(),
        key: record.key.clone(),
        capsule: record.capsule.clone(),
        expected_epoch: record.capsule.context_epoch.clone(),
        expected_facts_digest: record.capsule.facts_digest.clone(),
    })
    .map_err(|_| ContextRenderError::Integrity)
}
pub(crate) fn validate_get_request(scope: &ContextScope, key: &RenderKey) -> Result<()> {
    scope.validate()?;
    if !key_valid(key) {
        return Err(ContextRenderError::Invalid);
    }
    Ok(())
}
pub(crate) fn validate_page_request(
    scope: &ContextScope,
    cursor: Option<&RenderCursor>,
    limit: usize,
) -> Result<String> {
    let scope_digest = scope.validate()?;
    if !(1..=MAX_PAGE_ITEMS).contains(&limit)
        || cursor.is_some_and(|c| c.scope_digest != scope_digest || !key_valid(&c.after))
    {
        return Err(ContextRenderError::Invalid);
    }
    Ok(scope_digest)
}
pub(crate) fn validate_advance(request: &AdvanceContextBaseline) -> Result<()> {
    json(request, MAX_COMMAND_BYTES)?;
    request.scope.validate()?;
    if !id(&request.command_id)
        || !key_valid(&request.render.key)
        || !digest(&request.render.record_digest)
        || !text(&request.application_ack_ref, 512)
        || request.delivery_ref.as_ref().is_some_and(|s| !text(s, 512))
        || request.expected_prior.as_ref().is_some_and(|p| {
            !digest(&p.state_digest)
                || p.revision
                    .parse::<i64>()
                    .ok()
                    .is_none_or(|v| v <= 0 || v.to_string() != p.revision)
        })
    {
        return Err(ContextRenderError::Invalid);
    }
    Ok(())
}
fn bounds(connection: &Connection) -> Result<()> {
    let (records,bytes):(i64,i64)=connection.query_row("SELECT count(*),COALESCE(sum(length(CAST(record_json AS BLOB))),0) FROM main.context_render_records",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let (commands,command_bytes):(i64,i64)=connection.query_row("SELECT count(*),COALESCE(sum(length(CAST(request_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM main.context_render_commands",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let baselines: i64 = connection.query_row(
        "SELECT count(*) FROM main.context_render_baselines",
        [],
        |r| r.get(0),
    )?;
    if baselines > MAX_BASELINE_COMMANDS as i64
        || records > MAX_RENDERS as i64
        || bytes > MAX_RENDER_HISTORY_BYTES as i64
        || commands > MAX_BASELINE_COMMANDS as i64
        || command_bytes > MAX_BASELINE_HISTORY_BYTES as i64
    {
        return Err(ContextRenderError::Capacity);
    }
    Ok(())
}
pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0004_context_renders.sql"))?;
    tx.commit()?;
    validate_all(connection)
}
fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, u32, String, String)> {
    Ok((
        super::stored_text(row, 0, 71)?,
        super::stored_text(row, 1, 128)?,
        row.get(2)?,
        super::stored_text(row, 3, MAX_RECORD_BYTES)?,
        super::stored_text(row, 4, 71)?,
    ))
}
fn decode_record(
    row: (String, String, u32, String, String),
    scope: Option<&ContextScope>,
) -> Result<StoredContextRender> {
    let (scope_digest, id, version, bytes, expected) = row;
    let record: StoredContextRender = serde_json::from_str(&bytes)?;
    validate_record(&record)?;
    if record.key.render_id != id
        || record.key.version != version
        || hash(&record.scope)? != scope_digest
        || hash(&record)? != expected
        || scope.is_some_and(|s| s != &record.scope)
    {
        return Err(ContextRenderError::Integrity);
    }
    Ok(record)
}
fn get_inner(
    connection: &Connection,
    scope: &ContextScope,
    key: &RenderKey,
) -> Result<Option<StoredContextRender>> {
    if !key_valid(key) {
        return Err(ContextRenderError::Invalid);
    }
    let scope_digest = scope.validate()?;
    let row=connection.query_row("SELECT scope_digest,render_id,version,record_json,record_digest FROM main.context_render_records WHERE scope_digest=?1 AND render_id=?2 AND version=?3",
        params![scope_digest,key.render_id,key.version],read_record).optional()?;
    row.map(|r| decode_record(r, Some(scope))).transpose()
}
pub(crate) fn get(
    connection: &Connection,
    scope: &ContextScope,
    key: &RenderKey,
) -> Result<Option<StoredContextRender>> {
    get_inner(connection, scope, key)
}
pub(crate) fn record(
    connection: &mut Connection,
    request: RecordContextRender,
) -> Result<ContextRenderReceipt> {
    let record = make_record(&request)?;
    let bytes = json(&record, MAX_RECORD_BYTES)?;
    let record_digest = hash(&record)?;
    let scope_digest = request.scope.validate()?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = get_inner(&tx, &request.scope, &request.key)? {
        if existing != record {
            return Err(ContextRenderError::Collision);
        }
        tx.commit()?;
        return Ok(ContextRenderReceipt {
            render: RenderRef {
                key: request.key,
                record_digest,
            },
            duplicate: true,
            authority_granted: false,
        });
    }
    bounds(&tx)?;
    let (count,total):(i64,i64)=tx.query_row("SELECT count(*),COALESCE(sum(length(CAST(record_json AS BLOB))),0) FROM main.context_render_records",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count >= MAX_RENDERS as i64
        || total
            .checked_add(bytes.len() as i64)
            .is_none_or(|v| v > MAX_RENDER_HISTORY_BYTES as i64)
    {
        return Err(ContextRenderError::Capacity);
    }
    tx.execute(
        "INSERT INTO main.context_render_records VALUES(?1,?2,?3,?4,?5)",
        params![
            scope_digest,
            request.key.render_id,
            request.key.version,
            bytes,
            record_digest
        ],
    )?;
    tx.commit()?;
    Ok(ContextRenderReceipt {
        render: RenderRef {
            key: request.key,
            record_digest,
        },
        duplicate: false,
        authority_granted: false,
    })
}
pub(crate) fn page(
    connection: &Connection,
    scope: &ContextScope,
    cursor: Option<&RenderCursor>,
    limit: usize,
) -> Result<ContextRenderPage> {
    let scope_digest = validate_page_request(scope, cursor, limit)?;
    let after = cursor.map(|c| c.after.render_id.as_str()).unwrap_or("");
    let version = cursor.map(|c| c.after.version).unwrap_or(0);
    let mut statement=connection.prepare("SELECT scope_digest,render_id,version,record_json,record_digest FROM main.context_render_records WHERE scope_digest=?1 AND (render_id>?2 OR (render_id=?2 AND version>?3)) ORDER BY render_id,version LIMIT ?4")?;
    let mut rows = statement.query(params![scope_digest, after, version, (limit + 1) as i64])?;
    let mut entries = Vec::new();
    let mut size = MAX_SCOPE_BYTES;
    let mut more = false;
    while let Some(row) = rows.next()? {
        let raw = read_record(row)?;
        if entries.len() == limit || size + raw.3.len() > MAX_PAGE_BYTES {
            more = true;
            break;
        }
        size += raw.3.len();
        entries.push(decode_record(raw, Some(scope))?);
    }
    let next = if more {
        entries.last().map(|r| RenderCursor {
            scope_digest,
            after: r.key.clone(),
        })
    } else {
        None
    };
    Ok(ContextRenderPage { entries, next })
}
fn baseline_for(
    request: &AdvanceContextBaseline,
    record: &StoredContextRender,
    sequence: i64,
) -> Result<ContextBaseline> {
    let state_digest = hash(&(
        "context-baseline-v1",
        &request.scope,
        sequence,
        &request.render,
        &record.capsule.context_epoch,
        &request.application_ack_ref,
        &request.delivery_ref,
    ))?;
    Ok(ContextBaseline {
        cursor: BaselineCursor {
            revision: sequence.to_string(),
            state_digest,
        },
        render: request.render.clone(),
        context_epoch: record.capsule.context_epoch.clone(),
        application_ack_ref: request.application_ack_ref.clone(),
        delivery_ref: request.delivery_ref.clone(),
        authority_granted: false,
    })
}
fn history(
    connection: &Connection,
    scope_digest: &str,
) -> Result<Vec<(AdvanceContextBaseline, BaselineReceipt)>> {
    let mut statement=connection.prepare("SELECT command_id,sequence,request_json,request_digest,receipt_json,receipt_digest FROM main.context_render_commands WHERE scope_digest=?1 ORDER BY sequence LIMIT 1025")?;
    let mut rows = statement.query([scope_digest])?;
    let mut result = Vec::new();
    let mut previous = None;
    let mut total = 0usize;
    while let Some(row) = rows.next()? {
        if result.len() >= MAX_BASELINE_COMMANDS {
            return Err(ContextRenderError::Capacity);
        }
        let id = super::stored_text(row, 0, 128)?;
        let sequence: i64 = row.get(1)?;
        let request_json = super::stored_text(row, 2, MAX_COMMAND_BYTES)?;
        let request_digest = super::stored_text(row, 3, 71)?;
        let receipt_json = super::stored_text(row, 4, MAX_COMMAND_BYTES)?;
        let receipt_digest = super::stored_text(row, 5, 71)?;
        total = total
            .checked_add(request_json.len() + receipt_json.len())
            .ok_or(ContextRenderError::Capacity)?;
        if total > MAX_BASELINE_HISTORY_BYTES {
            return Err(ContextRenderError::Capacity);
        }
        let request: AdvanceContextBaseline = serde_json::from_str(&request_json)?;
        let receipt: BaselineReceipt = serde_json::from_str(&receipt_json)?;
        validate_advance(&request).map_err(|_| ContextRenderError::Integrity)?;
        if id != request.command_id
            || hash(&request.scope)? != scope_digest
            || sequence != result.len() as i64 + 1
            || request.expected_prior != previous
            || hash(&request)? != request_digest
            || hash(&receipt)? != receipt_digest
        {
            return Err(ContextRenderError::Integrity);
        }
        let record = get_inner(connection, &request.scope, &request.render.key)?
            .ok_or(ContextRenderError::Integrity)?;
        if hash(&record)? != request.render.record_digest
            || receipt.duplicate
            || receipt.baseline != baseline_for(&request, &record, sequence)?
        {
            return Err(ContextRenderError::Integrity);
        }
        previous = Some(receipt.baseline.cursor.clone());
        result.push((request, receipt));
    }
    let pointer: Option<i64> = connection
        .query_row(
            "SELECT current_sequence FROM main.context_render_baselines WHERE scope_digest=?1",
            [scope_digest],
            |r| r.get(0),
        )
        .optional()?;
    if pointer
        != if result.is_empty() {
            None
        } else {
            Some(result.len() as i64)
        }
    {
        return Err(ContextRenderError::Integrity);
    }
    Ok(result)
}
pub(crate) fn baseline(
    connection: &Connection,
    scope: &ContextScope,
) -> Result<Option<ContextBaseline>> {
    let scope_digest = scope.validate()?;
    Ok(history(connection, &scope_digest)?
        .last()
        .map(|(_, r)| r.baseline.clone()))
}
pub(crate) fn advance(
    connection: &mut Connection,
    request: AdvanceContextBaseline,
) -> Result<BaselineReceipt> {
    validate_advance(&request)?;
    let scope_digest = request.scope.validate()?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let history = history(&tx, &scope_digest)?;
    if let Some((original, receipt)) = history
        .iter()
        .find(|(r, _)| r.command_id == request.command_id)
    {
        if original != &request {
            return Err(ContextRenderError::Collision);
        }
        let mut replay = receipt.clone();
        replay.duplicate = true;
        tx.commit()?;
        return Ok(replay);
    }
    if request.expected_prior != history.last().map(|(_, r)| r.baseline.cursor.clone()) {
        return Err(ContextRenderError::StaleBaseline);
    }
    let record = get_inner(&tx, &request.scope, &request.render.key)?
        .ok_or(ContextRenderError::MissingRender)?;
    if hash(&record)? != request.render.record_digest {
        return Err(ContextRenderError::MissingRender);
    }
    let sequence = i64::try_from(history.len())
        .map_err(|_| ContextRenderError::Capacity)?
        .checked_add(1)
        .ok_or(ContextRenderError::Capacity)?;
    let receipt = BaselineReceipt {
        baseline: baseline_for(&request, &record, sequence)?,
        duplicate: false,
    };
    let request_json = json(&request, MAX_COMMAND_BYTES)?;
    let receipt_json = json(&receipt, MAX_COMMAND_BYTES)?;
    bounds(&tx)?;
    let (count,total):(i64,i64)=tx.query_row("SELECT count(*),COALESCE(sum(length(CAST(request_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM main.context_render_commands",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count >= MAX_BASELINE_COMMANDS as i64
        || total
            .checked_add((request_json.len() + receipt_json.len()) as i64)
            .is_none_or(|v| v > MAX_BASELINE_HISTORY_BYTES as i64)
    {
        return Err(ContextRenderError::Capacity);
    }
    tx.execute(
        "INSERT INTO main.context_render_commands VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            scope_digest,
            request.command_id,
            sequence,
            request_json,
            hash(&request)?,
            receipt_json,
            hash(&receipt)?
        ],
    )?;
    tx.execute("INSERT INTO main.context_render_baselines VALUES(?1,?2) ON CONFLICT(scope_digest) DO UPDATE SET current_sequence=excluded.current_sequence",params![scope_digest,sequence])?;
    tx.commit()?;
    Ok(receipt)
}
pub(crate) fn validate_all(connection: &Connection) -> Result<()> {
    bounds(connection)?;
    let mut statement=connection.prepare("SELECT scope_digest,render_id,version,record_json,record_digest FROM main.context_render_records LIMIT 257")?;
    let mut rows = statement.query([])?;
    let mut scopes = BTreeSet::new();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > MAX_RENDERS {
            return Err(ContextRenderError::Capacity);
        }
        let record = decode_record(read_record(row)?, None)?;
        scopes.insert(hash(&record.scope)?);
    }
    let mut statement=connection.prepare("SELECT scope_digest FROM main.context_render_commands UNION SELECT scope_digest FROM main.context_render_baselines LIMIT 2049")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        scopes.insert(super::stored_text(row, 0, 71)?);
        if scopes.len() > MAX_BASELINE_COMMANDS + MAX_RENDERS {
            return Err(ContextRenderError::Capacity);
        }
    }
    for scope in scopes {
        history(connection, &scope)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Store, StoreError};
    use super::*;
    use admiralty_core::coordination::context::*;
    use serde_json::json;

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix("adm-context-")
            .tempdir()
            .expect("private directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .expect("0700");
        }
        dir
    }
    fn protected() -> ProtectedTaskFacts {
        let contract=serde_json::from_value(json!({
            "schema_version":1,"project_id":"project-a","root_id":"root-a","task_id":"task-a",
            "revision":1,"title":"Context fixture","objective":"Do not publish; preserve 5 ms, unknown is not zero",
            "non_goals":["Do not claim acceptance"],"owned_paths":["src/lib.rs"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],
            "criteria":[{"id":"C1","text":"Preserve context","check_ids":["unit"]}],
            "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":10}],
            "authority_ref":"fixture:scope","max_attempts":1,"simulated":true
        })).expect("contract");
        ProtectedTaskFacts {
            contract,
            immutable_refs: vec![],
            candidate: FactObservation::Unknown {
                reason: "Not submitted".into(),
            },
            check: CheckObservation::NotRun {
                reason: "No candidate".into(),
            },
            evidence: vec![],
            uncertainty: vec!["Delivery unknown".into()],
            budget: BudgetObservation::Unknown {
                reason: "No provider measurement".into(),
            },
            next_action: "Implement within scope; do not publish".into(),
        }
    }
    fn project() -> ProjectExceptionFacts {
        ProjectExceptionFacts {
            project_id: ProjectId::new("project-a").expect("project"),
            revision: 1,
            authority_ref: "grant:project".into(),
            source_ref: "events:17".into(),
            exceptions: vec![protected()],
            uncertainty: vec!["No acceptance observed".into()],
            next_action: "Resolve scoped exception".into(),
        }
    }
    fn scope() -> ContextScope {
        ContextScope {
            recipient: PrincipalId::new("manager-a").expect("recipient"),
            role: RecipientRole::Commodore,
            namespace: ContextNamespace::Project {
                project_id: ProjectId::new("project-a").expect("project"),
                root_id: RootId::new("root-a").expect("root"),
            },
        }
    }
    fn request(id: &str, epoch: &str) -> RecordContextRender {
        let capsule =
            compile_project_exceptions(&project(), epoch, 16_384).expect("actual role compiler");
        RecordContextRender {
            scope: scope(),
            key: RenderKey {
                render_id: id.into(),
                version: 1,
            },
            expected_epoch: epoch.into(),
            expected_facts_digest: capsule.facts_digest.clone(),
            capsule,
        }
    }
    fn ack(render: RenderRef, id: &str, prior: Option<BaselineCursor>) -> AdvanceContextBaseline {
        AdvanceContextBaseline {
            scope: scope(),
            command_id: id.into(),
            expected_prior: prior,
            render,
            application_ack_ref: format!("application:{id}"),
            delivery_ref: None,
        }
    }
    fn context_error<T: std::fmt::Debug>(
        result: super::super::Result<T>,
        matches: fn(&ContextRenderError) -> bool,
    ) {
        assert!(
            matches!(result,Err(StoreError::ContextRender(ref error)) if matches(error)),
            "{result:?}"
        );
    }
    #[test]
    fn writer_records_exact_text_without_delivery_or_baseline_and_rejects_collisions() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("actual Store");
        let command = request("render-1", "epoch-1");
        let before = store.snapshot().expect("snapshot").cursor;
        let receipt = store
            .record_context_render(command.clone())
            .expect("record");
        assert!(!receipt.duplicate && !receipt.authority_granted);
        let record = store
            .context_render(scope(), command.key.clone())
            .expect("get")
            .expect("record");
        assert_eq!(record.capsule, command.capsule);
        assert_eq!(record.rendered_bytes, record.capsule.text.len().to_string());
        assert_eq!(
            (&*record.byte_unit, &*record.token_unit),
            ("utf8_bytes", "estimated_tokens")
        );
        assert!(
            record.recorded
                && !record.delivered
                && !record.application_acknowledged
                && !record.authority_granted
        );
        assert!(
            store
                .context_baseline(scope())
                .expect("no baseline")
                .is_none()
        );
        assert!(
            store
                .record_context_render(command.clone())
                .expect("lost reply")
                .duplicate
        );
        context_error(
            store.record_context_render(request("render-1", "epoch-2")),
            |e| matches!(e, ContextRenderError::Collision),
        );
        let mut newer = command;
        newer.key.version = 2;
        store
            .record_context_render(newer)
            .expect("immutable new version");
        assert_eq!(
            store
                .context_renders_page(scope(), None, 8)
                .expect("page")
                .entries
                .len(),
            2
        );
        assert_eq!(
            store.snapshot().expect("snapshot").cursor,
            before,
            "context records do not fabricate task events"
        );
    }
    #[test]
    fn scope_and_independent_epoch_facts_bindings_fail_closed() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        let command = request("render-1", "epoch-1");
        store
            .record_context_render(command.clone())
            .expect("record");
        let mut foreign = scope();
        foreign.recipient = PrincipalId::new("manager-b").expect("recipient");
        assert!(
            store
                .context_render(foreign.clone(), command.key.clone())
                .expect("isolated get")
                .is_none()
        );
        assert!(
            store
                .context_renders_page(foreign.clone(), None, 1)
                .expect("isolated page")
                .entries
                .is_empty()
        );
        let mut wrong = command.clone();
        wrong.scope = foreign;
        assert!(
            store.record_context_render(wrong).is_ok(),
            "same facts may be separately recorded for authorized recipient scope"
        );
        for kind in 0..5 {
            let mut bad = command.clone();
            match kind {
                0 => bad.expected_epoch = "other-epoch".into(),
                1 => bad.expected_facts_digest = hash(&"different facts").expect("digest"),
                2 => bad.scope.role = RecipientRole::Admiral,
                3 => {
                    bad.scope.namespace = ContextNamespace::Project {
                        project_id: ProjectId::new("project-b").expect("project"),
                        root_id: RootId::new("root-a").expect("root"),
                    }
                }
                _ => {
                    bad.scope.namespace = ContextNamespace::Project {
                        project_id: ProjectId::new("project-a").expect("project"),
                        root_id: RootId::new("root-b").expect("root"),
                    }
                }
            }
            context_error(store.record_context_render(bad), |e| {
                matches!(e, ContextRenderError::Invalid)
            });
        }
        let mut wrong_handoff = command;
        let facts = HandoffFacts {
            from: PrincipalId::new("worker-a").expect("id"),
            to: PrincipalId::new("worker-b").expect("id"),
            attempt_id: admiralty_core::AttemptId::new("attempt-a").expect("id"),
            task: protected(),
            failed_approaches: vec![],
        };
        wrong_handoff.capsule = compile_handoff(&facts, "epoch-1", 16_384).expect("handoff");
        wrong_handoff.scope.role = RecipientRole::Worker;
        wrong_handoff.expected_facts_digest = wrong_handoff.capsule.facts_digest.clone();
        context_error(store.record_context_render(wrong_handoff.clone()), |e| {
            matches!(e, ContextRenderError::Invalid)
        });
        wrong_handoff.scope.recipient = facts.to;
        let handoff_key = wrong_handoff.key.clone();
        let handoff_scope = wrong_handoff.scope.clone();
        let handoff_text = wrong_handoff.capsule.text.clone();
        store
            .record_context_render(wrong_handoff)
            .expect("properly scoped worker handoff");
        assert_eq!(
            store
                .context_render(handoff_scope, handoff_key)
                .expect("get")
                .expect("handoff")
                .capsule
                .text,
            handoff_text
        );
        let decision = DecisionFacts {
            recipient: RecipientRole::Commodore,
            decision_id: "decision-a".into(),
            expected_revision: 1,
            task: protected(),
            blocking_requirement: "Choose interface".into(),
            requested_authority: "Owner decision required".into(),
            deadline: FactObservation::Unknown {
                reason: "No deadline observed".into(),
            },
            options: vec!["Keep 5 ms".into()],
            decision: DecisionObservation::Proposed {
                answer: "Keep 5 ms; no approval yet".into(),
            },
            work_that_can_proceed: vec!["Read scoped source".into()],
        };
        let capsule = compile_decision_capsule(&decision, "epoch-decision", 16_384)
            .expect("actual decision compiler");
        let decision_request = RecordContextRender {
            scope: scope(),
            key: RenderKey {
                render_id: "decision-a".into(),
                version: 1,
            },
            expected_epoch: "epoch-decision".into(),
            expected_facts_digest: capsule.facts_digest.clone(),
            capsule,
        };
        store
            .record_context_render(decision_request)
            .expect("project scoped decision facts, not decision authority");
    }
    #[test]
    fn fleet_namespace_is_explicit_canonical_and_checks_every_task_root() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        let facts = FleetExceptionFacts {
            revision: 1,
            authority_ref: "grant:fleet".into(),
            source_ref: "events:18".into(),
            project_scope: vec![project().project_id],
            projects: vec![project()],
            uncertainty: vec![],
            next_action: "Resolve scoped exception".into(),
        };
        let capsule = compile_fleet_exceptions(&facts, "epoch-fleet", 16_384).expect("fleet");
        let roots = vec![ProjectRoot {
            project_id: ProjectId::new("project-a").expect("project"),
            root_id: RootId::new("root-a").expect("root"),
        }];
        let mut command = RecordContextRender {
            scope: ContextScope {
                recipient: PrincipalId::new("admiral-a").expect("principal"),
                role: RecipientRole::Admiral,
                namespace: ContextNamespace::Fleet {
                    fleet_id: "fleet-a".into(),
                    project_roots: roots.clone(),
                },
            },
            key: RenderKey {
                render_id: "fleet-summary".into(),
                version: 1,
            },
            expected_epoch: "epoch-fleet".into(),
            expected_facts_digest: capsule.facts_digest.clone(),
            capsule,
        };
        store
            .record_context_render(command.clone())
            .expect("explicit fleet scope");
        for wrong_roots in [
            vec![],
            vec![roots[0].clone(), roots[0].clone()],
            vec![ProjectRoot {
                project_id: roots[0].project_id.clone(),
                root_id: RootId::new("wrong-root").expect("root"),
            }],
            vec![roots[0].clone(); 17],
        ] {
            command.scope.namespace = ContextNamespace::Fleet {
                fleet_id: "fleet-a".into(),
                project_roots: wrong_roots,
            };
            context_error(store.record_context_render(command.clone()), |e| {
                matches!(e, ContextRenderError::Invalid)
            });
        }
    }
    #[test]
    fn baseline_cas_and_historical_lost_ack_replay_never_rewind_or_change_command() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        let render1 = store
            .record_context_render(request("render-1", "epoch-1"))
            .expect("record")
            .render;
        let render2 = store
            .record_context_render(request("render-2", "epoch-2"))
            .expect("record")
            .render;
        let first_command = ack(render1, "ack-1", None);
        let first = store
            .advance_context_baseline(first_command.clone())
            .expect("first ACK");
        assert!(first.baseline.delivery_ref.is_none() && !first.baseline.authority_granted);
        let mut second_command = ack(render2, "ack-2", Some(first.baseline.cursor.clone()));
        second_command.delivery_ref = Some("transport:receipt-2".into());
        let second = store
            .advance_context_baseline(second_command)
            .expect("second ACK");
        let replay = store
            .advance_context_baseline(first_command.clone())
            .expect("exact historical retry");
        assert!(replay.duplicate);
        assert_eq!(replay.baseline, first.baseline);
        assert_eq!(
            store.context_baseline(scope()).expect("current"),
            Some(second.baseline)
        );
        for delivery in [false, true] {
            let mut collision = first_command.clone();
            if delivery {
                collision.delivery_ref = Some("new-transport".into())
            } else {
                collision.application_ack_ref = "different-ack".into()
            };
            context_error(store.advance_context_baseline(collision), |e| {
                matches!(e, ContextRenderError::Collision)
            });
        }
        let mut stale = first_command;
        stale.command_id = "new-command".into();
        context_error(store.advance_context_baseline(stale), |e| {
            matches!(e, ContextRenderError::StaleBaseline)
        });
    }
    #[test]
    fn baseline_requires_exact_record_ack_and_one_concurrent_cas_winner() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        let render = store
            .record_context_render(request("render-1", "epoch-1"))
            .expect("record")
            .render;
        let mut bad = ack(render.clone(), "ack-missing", None);
        bad.render.record_digest = hash(&"other record").expect("hash");
        context_error(store.advance_context_baseline(bad), |e| {
            matches!(e, ContextRenderError::MissingRender)
        });
        let mut bad = ack(render.clone(), "ack-empty", None);
        bad.application_ack_ref = " ".into();
        context_error(store.advance_context_baseline(bad), |e| {
            matches!(e, ContextRenderError::Invalid)
        });
        let results = std::thread::scope(|threads| {
            let a = store.clone();
            let b = store.clone();
            let r = render.clone();
            let first = threads.spawn(move || a.advance_context_baseline(ack(r, "ack-a", None)));
            let second =
                threads.spawn(move || b.advance_context_baseline(ack(render, "ack-b", None)));
            [first.join().expect("join"), second.join().expect("join")]
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(
                    r,
                    Err(StoreError::ContextRender(ContextRenderError::StaleBaseline))
                ))
                .count(),
            1
        );
    }
    #[test]
    fn persistence_backup_and_rebuild_retain_baseline_and_exact_render() {
        let dir = private_dir();
        let path = dir.path().join("state.sqlite");
        let backup = dir.path().join("backup.sqlite");
        let store = Store::open(&path).expect("Store");
        let command = request("render-1", "epoch-1");
        let render = store
            .record_context_render(command.clone())
            .expect("record")
            .render;
        let baseline = store
            .advance_context_baseline(ack(render, "ack-1", None))
            .expect("ACK")
            .baseline;
        store.backup(&backup).expect("actual SQLite backup");
        store.rebuild().expect("validated replay");
        drop(store);
        for file in [&path, &backup] {
            let reopened = Store::open(file).expect("reopen actual persisted database");
            assert_eq!(
                reopened
                    .context_render(scope(), command.key.clone())
                    .expect("get")
                    .expect("record")
                    .capsule
                    .text,
                command.capsule.text
            );
            assert_eq!(
                reopened.context_baseline(scope()).expect("baseline"),
                Some(baseline.clone())
            );
        }
    }
    #[test]
    fn bounded_keyset_pages_preserve_history_and_cursors_do_not_cross_scope() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        for id in ["render-10", "render-2", "render-3"] {
            store
                .record_context_render(request(id, "epoch-1"))
                .expect("record");
        }
        let first = store
            .context_renders_page(scope(), None, 2)
            .expect("first page");
        assert_eq!(
            first
                .entries
                .iter()
                .map(|r| r.key.render_id.as_str())
                .collect::<Vec<_>>(),
            vec!["render-10", "render-2"]
        );
        let mut foreign = scope();
        foreign.recipient = PrincipalId::new("other-manager").expect("id");
        assert!(
            store
                .context_renders_page(foreign, first.next.clone(), 2)
                .is_err()
        );
        let last = store
            .context_renders_page(scope(), first.next, 2)
            .expect("remaining page");
        assert_eq!(last.entries[0].key.render_id, "render-3");
        assert!(last.next.is_none());
        assert!(store.context_renders_page(scope(), None, 0).is_err());
        assert!(store.context_renders_page(scope(), None, 9).is_err());
        for n in 3..MAX_RENDERS {
            store
                .record_context_render(request(&format!("capacity-{n}"), "epoch-1"))
                .expect("up to cap");
        }
        context_error(
            store.record_context_render(request("over-capacity", "epoch-1")),
            |e| matches!(e, ContextRenderError::Capacity),
        );
        assert!(
            store
                .record_context_render(request("render-10", "epoch-1"))
                .expect("replay remains valid at cap")
                .duplicate
        );
        assert!(
            store
                .context_render(scope(), request("render-10", "epoch-1").key)
                .expect("original retained")
                .is_some()
        );
    }
    #[test]
    fn page_byte_cap_and_malformed_or_oversized_text_are_enforced() {
        let dir = private_dir();
        let store = Store::open(dir.path().join("state.sqlite")).expect("Store");
        for n in 0..5 {
            let mut large = request(&format!("large-{n}"), "epoch-1");
            large
                .capsule
                .text
                .push_str(&" ".repeat(MAX_ROLE_CAPSULE_BYTES - large.capsule.text.len()));
            large.capsule.rendered_digest =
                format!("sha256:{:x}", Sha256::digest(large.capsule.text.as_bytes()));
            large.capsule.estimated_tokens = large.capsule.text.len().div_ceil(4);
            store
                .record_context_render(large)
                .expect("valid bounded JSON whitespace retained exactly");
        }
        let page = store
            .context_renders_page(scope(), None, 8)
            .expect("bounded page");
        assert!(page.entries.len() < 5 && page.next.is_some());
        assert!(serde_json::to_vec(&page).expect("JSON").len() <= MAX_PAGE_BYTES);
        for kind in 0..4 {
            let mut bad = request("bad", "epoch-1");
            match kind {
                0 => bad.capsule.text = "x".repeat(MAX_ROLE_CAPSULE_BYTES + 1),
                1 => bad.capsule.estimated_tokens += 1,
                2 => bad.capsule.tokenizer = "actual_provider_tokens".into(),
                _ => bad.capsule.text = "{broken".into(),
            }
            if kind == 3 {
                bad.capsule.rendered_digest =
                    format!("sha256:{:x}", Sha256::digest(bad.capsule.text.as_bytes()));
                bad.capsule.estimated_tokens = bad.capsule.text.len().div_ceil(4);
            }
            context_error(store.record_context_render(bad), |e| {
                matches!(e, ContextRenderError::Invalid)
            });
        }
        let mut forged = request("forged-authority", "epoch-1");
        let mut document: serde_json::Value =
            serde_json::from_str(&forged.capsule.text).expect("actual document");
        document["authority_granted"] = serde_json::Value::Bool(true);
        forged.capsule.text = serde_json::to_string(&document).expect("JSON");
        forged.capsule.rendered_digest = format!(
            "sha256:{:x}",
            Sha256::digest(forged.capsule.text.as_bytes())
        );
        forged.capsule.estimated_tokens = forged.capsule.text.len().div_ceil(4);
        context_error(store.record_context_render(forged), |e| {
            matches!(e, ContextRenderError::Invalid)
        });
    }
    #[test]
    fn failed_pointer_write_rolls_back_command_and_can_be_retried() {
        let dir = private_dir();
        let path = dir.path().join("state.sqlite");
        let store = Store::open(&path).expect("Store");
        let render = store
            .record_context_render(request("render-1", "epoch-1"))
            .expect("record")
            .render;
        let fault = Connection::open(&path).expect("owned fault injection");
        fault.execute_batch("CREATE TRIGGER context_test_abort BEFORE INSERT ON context_render_baselines BEGIN SELECT RAISE(ABORT,'injected pointer failure'); END;").expect("fault");
        let command = ack(render, "ack-1", None);
        assert!(store.advance_context_baseline(command.clone()).is_err());
        assert_eq!(
            fault
                .query_row("SELECT count(*) FROM context_render_commands", [], |r| r
                    .get::<_, i64>(0))
                .expect("count"),
            0
        );
        assert!(
            store
                .context_baseline(scope())
                .expect("unchanged")
                .is_none()
        );
        fault
            .execute_batch("DROP TRIGGER context_test_abort")
            .expect("remove owned fault");
        assert!(
            !store
                .advance_context_baseline(command)
                .expect("retry after rollback")
                .duplicate
        );
    }
    #[test]
    fn live_corruption_blocks_reads_backup_and_rebuild_before_effects() {
        let dir = private_dir();
        let path = dir.path().join("state.sqlite");
        let backup = dir.path().join("must-not-publish.sqlite");
        let store = Store::open(&path).expect("Store");
        let command = request("render-1", "epoch-1");
        store
            .record_context_render(command.clone())
            .expect("record");
        let cursor = store.snapshot().expect("snapshot").cursor;
        let fault = Connection::open(&path).expect("owned corruption fixture");
        fault
            .execute_batch("DROP TRIGGER context_render_immutable_update")
            .expect("fault");
        fault
            .execute(
                "UPDATE context_render_records SET record_digest=?1",
                [hash(&"different content").expect("hash")],
            )
            .expect("hash corruption");
        assert!(store.context_render(scope(), command.key).is_err());
        assert!(store.backup(&backup).is_err());
        assert!(!backup.exists(), "corrupt backup must not be published");
        assert!(store.rebuild().is_err());
        assert_eq!(
            store.snapshot().expect("unchanged domain cursor").cursor,
            cursor
        );
    }
    #[test]
    fn corrupt_text_and_rewound_baseline_fail_closed_on_reopen() {
        for rewind in [false, true] {
            let dir = private_dir();
            let path = dir.path().join("state.sqlite");
            let store = Store::open(&path).expect("Store");
            let render = store
                .record_context_render(request("render-1", "epoch-1"))
                .expect("record")
                .render;
            let first = store
                .advance_context_baseline(ack(render.clone(), "ack-1", None))
                .expect("first");
            store
                .advance_context_baseline(ack(render, "ack-2", Some(first.baseline.cursor)))
                .expect("second");
            drop(store);
            let fault = Connection::open(&path).expect("owned corruption fixture");
            if rewind {
                fault
                    .execute("UPDATE context_render_baselines SET current_sequence=1", [])
                    .expect("rewind");
            } else {
                fault.execute_batch("DROP TRIGGER context_render_immutable_update; PRAGMA ignore_check_constraints=ON;").expect("controlled corruption");
                fault
                    .execute(
                        "UPDATE context_render_records SET record_json=?1",
                        ["x".repeat(MAX_RECORD_BYTES + 1)],
                    )
                    .expect("oversized stored column");
            }
            drop(fault);
            assert!(
                Store::open(&path).is_err(),
                "corruption must not be normalized or accepted"
            );
        }
    }
}
