//! Immutable provider-REPORTED native counters on the Store's sole writer.
//!
//! This journal performs no normalization, model qualification, valuation,
//! reservation, settlement, delivery, or execution. Proofs and installed current
//! policies are process-local; serialized provenance is only a recorded claim.
use admiralty_core::{
    ProjectId, RootId, SourceIdentity, TaskContract, TaskId, TaskView,
    economics::{DecimalU64, ObservedAmount, ParentInclusion},
    limits::bounded_json_size,
};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{any::Any, collections::BTreeMap, sync::Arc};

pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_CONTRACT_BYTES: usize = 64 * 1024;
const MAX_RECEIPT_BYTES: usize = 8192;
const MAX_ORIGINS: i64 = 1024;
const MAX_REVISIONS: i64 = 8192;
const MAX_TOTAL_BYTES: i64 = 64 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 256 * 1024;
pub type UsageProof = Arc<dyn Any + Send + Sync>;
pub type Result<T> = std::result::Result<T, UsageError>;
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("usage SQLite failure: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("usage JSON failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid or oversized native usage input")]
    Invalid,
    #[error("native usage access is not currently authorized")]
    Denied,
    #[error("native usage immutable identity collision")]
    Collision,
    #[error("native usage revision compare-and-swap failed")]
    Stale,
    #[error("native usage bounded storage is full; nothing evicted")]
    Capacity,
    #[error("native usage journal integrity failure")]
    Integrity,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
}
/// Reported is NOT independently verified, requested, or authenticated identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReportedFact<T> {
    Reported { value: T },
    Unknown { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CounterSemantics {
    Incremental,
    Cumulative { opening: ObservedAmount },
    Unknown { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeCounter {
    pub category: String,
    pub unit: ReportedFact<String>,
    pub semantics: CounterSemantics,
    /// Even core `Known` means provider-reported here, never verified billing.
    pub amount: ObservedAmount,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeUsageObservation {
    pub provider: ReportedFact<String>,
    pub model: ReportedFact<String>,
    pub effort: ReportedFact<String>,
    pub session: ReportedFact<String>,
    pub request: ReportedFact<String>,
    pub turn: ReportedFact<String>,
    pub event: ReportedFact<String>,
    pub attempt: ReportedFact<String>,
    pub segment: ReportedFact<String>,
    pub role: ReportedFact<String>,
    pub purpose: ReportedFact<String>,
    /// Provider-native timestamp text; no host-time or timezone inference.
    pub time: ReportedFact<String>,
    pub inclusion: ReportedFact<ParentInclusion>,
    pub counters: Vec<NativeCounter>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionRef {
    pub revision: DecimalU64,
    pub receipt_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageCommand {
    pub schema_version: u32,
    pub scope: UsageScope,
    pub command_id: String,
    /// Must belong to a separately authenticated usage-source namespace.
    pub source: SourceIdentity,
    pub actor: String,
    pub policy_digest: String,
    pub capture_ref: String,
    pub capture_digest: String,
    pub observation_id: String,
    pub revision: DecimalU64,
    pub parent: Option<RevisionRef>,
    pub correction_reason: Option<String>,
    pub observation: NativeUsageObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageReceipt {
    pub command_id: String,
    pub observation_id: String,
    pub revision: DecimalU64,
    pub scope_sequence: DecimalU64,
    pub command_digest: String,
    pub original_contract_digest: String,
    pub parent: Option<RevisionRef>,
    pub execution_granted: bool,
    pub model_qualified: bool,
    pub settled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordedUsage {
    pub command: UsageCommand,
    pub receipt: UsageReceipt,
    pub receipt_digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageResult {
    pub receipt: UsageReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageCursor {
    pub scope_digest: String,
    pub after_sequence: DecimalU64,
    pub head_sequence: DecimalU64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageRead {
    pub scope: UsageScope,
    pub actor: String,
    pub limit: u32,
    pub cursor: Option<UsageCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsagePage {
    pub records: Vec<RecordedUsage>,
    pub next: Option<UsageCursor>,
    pub head_sequence: DecimalU64,
}
/// All task/contract arguments come from Store custody. Callbacks must be
/// bounded, nonblocking, non-reentrant, and perform no I/O. No JSON issuer.
pub struct UsageWriteBinding<'a> {
    pub command: &'a UsageCommand,
    pub command_digest: &'a str,
    pub original_contract: &'a TaskContract,
    pub current_task: &'a TaskView,
    pub previous: Option<&'a RecordedUsage>,
}
pub struct UsageReadBinding<'a> {
    pub request: &'a UsageRead,
    pub current_task: &'a TaskView,
}
pub trait UsageAuthority: Send + Sync {
    fn authorize_write(
        &self,
        _binding: &UsageWriteBinding<'_>,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
        false
    }
    fn authorize_read(
        &self,
        _binding: &UsageReadBinding<'_>,
        _proof: &(dyn Any + Send + Sync),
    ) -> bool {
        false
    }
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn hash<T: Serialize + ?Sized>(domain: &str, value: &T, max: usize) -> Result<String> {
    bounded_json_size(value, max).map_err(|_| UsageError::Invalid)?;
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0]);
    h.update(serde_json::to_vec(value)?);
    Ok(format!("sha256:{:x}", h.finalize()))
}
fn scope_valid(s: &UsageScope) -> bool {
    id(s.project_id.as_str())
        && id(s.root_id.as_str())
        && id(s.task_id.as_str())
        && s.contract_revision > 0
        && digest(&s.contract_digest)
}
fn fact<T>(f: &ReportedFact<T>, valid: impl FnOnce(&T) -> bool) -> bool {
    match f {
        ReportedFact::Reported { value } => valid(value),
        ReportedFact::Unknown { reason } => text(reason, 512),
    }
}
fn amount(a: &ObservedAmount) -> bool {
    match a {
        ObservedAmount::Unknown { reason } => text(reason, 512),
        _ => true,
    }
}
fn observation_valid(o: &NativeUsageObservation) -> bool {
    let facts = [
        &o.provider,
        &o.model,
        &o.effort,
        &o.session,
        &o.request,
        &o.turn,
        &o.event,
        &o.attempt,
        &o.segment,
        &o.role,
        &o.purpose,
        &o.time,
    ];
    if !facts.iter().all(|f| fact(f, |s| text(s, 512)))
        || !fact(&o.inclusion, |p| match p {
            ParentInclusion::Interval { start, end } => start.0 <= end.0,
            _ => true,
        })
        || o.counters.is_empty()
        || o.counters.len() > 8
    {
        return false;
    }
    let mut previous = "";
    for c in &o.counters {
        if !id(&c.category)
            || c.category.as_str() <= previous
            || !fact(&c.unit, |s| id(s))
            || !amount(&c.amount)
            || !match &c.semantics {
                CounterSemantics::Incremental => true,
                CounterSemantics::Cumulative { opening } => amount(opening),
                CounterSemantics::Unknown { reason } => text(reason, 512),
            }
        {
            return false;
        }
        previous = &c.category;
    }
    true
}
pub fn validate_command(c: &UsageCommand) -> Result<String> {
    bounded_json_size(c, MAX_COMMAND_BYTES).map_err(|_| UsageError::Invalid)?;
    if c.schema_version != 1
        || !scope_valid(&c.scope)
        || !id(&c.command_id)
        || !id(&c.observation_id)
        || !id(&c.actor)
        || !id(&c.source.producer_id)
        || !id(&c.source.incarnation)
        || c.source.validate().is_err()
        || !digest(&c.policy_digest)
        || !digest(&c.capture_digest)
        || !text(&c.capture_ref, 512)
        || c.capture_ref.chars().any(char::is_whitespace)
        || c.revision.0 == 0
        || c.revision.0 > 32
        || !observation_valid(&c.observation)
    {
        return Err(UsageError::Invalid);
    }
    match (&c.parent, &c.correction_reason, c.revision.0) {
        (None, None, 1) => {}
        (Some(p), Some(reason), n)
            if n > 1
                && p.revision.0.checked_add(1) == Some(n)
                && digest(&p.receipt_digest)
                && text(reason, 512) => {}
        _ => return Err(UsageError::Invalid),
    }
    hash("admiralty.native-usage.command/v1", c, MAX_COMMAND_BYTES)
}
pub fn validate_read(r: &UsageRead) -> Result<()> {
    bounded_json_size(r, 4096).map_err(|_| UsageError::Invalid)?;
    if !scope_valid(&r.scope) || !id(&r.actor) || r.limit == 0 || r.limit > 32 {
        return Err(UsageError::Invalid);
    }
    if let Some(c) = &r.cursor
        && (!digest(&c.scope_digest)
            || c.after_sequence.0 > c.head_sequence.0
            || c.head_sequence.0 > MAX_REVISIONS as u64)
    {
        return Err(UsageError::Invalid);
    }
    Ok(())
}
fn scope_key(s: &UsageScope) -> Result<String> {
    hash("admiralty.native-usage.scope/v1", s, 4096)
}
fn identity(o: &NativeUsageObservation) -> Result<String> {
    // Amount-only corrections preserve all categories, native units, semantic
    // baselines, and reported/unknown metadata byte for byte.
    let mut v = o.clone();
    for c in &mut v.counters {
        c.amount = ObservedAmount::Unknown {
            reason: "identity-excludes-current-amount".into(),
        };
    }
    hash("admiralty.native-usage.identity/v1", &v, MAX_COMMAND_BYTES)
}
fn contract_matches(c: &TaskContract, s: &UsageScope) -> Result<bool> {
    Ok(c.project_id == s.project_id
        && c.root_id == s.root_id
        && c.task_id == s.task_id
        && c.revision == s.contract_revision
        && admiralty_core::content_digest(c).map_err(|_| UsageError::Integrity)?
            == s.contract_digest)
}
fn current_matches(t: &TaskView, s: &UsageScope) -> bool {
    t.contract.project_id == s.project_id
        && t.contract.root_id == s.root_id
        && t.contract.task_id == s.task_id
}
#[derive(Clone)]
struct Origin {
    scope: UsageScope,
    contract: TaskContract,
    identity: String,
}
struct History {
    origins: BTreeMap<(String, String), Origin>,
    rows: Vec<RecordedUsage>,
}
pub(crate) fn initialize(c: &Connection) -> Result<()> {
    c.execute_batch(include_str!("../migrations/0008_usage_observations.sql"))?;
    validate_all(c)
}
fn capacity(c: &Connection) -> Result<(i64, i64, i64)> {
    let (origins,ob): (i64,i64)=c.query_row("SELECT count(*),coalesce(sum(length(CAST(scope_key AS BLOB))+length(CAST(observation_id AS BLOB))+length(CAST(scope_json AS BLOB))+length(CAST(contract_json AS BLOB))+length(CAST(identity_digest AS BLOB))),0) FROM usage_observation_origins",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let (rows,rb): (i64,i64)=c.query_row("SELECT count(*),coalesce(sum(length(CAST(scope_key AS BLOB))+length(CAST(observation_id AS BLOB))+length(CAST(command_id AS BLOB))+length(CAST(producer_id AS BLOB))+length(CAST(incarnation AS BLOB))+length(CAST(source_sequence AS BLOB))+length(CAST(command_json AS BLOB))+length(CAST(command_digest AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(receipt_digest AS BLOB))),0) FROM usage_observation_revisions",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let bytes = ob.checked_add(rb).ok_or(UsageError::Capacity)?;
    if origins > MAX_ORIGINS || rows > MAX_REVISIONS || bytes > MAX_TOTAL_BYTES {
        return Err(UsageError::Capacity);
    }
    Ok((origins, rows, bytes))
}
fn decode<T: serde::de::DeserializeOwned + Serialize>(s: &str) -> Result<T> {
    let v: T = serde_json::from_str(s).map_err(|_| UsageError::Integrity)?;
    if serde_json::to_string(&v)? != s {
        return Err(UsageError::Integrity);
    }
    Ok(v)
}
fn history(c: &Connection, key: Option<&str>) -> Result<History> {
    capacity(c)?;
    let mut h = History {
        origins: BTreeMap::new(),
        rows: Vec::new(),
    };
    let mut stmt=c.prepare("SELECT scope_key,observation_id,scope_json,contract_json,identity_digest FROM usage_observation_origins WHERE (?1 IS NULL OR scope_key=?1) ORDER BY scope_key,observation_id")?;
    let mut rows = stmt.query([key])?;
    while let Some(r) = rows.next()? {
        let k = crate::stored_text(r, 0, 71)?;
        let obs = crate::stored_text(r, 1, 128)?;
        let scope: UsageScope = decode(&crate::stored_text(r, 2, 4096)?)?;
        let contract: TaskContract = decode(&crate::stored_text(r, 3, MAX_CONTRACT_BYTES)?)?;
        let ident = crate::stored_text(r, 4, 71)?;
        if !scope_valid(&scope)
            || !id(&obs)
            || scope_key(&scope)? != k
            || !digest(&ident)
            || contract.validate().is_err()
            || !contract_matches(&contract, &scope)?
        {
            return Err(UsageError::Integrity);
        }
        h.origins.insert(
            (k, obs),
            Origin {
                scope,
                contract,
                identity: ident,
            },
        );
    }
    let mut stmt=c.prepare("SELECT scope_key,observation_id,revision,scope_sequence,command_id,producer_id,incarnation,source_sequence,command_json,command_digest,receipt_json,receipt_digest FROM usage_observation_revisions WHERE (?1 IS NULL OR scope_key=?1) ORDER BY scope_key,scope_sequence")?;
    let mut rows = stmt.query([key])?;
    let mut heads: BTreeMap<(String, String), RecordedUsage> = BTreeMap::new();
    let mut sequences: BTreeMap<String, u64> = BTreeMap::new();
    while let Some(r) = rows.next()? {
        let k = crate::stored_text(r, 0, 71)?;
        let obs = crate::stored_text(r, 1, 128)?;
        let revision: i64 = r.get(2)?;
        let seq: i64 = r.get(3)?;
        let cmdid = crate::stored_text(r, 4, 128)?;
        let producer = crate::stored_text(r, 5, 128)?;
        let incarnation = crate::stored_text(r, 6, 128)?;
        let source_seq = crate::stored_text(r, 7, 20)?;
        let command: UsageCommand = decode(&crate::stored_text(r, 8, MAX_COMMAND_BYTES)?)?;
        let cd = crate::stored_text(r, 9, 71)?;
        let receipt: UsageReceipt = decode(&crate::stored_text(r, 10, MAX_RECEIPT_BYTES)?)?;
        let rd = crate::stored_text(r, 11, 71)?;
        let o = h
            .origins
            .get(&(k.clone(), obs.clone()))
            .ok_or(UsageError::Integrity)?;
        let previous = heads.get(&(k.clone(), obs.clone()));
        let expected_parent = previous.map(|p| RevisionRef {
            revision: p.command.revision.clone(),
            receipt_digest: p.receipt_digest.clone(),
        });
        let next_seq = sequences.get(&k).copied().unwrap_or(0) + 1;
        if command.scope != o.scope
            || scope_key(&command.scope)? != k
            || command.observation_id != obs
            || command.command_id != cmdid
            || command.source.producer_id != producer
            || command.source.incarnation != incarnation
            || command.source.sequence != source_seq
            || revision <= 0
            || command.revision.0 != revision as u64
            || seq <= 0
            || seq as u64 != next_seq
            || validate_command(&command).map_err(|_| UsageError::Integrity)? != cd
            || identity(&command.observation)? != o.identity
            || command.parent != expected_parent
            || command.revision.0 != previous.map_or(1, |p| p.command.revision.0 + 1)
            || receipt != make_receipt(&command, &cd, next_seq)
            || hash(
                "admiralty.native-usage.receipt/v1",
                &receipt,
                MAX_RECEIPT_BYTES,
            )? != rd
        {
            return Err(UsageError::Integrity);
        }
        let record = RecordedUsage {
            command,
            receipt,
            receipt_digest: rd,
        };
        heads.insert((k.clone(), obs), record.clone());
        sequences.insert(k, next_seq);
        h.rows.push(record);
    }
    if h.origins.len() != heads.len() {
        return Err(UsageError::Integrity);
    }
    Ok(h)
}
pub(crate) fn validate_all(c: &Connection) -> Result<()> {
    history(c, None).map(|_| ())
}
fn make_receipt(c: &UsageCommand, d: &str, sequence: u64) -> UsageReceipt {
    UsageReceipt {
        command_id: c.command_id.clone(),
        observation_id: c.observation_id.clone(),
        revision: c.revision.clone(),
        scope_sequence: DecimalU64(sequence),
        command_digest: d.into(),
        original_contract_digest: c.scope.contract_digest.clone(),
        parent: c.parent.clone(),
        execution_granted: false,
        model_qualified: false,
        settled: false,
    }
}
/// Caller must be the existing sole writer. No connection/authority is opened
/// here. Current policy runs before duplicate handling and at final commit.
pub(crate) fn record_authorized(
    c: &mut Connection,
    command: &UsageCommand,
    authority: Option<&dyn UsageAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<UsageResult> {
    let cd = validate_command(command)?;
    let task = task
        .filter(|t| current_matches(t, &command.scope))
        .ok_or(UsageError::Denied)?;
    let authority = authority.ok_or(UsageError::Denied)?;
    let key = scope_key(&command.scope)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let h = history(&tx, Some(&key))?;
    let origin = h
        .origins
        .get(&(key.clone(), command.observation_id.clone()));
    let original = match origin {
        Some(o) => &o.contract,
        None => {
            if !contract_matches(&task.contract, &command.scope)? {
                return Err(UsageError::Denied);
            }
            &task.contract
        }
    };
    let previous = h
        .rows
        .iter()
        .rev()
        .find(|r| r.command.observation_id == command.observation_id);
    let binding = UsageWriteBinding {
        command,
        command_digest: &cd,
        original_contract: original,
        current_task: task,
        previous,
    };
    if !authority.authorize_write(&binding, proof) {
        return Err(UsageError::Denied);
    }
    let mut stmt=tx.prepare("SELECT scope_key,command_id,producer_id,incarnation,source_sequence,command_digest FROM usage_observation_revisions WHERE (scope_key=?1 AND command_id=?2) OR (producer_id=?3 AND incarnation=?4 AND source_sequence=?5)")?;
    let mut rows = stmt.query(params![
        key,
        command.command_id,
        command.source.producer_id,
        command.source.incarnation,
        command.source.sequence
    ])?;
    let mut collision = false;
    let mut found = false;
    while let Some(r) = rows.next()? {
        let k = crate::stored_text(r, 0, 71)?;
        let id = crate::stored_text(r, 1, 128)?;
        let producer = crate::stored_text(r, 2, 128)?;
        let incarnation = crate::stored_text(r, 3, 128)?;
        let sequence = crate::stored_text(r, 4, 20)?;
        let d = crate::stored_text(r, 5, 71)?;
        found = true;
        if k != key
            || id != command.command_id
            || producer != command.source.producer_id
            || incarnation != command.source.incarnation
            || sequence != command.source.sequence
            || d != cd
        {
            collision = true;
        }
    }
    drop(rows);
    drop(stmt);
    if collision {
        return Err(UsageError::Collision);
    }
    if found {
        let r = h
            .rows
            .iter()
            .find(|r| r.command.command_id == command.command_id && r.command == *command)
            .ok_or(UsageError::Integrity)?;
        if !authority.authorize_write(&binding, proof) {
            return Err(UsageError::Denied);
        }
        return Ok(UsageResult {
            receipt: r.receipt.clone(),
            receipt_digest: r.receipt_digest.clone(),
            replayed: true,
        });
    }
    if h.rows.iter().any(|r| {
        r.command.observation_id == command.observation_id && r.command.revision == command.revision
    }) {
        return Err(UsageError::Collision);
    }
    let parent = previous.map(|p| RevisionRef {
        revision: p.command.revision.clone(),
        receipt_digest: p.receipt_digest.clone(),
    });
    if command.parent != parent
        || command.revision.0 != previous.map_or(1, |p| p.command.revision.0 + 1)
    {
        return Err(UsageError::Stale);
    }
    if let Some(o) = origin
        && o.identity != identity(&command.observation)?
    {
        return Err(UsageError::Collision);
    }
    let (origin_count, row_count, bytes) = capacity(&tx)?;
    if row_count >= MAX_REVISIONS || (origin.is_none() && origin_count >= MAX_ORIGINS) {
        return Err(UsageError::Capacity);
    }
    let seq = h.rows.last().map_or(1, |r| r.receipt.scope_sequence.0 + 1);
    let receipt = make_receipt(command, &cd, seq);
    let rd = hash(
        "admiralty.native-usage.receipt/v1",
        &receipt,
        MAX_RECEIPT_BYTES,
    )?;
    bounded_json_size(original, MAX_CONTRACT_BYTES).map_err(|_| UsageError::Invalid)?;
    let cj = serde_json::to_string(command)?;
    let rj = serde_json::to_string(&receipt)?;
    let sj = serde_json::to_string(&command.scope)?;
    let ct = serde_json::to_string(original)?;
    let ident = identity(&command.observation)?;
    let mut extra = key.len()
        + command.observation_id.len()
        + command.command_id.len()
        + command.source.producer_id.len()
        + command.source.incarnation.len()
        + command.source.sequence.len()
        + cj.len()
        + cd.len()
        + rj.len()
        + rd.len();
    if origin.is_none() {
        extra += key.len() + command.observation_id.len() + sj.len() + ct.len() + ident.len();
    }
    if bytes
        .checked_add(extra as i64)
        .is_none_or(|n| n > MAX_TOTAL_BYTES)
    {
        return Err(UsageError::Capacity);
    }
    if origin.is_none() {
        bounded_json_size(original, MAX_CONTRACT_BYTES).map_err(|_| UsageError::Invalid)?;
        tx.execute(
            "INSERT INTO usage_observation_origins VALUES(?1,?2,?3,?4,?5)",
            params![key, command.observation_id, sj, ct, ident],
        )?;
    }
    tx.execute(
        "INSERT INTO usage_observation_revisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            key,
            command.observation_id,
            command.revision.0 as i64,
            seq as i64,
            command.command_id,
            command.source.producer_id,
            command.source.incarnation,
            command.source.sequence,
            cj,
            cd,
            rj,
            rd
        ],
    )?;
    if !authority.authorize_write(&binding, proof) {
        return Err(UsageError::Denied);
    }
    tx.commit()?;
    Ok(UsageResult {
        receipt,
        receipt_digest: rd,
        replayed: false,
    })
}
pub(crate) fn read_authorized(
    c: &mut Connection,
    r: &UsageRead,
    authority: Option<&dyn UsageAuthority>,
    proof: &(dyn Any + Send + Sync),
    task: Option<&TaskView>,
) -> Result<UsagePage> {
    validate_read(r)?;
    let task = task
        .filter(|t| current_matches(t, &r.scope))
        .ok_or(UsageError::Denied)?;
    let authority = authority.ok_or(UsageError::Denied)?;
    let b = UsageReadBinding {
        request: r,
        current_task: task,
    };
    if !authority.authorize_read(&b, proof) {
        return Err(UsageError::Denied);
    }
    let key = scope_key(&r.scope)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let h = history(&tx, Some(&key))?;
    let actual = h.rows.last().map_or(0, |r| r.receipt.scope_sequence.0);
    let (after, head) = match &r.cursor {
        None => (0, actual),
        Some(v) if v.scope_digest == key && v.head_sequence.0 <= actual => {
            (v.after_sequence.0, v.head_sequence.0)
        }
        _ => return Err(UsageError::Invalid),
    };
    let mut records = Vec::new();
    let mut page_bytes = 1024usize;
    for v in h
        .rows
        .into_iter()
        .filter(|v| v.receipt.scope_sequence.0 > after && v.receipt.scope_sequence.0 <= head)
        .take(r.limit as usize)
    {
        let bytes = bounded_json_size(&v, MAX_COMMAND_BYTES + MAX_RECEIPT_BYTES + 1024)
            .map_err(|_| UsageError::Integrity)?;
        if page_bytes
            .checked_add(bytes)
            .is_none_or(|n| n > MAX_PAGE_BYTES)
        {
            break;
        }
        page_bytes += bytes;
        records.push(v);
    }
    let end = records.last().map_or(after, |v| v.receipt.scope_sequence.0);
    let next = (end < head).then_some(UsageCursor {
        scope_digest: key,
        after_sequence: DecimalU64(end),
        head_sequence: DecimalU64(head),
    });
    let page = UsagePage {
        records,
        next,
        head_sequence: DecimalU64(head),
    };
    bounded_json_size(&page, MAX_PAGE_BYTES).map_err(|_| UsageError::Capacity)?;
    if !authority.authorize_read(&b, proof) {
        return Err(UsageError::Denied);
    }
    tx.commit()?;
    Ok(page)
}
