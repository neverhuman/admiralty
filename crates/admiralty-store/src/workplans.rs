//! Explicit, offline registration of immutable JSON workplan revisions.
//! No task events, launch, native qualification or acceptance are produced.
use crate::stored_text;
use admiralty_core::{
    ProjectId, TaskContract, TaskId, content_digest,
    limits::bounded_json_size,
    workplans::{
        MAX_WORKPLAN_BYTES, Workplan, WorkplanError, WorkplanPreview, preview_workplan,
        preview_workplan_with_existing,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PLANS: usize = 256;
pub const MAX_REVISIONS_PER_PLAN: usize = 64;
pub const MAX_REGISTERED_TASKS: usize = 20_000;
pub const MAX_LIST_PAGE: usize = 100;
pub const MAX_RECEIPT_BYTES: usize = 512 * 1024;
pub const MAX_REGISTRY_BYTES: usize = 64 * 1024 * 1024;
const MAX_BUSY_MILLIS: i64 = 5000;

pub type Result<T> = std::result::Result<T, WorkplanStoreError>;
#[derive(Debug, thiserror::Error)]
pub enum WorkplanStoreError {
    #[error("workplan SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("workplan JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("workplan preflight rejected input: {0}")]
    Parser(#[from] WorkplanError),
    #[error("invalid workplan registry input: {0}")]
    Invalid(&'static str),
    #[error("workplan registry byte bound exceeded")]
    TooLarge,
    #[error("workplan project/plan binding does not match source")]
    ScopeMismatch,
    #[error("expected current workplan revision does not match")]
    StaleCurrent,
    #[error("fresh workplan revision must advance exactly one")]
    RevisionSequence,
    #[error("workplan command ID reused with different scope/content")]
    IdentityCollision,
    #[error("workplan registry integrity failed: {0}")]
    Integrity(&'static str),
    #[error("workplan registry capacity is full; no history was evicted")]
    Capacity,
    #[error("workplan registry schema/connection assumptions do not hold")]
    Schema,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterWorkplanRequest {
    pub command_id: String,
    pub project_id: ProjectId,
    pub plan_id: String,
    pub expected_current_revision: String,
    pub source: Vec<u8>,
    pub source_digest: String,
    pub canonical_digest: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct WorkplanReceipt {
    pub command_id: String,
    pub project_id: ProjectId,
    pub plan_id: String,
    pub revision: String,
    pub previous_revision: String,
    pub source_digest: String,
    pub canonical_digest: String,
    pub task_order: Vec<TaskId>,
    pub task_digests: BTreeMap<TaskId, String>,
    recorded_only: bool,
    native_execution_qualified: bool,
    task_events_emitted: bool,
    execution_granted: bool,
    acceptance_granted: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RegisteredWorkplan {
    pub plan: Workplan,
    pub receipt: WorkplanReceipt,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct WorkplanSummary {
    pub project_id: ProjectId,
    pub plan_id: String,
    pub current_revision: String,
    pub source_digest: String,
    pub canonical_digest: String,
    pub task_count: usize,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct WorkplanPage {
    pub entries: Vec<WorkplanSummary>,
    pub next_after: Option<String>,
    recorded_only: bool,
    native_execution_qualified: bool,
    task_events_emitted: bool,
    execution_granted: bool,
    acceptance_granted: bool,
}

const SCHEMA: &[(&str, &str, &str)] = &[
    ("table", "workplan_registry_plans", "CREATE TABLE IF NOT EXISTS main.workplan_registry_plans (
        project_id TEXT NOT NULL,plan_id TEXT NOT NULL,current_revision INTEGER NOT NULL CHECK(current_revision BETWEEN 1 AND 64),
        PRIMARY KEY(project_id,plan_id), FOREIGN KEY(project_id,plan_id,current_revision)
        REFERENCES workplan_registry_revisions(project_id,plan_id,revision) DEFERRABLE INITIALLY DEFERRED) STRICT"),
    ("table", "workplan_registry_revisions", "CREATE TABLE IF NOT EXISTS main.workplan_registry_revisions (
        project_id TEXT NOT NULL,plan_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 64),
        command_id TEXT NOT NULL UNIQUE,request_digest TEXT NOT NULL,source_digest TEXT NOT NULL,canonical_digest TEXT NOT NULL,
        source_json TEXT NOT NULL,canonical_json TEXT NOT NULL,receipt_json TEXT NOT NULL,
        PRIMARY KEY(project_id,plan_id,revision), FOREIGN KEY(project_id,plan_id)
        REFERENCES workplan_registry_plans(project_id,plan_id) DEFERRABLE INITIALLY DEFERRED) STRICT"),
    ("table", "workplan_registry_tasks", "CREATE TABLE IF NOT EXISTS main.workplan_registry_tasks (
        task_id TEXT PRIMARY KEY,project_id TEXT NOT NULL,contract_digest TEXT NOT NULL,contract_json TEXT NOT NULL,
        UNIQUE(task_id,contract_digest)) STRICT"),
    ("table", "workplan_registry_revision_tasks", "CREATE TABLE IF NOT EXISTS main.workplan_registry_revision_tasks (
        project_id TEXT NOT NULL,plan_id TEXT NOT NULL,revision INTEGER NOT NULL,task_id TEXT NOT NULL,
        contract_digest TEXT NOT NULL,position INTEGER NOT NULL CHECK(position>=0),
        PRIMARY KEY(project_id,plan_id,revision,task_id),UNIQUE(project_id,plan_id,revision,position),
        FOREIGN KEY(project_id,plan_id,revision) REFERENCES workplan_registry_revisions(project_id,plan_id,revision),
        FOREIGN KEY(task_id,contract_digest) REFERENCES workplan_registry_tasks(task_id,contract_digest)) STRICT"),
    ("index", "workplan_registry_task_anchors", "CREATE INDEX IF NOT EXISTS main.workplan_registry_task_anchors ON workplan_registry_revision_tasks(task_id,project_id,plan_id,revision)"),
];

/// Explicit initialization after the calling store's migrations, in autocommit.
/// Enables FK checks, leaves global user_version and unrelated tables untouched.
pub fn initialize(connection: &Connection) -> Result<()> {
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection.pragma_update(None, "foreign_keys", true)?;
    connection_assumptions(connection)?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    verify_schema(&tx)?;
    for (kind, table, _) in SCHEMA {
        if *kind != "table" {
            continue;
        }
        if tx
            .prepare(&format!("PRAGMA main.foreign_key_check({table})"))?
            .exists([])?
        {
            return Err(WorkplanStoreError::Integrity("foreign key closure"));
        }
    }
    tx.commit()?;
    Ok(())
}
fn normalized_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .replace("if not exists ", "")
        .replace("main.", "")
        .trim_end_matches(';')
        .to_owned()
}
fn verify_schema(tx: &Transaction<'_>) -> Result<()> {
    connection_assumptions(tx)?;
    for (kind, name, expected) in SCHEMA {
        let sql: Option<String> = tx
            .query_row(
                "SELECT sql FROM main.sqlite_schema WHERE type=?1 AND name=?2",
                params![kind, name],
                |row| stored_text(row, 0, 16 * 1024),
            )
            .optional()?;
        if sql.as_deref().map(normalized_sql) != Some(normalized_sql(expected)) {
            return Err(WorkplanStoreError::Schema);
        }
    }
    let triggers:i64=tx.query_row("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name IN ('workplan_registry_plans','workplan_registry_revisions','workplan_registry_tasks','workplan_registry_revision_tasks')",[],|row|row.get(0))?;
    if triggers != 0 {
        return Err(WorkplanStoreError::Schema);
    }
    Ok(())
}
fn connection_assumptions(connection: &Connection) -> Result<()> {
    let fk: i64 = connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let busy: i64 = connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    if fk != 1 || !(0..=MAX_BUSY_MILLIS).contains(&busy) {
        return Err(WorkplanStoreError::Schema);
    }
    Ok(())
}
fn safe_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        return Err(WorkplanStoreError::Invalid(
            "bounded ASCII identifier required",
        ));
    }
    TaskId::new(value.to_owned())
        .map(|_| ())
        .map_err(|_| WorkplanStoreError::Invalid("bounded ASCII identifier required"))
}
fn revision(value: &str) -> Result<u32> {
    if value.len() > 10 {
        return Err(WorkplanStoreError::Invalid(
            "canonical u32 revision required",
        ));
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or(WorkplanStoreError::Invalid(
            "canonical u32 revision required",
        ))
}
fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}
struct Prepared {
    plan: Workplan,
    preview: WorkplanPreview,
    canonical: String,
    request_digest: String,
    receipt: WorkplanReceipt,
    source_bytes: usize,
}
#[derive(Serialize)]
struct CommandIdentity<'a> {
    command_id: &'a str,
    project_id: &'a ProjectId,
    plan_id: &'a str,
    expected_current_revision: &'a str,
    source_digest: &'a str,
    canonical_digest: &'a str,
}
fn prepare(request: &RegisterWorkplanRequest) -> Result<Prepared> {
    if request.source.len() > MAX_WORKPLAN_BYTES {
        return Err(WorkplanStoreError::TooLarge);
    }
    // Bounds before cloning identifiers; raw source is independently capped and
    // never serialized as a JSON byte array to compute command identity.
    if request.command_id.len() > 128 || request.plan_id.len() > 128 {
        return Err(WorkplanStoreError::TooLarge);
    }
    safe_id(&request.command_id)?;
    safe_id(&request.plan_id)?;
    let previous = revision(&request.expected_current_revision)?;
    let preview = preview_workplan(&request.source)?;
    if preview.project_id != request.project_id || preview.plan_id != request.plan_id {
        return Err(WorkplanStoreError::ScopeMismatch);
    }
    let plan: Workplan = serde_json::from_slice(&request.source)?;
    bounded_json_size(&plan, MAX_WORKPLAN_BYTES).map_err(|_| WorkplanStoreError::TooLarge)?;
    let canonical_digest = content_digest(&plan)?;
    if preview.source_digest != request.source_digest
        || canonical_digest != request.canonical_digest
    {
        return Err(WorkplanStoreError::Invalid(
            "source/canonical digest mismatch",
        ));
    }
    if previous.checked_add(1) != Some(plan.revision) {
        return Err(WorkplanStoreError::RevisionSequence);
    }
    let request_digest = content_digest(&CommandIdentity {
        command_id: &request.command_id,
        project_id: &request.project_id,
        plan_id: &request.plan_id,
        expected_current_revision: &request.expected_current_revision,
        source_digest: &request.source_digest,
        canonical_digest: &request.canonical_digest,
    })?;
    let receipt = WorkplanReceipt {
        command_id: request.command_id.clone(),
        project_id: plan.project_id.clone(),
        plan_id: plan.plan_id.clone(),
        revision: plan.revision.to_string(),
        previous_revision: previous.to_string(),
        source_digest: preview.source_digest.clone(),
        canonical_digest,
        task_order: preview.task_order.clone(),
        task_digests: preview.task_digests.clone(),
        recorded_only: true,
        native_execution_qualified: false,
        task_events_emitted: false,
        execution_granted: false,
        acceptance_granted: false,
    };
    bounded_json_size(&receipt, MAX_RECEIPT_BYTES).map_err(|_| WorkplanStoreError::TooLarge)?;
    Ok(Prepared {
        canonical: serde_json::to_string(&plan)?,
        plan,
        preview,
        request_digest,
        receipt,
        source_bytes: request.source.len(),
    })
}

pub fn register(
    connection: &mut Connection,
    request: RegisterWorkplanRequest,
) -> Result<WorkplanReceipt> {
    register_with_existing(connection, request, &BTreeMap::new())
}
pub fn register_with_existing(
    connection: &mut Connection,
    request: RegisterWorkplanRequest,
    existing: &BTreeMap<TaskId, String>,
) -> Result<WorkplanReceipt> {
    let prepared = prepare(&request)?;
    if existing.len() > MAX_REGISTERED_TASKS {
        return Err(WorkplanStoreError::Capacity);
    }
    for digest in existing.values() {
        if !is_digest(digest) {
            return Err(WorkplanStoreError::Invalid(
                "invalid authoritative task digest",
            ));
        }
    }
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection_assumptions(connection)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_schema(&tx)?;
    let prior:Option<(String,String,u32,String)>=tx.query_row("SELECT project_id,plan_id,revision,request_digest FROM main.workplan_registry_revisions WHERE command_id=?1",[&request.command_id],|row|Ok((stored_text(row,0,128)?,stored_text(row,1,128)?,row.get(2)?,stored_text(row,3,71)?))).optional()?;
    if let Some((project, plan, number, digest)) = prior {
        if project != request.project_id.as_str()
            || plan != request.plan_id
            || digest != prepared.request_digest
        {
            return Err(WorkplanStoreError::IdentityCollision);
        }
        validate_plan(&tx, &request.project_id, &request.plan_id)?
            .ok_or(WorkplanStoreError::Integrity("missing current pointer"))?;
        let stored = read_revision(&tx, &request.project_id, &request.plan_id, number)?
            .ok_or(WorkplanStoreError::Integrity("missing replay revision"))?;
        if stored.receipt != prepared.receipt {
            return Err(WorkplanStoreError::Integrity("replay receipt binding"));
        }
        return Ok(stored.receipt);
    }
    let actual = validate_plan(&tx, &request.project_id, &request.plan_id)?.unwrap_or(0);
    if actual != revision(&request.expected_current_revision)? {
        return Err(WorkplanStoreError::StaleCurrent);
    }
    if prepared.plan.revision as usize > MAX_REVISIONS_PER_PLAN {
        return Err(WorkplanStoreError::Capacity);
    }
    let mut known = known_tasks(&tx)?;
    for (id, digest) in existing {
        if known.get(id).is_some_and(|prior| prior != digest) {
            return Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(id.to_string()),
            ));
        }
        known.insert(id.clone(), digest.clone());
    }
    if known.len() > MAX_REGISTERED_TASKS {
        return Err(WorkplanStoreError::Capacity);
    }
    preview_workplan_with_existing(&request.source, &known)?;
    admit_capacity(&tx, &prepared, actual == 0)?;
    if actual == 0 {
        tx.execute("INSERT INTO main.workplan_registry_plans(project_id,plan_id,current_revision) VALUES(?1,?2,?3)",params![request.project_id.as_str(),request.plan_id,prepared.plan.revision])?;
    }
    tx.execute("INSERT INTO main.workplan_registry_revisions(project_id,plan_id,revision,command_id,request_digest,source_digest,canonical_digest,source_json,canonical_json,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![request.project_id.as_str(),request.plan_id,prepared.plan.revision,request.command_id,prepared.request_digest,request.source_digest,request.canonical_digest,std::str::from_utf8(&request.source).map_err(|_|WorkplanStoreError::Invalid("UTF-8 JSON required"))?,prepared.canonical,serde_json::to_string(&prepared.receipt)?])?;
    for task in &prepared.plan.tasks {
        let digest = &prepared.preview.task_digests[&task.task_id];
        tx.execute("INSERT OR IGNORE INTO main.workplan_registry_tasks(task_id,project_id,contract_digest,contract_json) VALUES(?1,?2,?3,?4)",params![task.task_id.as_str(),task.project_id.as_str(),digest,serde_json::to_string(task)?])?;
    }
    for (position, id) in prepared.preview.task_order.iter().enumerate() {
        tx.execute("INSERT INTO main.workplan_registry_revision_tasks(project_id,plan_id,revision,task_id,contract_digest,position) VALUES(?1,?2,?3,?4,?5,?6)",params![request.project_id.as_str(),request.plan_id,prepared.plan.revision,id.as_str(),prepared.preview.task_digests[id],position as i64])?;
    }
    if actual != 0 {
        let changed=tx.execute("UPDATE main.workplan_registry_plans SET current_revision=?1 WHERE project_id=?2 AND plan_id=?3 AND current_revision=?4",params![prepared.plan.revision,request.project_id.as_str(),request.plan_id,actual])?;
        if changed != 1 {
            return Err(WorkplanStoreError::StaleCurrent);
        }
    }
    current(&tx, &request.project_id, &request.plan_id)?;
    let stored = read_revision(
        &tx,
        &request.project_id,
        &request.plan_id,
        prepared.plan.revision,
    )?
    .ok_or(WorkplanStoreError::Integrity("new revision missing"))?;
    tx.commit()?;
    Ok(stored.receipt)
}

fn known_tasks(tx: &Transaction<'_>) -> Result<BTreeMap<TaskId, String>> {
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM main.workplan_registry_tasks",
        [],
        |row| row.get(0),
    )?;
    if count < 0 || count as usize > MAX_REGISTERED_TASKS {
        return Err(WorkplanStoreError::Capacity);
    }
    let mut statement=tx.prepare("SELECT task_id,contract_digest FROM main.workplan_registry_tasks ORDER BY task_id LIMIT ?1")?;
    let mut rows = statement.query([MAX_REGISTERED_TASKS as i64 + 1])?;
    let mut known = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let id = TaskId::new(stored_text(row, 0, 128)?)
            .map_err(|_| WorkplanStoreError::Integrity("stored task ID"))?;
        let digest = stored_text(row, 1, 71)?;
        if !is_digest(&digest) {
            return Err(WorkplanStoreError::Integrity("stored task digest"));
        }
        known.insert(id, digest);
    }
    Ok(known)
}
fn admit_capacity(tx: &Transaction<'_>, prepared: &Prepared, new_plan: bool) -> Result<()> {
    let plans: i64 = tx.query_row(
        "SELECT count(*) FROM main.workplan_registry_plans",
        [],
        |row| row.get(0),
    )?;
    let (revisions,bytes):(i64,i64)=tx.query_row("SELECT count(*),coalesce(sum(length(CAST(source_json AS BLOB))+length(CAST(canonical_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM main.workplan_registry_revisions",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
    let source_bytes = prepared.canonical.len()
        + prepared.source_bytes
        + serde_json::to_string(&prepared.receipt)?.len();
    if plans as usize > MAX_PLANS
        || (new_plan && plans as usize == MAX_PLANS)
        || revisions as usize >= MAX_PLANS * MAX_REVISIONS_PER_PLAN
        || bytes < 0
        || bytes as usize > MAX_REGISTRY_BYTES.saturating_sub(source_bytes)
    {
        return Err(WorkplanStoreError::Capacity);
    }
    let (tasks,contract_bytes):(i64,i64)=tx.query_row("SELECT count(*),coalesce(sum(length(CAST(contract_json AS BLOB))),0) FROM main.workplan_registry_tasks",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
    let mut new_count = 0;
    let mut new_bytes = 0;
    for task in &prepared.plan.tasks {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.workplan_registry_tasks WHERE task_id=?1)",
            [task.task_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            new_count += 1;
            new_bytes += serde_json::to_string(task)?.len();
        }
    }
    if tasks < 0
        || contract_bytes < 0
        || tasks as usize + new_count > MAX_REGISTERED_TASKS
        || contract_bytes as usize > MAX_REGISTRY_BYTES.saturating_sub(new_bytes)
    {
        return Err(WorkplanStoreError::Capacity);
    }
    Ok(())
}
fn current(tx: &Transaction<'_>, project: &ProjectId, plan: &str) -> Result<Option<u32>> {
    let pointer:Option<u32>=tx.query_row("SELECT current_revision FROM main.workplan_registry_plans WHERE project_id=?1 AND plan_id=?2",params![project.as_str(),plan],|row|row.get(0)).optional()?;
    let (count,maximum):(i64,Option<u32>)=tx.query_row("SELECT count(*),max(revision) FROM main.workplan_registry_revisions WHERE project_id=?1 AND plan_id=?2",params![project.as_str(),plan],|row|Ok((row.get(0)?,row.get(1)?)))?;
    if pointer.is_none() && count == 0 {
        return Ok(None);
    }
    if pointer.is_none()
        || pointer != maximum
        || count != i64::from(pointer.unwrap_or(0))
        || count <= 0
        || count as usize > MAX_REVISIONS_PER_PLAN
    {
        return Err(WorkplanStoreError::Integrity(
            "revision/current pointer closure",
        ));
    }
    Ok(pointer)
}

fn validate_plan(tx: &Transaction<'_>, project: &ProjectId, plan: &str) -> Result<Option<u32>> {
    let Some(head) = current(tx, project, plan)? else {
        return Ok(None);
    };
    for number in 1..=head {
        read_revision(tx, project, plan, number)?
            .ok_or(WorkplanStoreError::Integrity("missing retained revision"))?;
    }
    Ok(Some(head))
}

struct RevisionRow {
    command: String,
    request_digest: String,
    source_digest: String,
    canonical_digest: String,
    source: String,
    canonical: String,
    receipt: String,
}
fn read_revision(
    tx: &Transaction<'_>,
    project: &ProjectId,
    plan: &str,
    number: u32,
) -> Result<Option<RegisteredWorkplan>> {
    let row:Option<RevisionRow>=tx.query_row("SELECT command_id,request_digest,source_digest,canonical_digest,source_json,canonical_json,receipt_json FROM main.workplan_registry_revisions WHERE project_id=?1 AND plan_id=?2 AND revision=?3",params![project.as_str(),plan,number],|row|Ok(RevisionRow {command:stored_text(row,0,128)?,request_digest:stored_text(row,1,71)?,source_digest:stored_text(row,2,71)?,canonical_digest:stored_text(row,3,71)?,source:stored_text(row,4,MAX_WORKPLAN_BYTES)?,canonical:stored_text(row,5,MAX_WORKPLAN_BYTES)?,receipt:stored_text(row,6,MAX_RECEIPT_BYTES)?})).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let previous = number
        .checked_sub(1)
        .ok_or(WorkplanStoreError::Integrity("zero stored revision"))?;
    let prepared = prepare(&RegisterWorkplanRequest {
        command_id: row.command,
        project_id: project.clone(),
        plan_id: plan.to_owned(),
        expected_current_revision: previous.to_string(),
        source: row.source.into_bytes(),
        source_digest: row.source_digest,
        canonical_digest: row.canonical_digest,
    })?;
    if prepared.request_digest != row.request_digest
        || prepared.canonical != row.canonical
        || serde_json::to_string(&prepared.receipt)? != row.receipt
    {
        return Err(WorkplanStoreError::Integrity(
            "stored source/canonical/receipt identity",
        ));
    }
    let count:i64=tx.query_row("SELECT count(*) FROM main.workplan_registry_revision_tasks WHERE project_id=?1 AND plan_id=?2 AND revision=?3",params![project.as_str(),plan,number],|row|row.get(0))?;
    if count != prepared.preview.task_order.len() as i64 {
        return Err(WorkplanStoreError::Integrity("revision task closure"));
    }
    let mut statement=tx.prepare("SELECT task_id,contract_digest,position FROM main.workplan_registry_revision_tasks WHERE project_id=?1 AND plan_id=?2 AND revision=?3 ORDER BY position LIMIT 1001")?;
    let mut rows = statement.query(params![project.as_str(), plan, number])?;
    let mut position = 0;
    while let Some(row) = rows.next()? {
        let id = stored_text(row, 0, 128)?;
        let digest = stored_text(row, 1, 71)?;
        let offset: i64 = row.get(2)?;
        if prepared
            .preview
            .task_order
            .get(position)
            .map(TaskId::as_str)
            != Some(id.as_str())
            || offset != position as i64
            || prepared
                .preview
                .task_digests
                .get(&prepared.preview.task_order[position])
                != Some(&digest)
        {
            return Err(WorkplanStoreError::Integrity(
                "dependency order/task digest",
            ));
        }
        position += 1;
    }
    for task in &prepared.plan.tasks {
        let stored:Option<(String,String,String)>=tx.query_row("SELECT project_id,contract_digest,contract_json FROM main.workplan_registry_tasks WHERE task_id=?1",[task.task_id.as_str()],|row|Ok((stored_text(row,0,128)?,stored_text(row,1,71)?,stored_text(row,2,MAX_WORKPLAN_BYTES)?))).optional()?;
        let Some((project, digest, canonical)) = stored else {
            return Err(WorkplanStoreError::Integrity(
                "missing retained task identity",
            ));
        };
        if project != task.project_id.as_str()
            || digest != prepared.preview.task_digests[&task.task_id]
            || canonical != serde_json::to_string(task)?
        {
            return Err(WorkplanStoreError::Integrity(
                "retained task contract identity",
            ));
        }
    }
    Ok(Some(RegisteredWorkplan {
        plan: prepared.plan,
        receipt: prepared.receipt,
    }))
}

pub fn get(
    connection: &mut Connection,
    project: &ProjectId,
    plan_id: &str,
    requested_revision: Option<u32>,
) -> Result<Option<RegisteredWorkplan>> {
    safe_id(plan_id)?;
    if requested_revision == Some(0) {
        return Err(WorkplanStoreError::Invalid("revision must be positive"));
    }
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection_assumptions(connection)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    verify_schema(&tx)?;
    let Some(head) = validate_plan(&tx, project, plan_id)? else {
        return Ok(None);
    };
    let result = read_revision(&tx, project, plan_id, requested_revision.unwrap_or(head))?;
    tx.commit()?;
    Ok(result)
}
pub fn list(
    connection: &mut Connection,
    project: &ProjectId,
    after_plan: Option<&str>,
    limit: usize,
) -> Result<WorkplanPage> {
    if limit == 0 || limit > MAX_LIST_PAGE {
        return Err(WorkplanStoreError::Invalid("list limit must be 1..100"));
    }
    if let Some(cursor) = after_plan {
        safe_id(cursor)?;
    }
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection_assumptions(connection)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    verify_schema(&tx)?;
    let total: i64 = tx.query_row(
        "SELECT count(*) FROM main.workplan_registry_plans",
        [],
        |row| row.get(0),
    )?;
    if total < 0 || total as usize > MAX_PLANS {
        return Err(WorkplanStoreError::Capacity);
    }
    let mut statement=tx.prepare("SELECT plan_id FROM main.workplan_registry_plans WHERE project_id=?1 AND plan_id>?2 ORDER BY plan_id LIMIT ?3")?;
    let names = statement
        .query_map(
            params![
                project.as_str(),
                after_plan.unwrap_or(""),
                (limit + 1) as i64
            ],
            |row| stored_text(row, 0, 128),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let more = names.len() > limit;
    let mut entries = Vec::with_capacity(names.len().min(limit));
    for name in names.into_iter().take(limit) {
        safe_id(&name)?;
        let head = validate_plan(&tx, project, &name)?
            .ok_or(WorkplanStoreError::Integrity("listed pointer missing"))?;
        let record = read_revision(&tx, project, &name, head)?
            .ok_or(WorkplanStoreError::Integrity("listed revision missing"))?;
        entries.push(WorkplanSummary {
            project_id: project.clone(),
            plan_id: name,
            current_revision: record.receipt.revision,
            source_digest: record.receipt.source_digest,
            canonical_digest: record.receipt.canonical_digest,
            task_count: record.plan.tasks.len(),
        });
    }
    let next_after = if more {
        entries.last().map(|entry| entry.plan_id.clone())
    } else {
        None
    };
    drop(statement);
    tx.commit()?;
    Ok(WorkplanPage {
        entries,
        next_after,
        recorded_only: true,
        native_execution_qualified: false,
        task_events_emitted: false,
        execution_granted: false,
        acceptance_granted: false,
    })
}

/// Reciprocal TaskSubmitted guard. The owned writer calls this before append;
/// no event, acceptance or task launch is produced by this read.
pub fn validate_task_contract(connection: &Connection, contract: &TaskContract) -> Result<()> {
    bounded_json_size(contract, MAX_WORKPLAN_BYTES).map_err(|_| WorkplanStoreError::TooLarge)?;
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection_assumptions(connection)?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
    validate_task_contract_in_transaction(&tx, contract)?;
    tx.commit()?;
    Ok(())
}

/// Same reciprocal guard under the caller's already-owned append transaction.
/// It neither starts nor commits a transaction and preserves outer rollback.
pub fn validate_task_contract_in_transaction(
    tx: &Transaction<'_>,
    contract: &TaskContract,
) -> Result<()> {
    bounded_json_size(contract, MAX_WORKPLAN_BYTES).map_err(|_| WorkplanStoreError::TooLarge)?;
    verify_schema(tx)?;
    let retained: Option<(String, String, String)> = tx.query_row(
        "SELECT project_id,contract_digest,contract_json FROM main.workplan_registry_tasks WHERE task_id=?1",
        [contract.task_id.as_str()], |row| Ok((stored_text(row,0,128)?,stored_text(row,1,71)?,stored_text(row,2,MAX_WORKPLAN_BYTES)?)),
    ).optional()?;
    let Some((project, digest, canonical)) = retained else {
        return Ok(());
    };
    let anchor: Option<(String,String)> = tx.query_row(
        "SELECT project_id,plan_id FROM main.workplan_registry_revision_tasks WHERE task_id=?1 ORDER BY project_id,plan_id,revision LIMIT 1",
        [contract.task_id.as_str()], |row| Ok((stored_text(row,0,128)?,stored_text(row,1,128)?)),
    ).optional()?;
    let (anchor_project, anchor_plan) =
        anchor.ok_or(WorkplanStoreError::Integrity("unanchored retained task"))?;
    let anchor_project = ProjectId::new(anchor_project)
        .map_err(|_| WorkplanStoreError::Integrity("retained task project"))?;
    validate_plan(tx, &anchor_project, &anchor_plan)?
        .ok_or(WorkplanStoreError::Integrity("retained task plan missing"))?;
    if project != contract.project_id.as_str()
        || digest != content_digest(contract)?
        || canonical != serde_json::to_string(contract)?
    {
        return Err(WorkplanStoreError::Parser(
            WorkplanError::TaskIdentityConflict(contract.task_id.to_string()),
        ));
    }
    Ok(())
}

/// Startup/rebuild compatibility validation without rescanning one plan for each
/// submitted task. Unrelated domain tasks need no retained allocation or hash.
/// The caller supplies finite resident projection references, not unbounded I/O.
pub fn validate_task_contracts<'a>(
    connection: &Connection,
    contracts: impl IntoIterator<Item = &'a TaskContract>,
) -> Result<()> {
    if !connection.is_autocommit() {
        return Err(WorkplanStoreError::Schema);
    }
    connection_assumptions(connection)?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
    validate_task_contracts_inner(&tx, contracts, |tx, project, plan| {
        validate_plan(tx, project, plan)?
            .ok_or(WorkplanStoreError::Integrity("retained task plan missing"))?;
        Ok(())
    })?;
    tx.commit()?;
    Ok(())
}

fn validate_task_contracts_inner<'a>(
    tx: &Transaction<'_>,
    contracts: impl IntoIterator<Item = &'a TaskContract>,
    mut validate_anchor: impl FnMut(&Transaction<'_>, &ProjectId, &str) -> Result<()>,
) -> Result<()> {
    verify_schema(tx)?;
    let known = known_tasks(tx)?;
    if known.is_empty() {
        return Ok(());
    }
    let mut matched = BTreeMap::<TaskId, &'a TaskContract>::new();
    for contract in contracts {
        if !known.contains_key(&contract.task_id) {
            continue;
        }
        bounded_json_size(contract, MAX_WORKPLAN_BYTES)
            .map_err(|_| WorkplanStoreError::TooLarge)?;
        if let Some(previous) = matched.get(&contract.task_id) {
            if *previous != contract {
                return Err(WorkplanStoreError::Parser(
                    WorkplanError::TaskIdentityConflict(contract.task_id.to_string()),
                ));
            }
        } else {
            // At most MAX_REGISTERED_TASKS entries, all resident references.
            matched.insert(contract.task_id.clone(), contract);
        }
    }
    let mut anchors = BTreeSet::new();
    for id in matched.keys() {
        let anchor: Option<(String, String)> = tx.query_row(
            "SELECT project_id,plan_id FROM main.workplan_registry_revision_tasks WHERE task_id=?1 ORDER BY project_id,plan_id,revision LIMIT 1",
            [id.as_str()], |row| Ok((stored_text(row,0,128)?,stored_text(row,1,128)?)),
        ).optional()?;
        let (project, plan) =
            anchor.ok_or(WorkplanStoreError::Integrity("unanchored retained task"))?;
        safe_id(&plan)?;
        anchors.insert((project, plan));
    }
    if anchors.len() > MAX_PLANS {
        return Err(WorkplanStoreError::Capacity);
    }
    for (project, plan) in anchors {
        let project = ProjectId::new(project)
            .map_err(|_| WorkplanStoreError::Integrity("retained task project"))?;
        validate_anchor(tx, &project, &plan)?;
    }
    for (id, contract) in matched {
        let retained: Option<(String, String, String)> = tx.query_row(
            "SELECT project_id,contract_digest,contract_json FROM main.workplan_registry_tasks WHERE task_id=?1",
            [id.as_str()], |row| Ok((stored_text(row,0,128)?,stored_text(row,1,71)?,stored_text(row,2,MAX_WORKPLAN_BYTES)?)),
        ).optional()?;
        let (project, digest, canonical) = retained.ok_or(WorkplanStoreError::Integrity(
            "missing retained task identity",
        ))?;
        if project != contract.project_id.as_str()
            || known.get(&id) != Some(&digest)
            || digest != content_digest(contract)?
            || canonical != serde_json::to_string(contract)?
        {
            return Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(id.to_string()),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, RootId};
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    fn project() -> ProjectId {
        ProjectId::new("project").expect("fixture project")
    }
    fn contract(id: &str, dependencies: &[&str]) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: project(),
            root_id: RootId::new("root-one").expect("fixture root"),
            task_id: TaskId::new(id).expect("fixture task"),
            revision: 1,
            title: format!("Task {id}"),
            objective: "Do not remove the guard.\nKeep the 12 ms limit and 10 MiB bound exactly."
                .into(),
            non_goals: vec!["Do not restart any service".into()],
            owned_paths: vec![format!("src/{id}.rs")],
            forbidden_paths: vec!["secrets".into()],
            read_first: vec!["README.md".into()],
            dependencies: dependencies
                .iter()
                .map(|id| TaskId::new(*id).expect("fixture dependency"))
                .collect(),
            criteria: vec![AcceptanceCriterion {
                id: "negative-check".into(),
                text: "The guard remains enabled".into(),
                check_ids: vec!["test".into()],
            }],
            checks: vec![CheckSpec {
                id: "test".into(),
                program: "cargo".into(),
                args: vec![
                    "test".into(),
                    "--".into(),
                    "--exact".into(),
                    "must_not_delete".into(),
                ],
                cwd: ".".into(),
                timeout_secs: 12,
            }],
            authority_ref: "grant:recorded-plan".into(),
            max_attempts: 1,
            simulated: true,
        }
    }
    fn request(
        command: &str,
        plan_id: &str,
        number: u32,
        tasks: Vec<TaskContract>,
    ) -> RegisterWorkplanRequest {
        let plan = Workplan {
            schema_version: 1,
            plan_id: plan_id.into(),
            revision: number,
            project_id: project(),
            tasks,
        };
        let source = serde_json::to_vec(&plan).expect("fixture workplan JSON");
        RegisterWorkplanRequest {
            command_id: command.into(),
            project_id: project(),
            plan_id: plan_id.into(),
            expected_current_revision: (number - 1).to_string(),
            source_digest: preview_workplan(&source)
                .expect("fixture core preview")
                .source_digest,
            canonical_digest: content_digest(&plan).expect("fixture canonical digest"),
            source,
        }
    }
    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("fixture SQLite");
        connection
            .busy_timeout(Duration::from_secs(1))
            .expect("fixture busy timeout");
        initialize(&connection).expect("fixture registry initialization");
        connection
    }
    fn counts(connection: &Connection) -> (i64, i64, i64) {
        connection.query_row("SELECT (SELECT count(*) FROM main.workplan_registry_plans),(SELECT count(*) FROM main.workplan_registry_revisions),(SELECT count(*) FROM main.workplan_registry_tasks)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).expect("registry count fixture")
    }

    #[test]
    fn reopen_exact_historical_replay_preserves_current_and_exact_mixed_roots_constraints() {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("plans.sqlite");
        let mut connection = Connection::open(&path).expect("fixture file DB");
        initialize(&connection).expect("initialize file DB");
        let first = contract("a", &[]);
        let mut second = contract("b", &["a"]);
        second.root_id = RootId::new("root-two").expect("second root");
        let one = request(
            "register-one",
            "plan",
            1,
            vec![second.clone(), first.clone()],
        );
        let receipt = register(&mut connection, one.clone()).expect("register first exact plan");
        assert_eq!(
            receipt.task_order,
            [first.task_id.clone(), second.task_id.clone()]
        );
        register(
            &mut connection,
            request("register-two", "plan", 2, vec![first.clone()]),
        )
        .expect("register next revision");
        drop(connection);
        let mut reopened = Connection::open(&path).expect("reopen registry");
        initialize(&reopened).expect("explicit initialization after reopen");
        assert_eq!(
            register(&mut reopened, one).expect("historical replay"),
            receipt
        );
        let latest = get(&mut reopened, &project(), "plan", None)
            .expect("get current")
            .expect("current plan");
        assert_eq!(latest.plan.revision, 2);
        let historic = get(&mut reopened, &project(), "plan", Some(1))
            .expect("get historical")
            .expect("historical plan");
        assert_eq!(historic.plan.tasks, vec![second, first]);
        let json = serde_json::to_value(&historic.receipt).expect("receipt output");
        assert_eq!(json["recorded_only"], true);
        for flag in [
            "native_execution_qualified",
            "task_events_emitted",
            "execution_granted",
            "acceptance_granted",
        ] {
            assert_eq!(json[flag], false);
        }
        assert_eq!(counts(&reopened), (1, 2, 2));
    }

    #[test]
    fn command_scope_digest_and_revision_collisions_are_atomic() {
        let mut connection = database();
        let first = request("stable-command", "plan", 1, vec![contract("a", &[])]);
        register(&mut connection, first.clone()).expect("initial registration");
        let mut whitespace = first.clone();
        let plan: Workplan = serde_json::from_slice(&whitespace.source).expect("fixture plan");
        whitespace.source = serde_json::to_vec_pretty(&plan).expect("different source bytes");
        whitespace.source_digest = preview_workplan(&whitespace.source)
            .expect("pretty preview")
            .source_digest;
        assert!(matches!(
            register(&mut connection, whitespace),
            Err(WorkplanStoreError::IdentityCollision)
        ));
        assert!(matches!(
            register(
                &mut connection,
                request("other-command", "plan", 1, vec![contract("a", &[])])
            ),
            Err(WorkplanStoreError::StaleCurrent)
        ));
        let mut skipped = request("skip", "plan", 3, vec![]);
        skipped.expected_current_revision = "1".into();
        assert!(matches!(
            register(&mut connection, skipped),
            Err(WorkplanStoreError::RevisionSequence)
        ));
        let mut foreign = first.clone();
        foreign.project_id = ProjectId::new("other").expect("foreign project");
        assert!(matches!(
            register(&mut connection, foreign),
            Err(WorkplanStoreError::ScopeMismatch)
        ));
        let mut bad = first;
        bad.command_id = "bad-digest".into();
        bad.canonical_digest = format!("sha256:{}", "b".repeat(64));
        assert!(matches!(
            register(&mut connection, bad),
            Err(WorkplanStoreError::Invalid(_))
        ));
        assert_eq!(counts(&connection), (1, 1, 1));
    }

    #[test]
    fn removed_task_id_cannot_be_rebound_and_domain_preflight_is_reciprocal() {
        let mut connection = database();
        let task = contract("immutable", &[]);
        validate_task_contract(&connection, &task)
            .expect("no retained binding permits domain preflight");
        register(
            &mut connection,
            request("one", "plan", 1, vec![task.clone()]),
        )
        .expect("initial identity");
        register(&mut connection, request("two", "plan", 2, vec![]))
            .expect("remove task from current plan");
        let mut changed = task.clone();
        changed.objective = "Different scope under old ID".into();
        assert!(matches!(
            register(
                &mut connection,
                request("bad-three", "plan", 3, vec![changed.clone()])
            ),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        assert!(matches!(
            validate_task_contract(&connection, &changed),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        validate_task_contract(&connection, &task).expect("exact later submission permitted");
        register(
            &mut connection,
            request("three", "plan", 3, vec![task.clone()]),
        )
        .expect("unchanged task may return");
        let external =
            BTreeMap::from([(task.task_id.clone(), format!("sha256:{}", "f".repeat(64)))]);
        assert!(matches!(
            register_with_existing(
                &mut connection,
                request("other-plan", "external", 1, vec![task.clone()]),
                &external
            ),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        connection.execute("UPDATE main.workplan_registry_tasks SET contract_json='{}' WHERE task_id='immutable'",[]).expect("inject retained contract corruption");
        assert!(validate_task_contract(&connection, &task).is_err());
    }

    #[test]
    fn reciprocal_guard_joins_owned_append_transaction_without_committing_it() {
        let mut connection = database();
        let task = contract("pinned", &[]);
        register(
            &mut connection,
            request("first", "plan", 1, vec![task.clone()]),
        )
        .expect("pinned contract fixture");
        connection
            .execute_batch("CREATE TABLE append_effects(marker TEXT)")
            .expect("outer transaction effect fixture");
        {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .expect("owned append transaction");
            tx.execute("INSERT INTO append_effects VALUES('uncommitted')", [])
                .expect("outer provisional write");
            validate_task_contract_in_transaction(&tx, &task)
                .expect("matching contract inside append");
            assert!(
                !tx.is_autocommit(),
                "guard cannot commit caller transaction"
            );
            let mut changed = task.clone();
            changed.checks[0].args.push("different".into());
            assert!(matches!(
                validate_task_contract_in_transaction(&tx, &changed),
                Err(WorkplanStoreError::Parser(
                    WorkplanError::TaskIdentityConflict(_)
                ))
            ));
            // Caller rollback includes every effect preceding the guard.
        }
        let count: i64 = connection
            .query_row("SELECT count(*) FROM append_effects", [], |row| row.get(0))
            .expect("outer rollback count");
        assert_eq!(count, 0);
        let external_task = contract("already-submitted", &[]);
        let existing = BTreeMap::from([(
            external_task.task_id.clone(),
            format!("sha256:{}", "c".repeat(64)),
        )]);
        assert!(matches!(
            register_with_existing(
                &mut connection,
                request("external-conflict", "new-plan", 1, vec![external_task]),
                &existing
            ),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        assert_eq!(counts(&connection), (1, 1, 1));
    }

    #[test]
    fn batch_guard_validates_shared_anchor_once_and_skips_unrelated_projection_size() {
        let mut connection = database();
        let untouched = std::iter::from_fn(|| -> Option<&TaskContract> {
            panic!("empty registry must not consume unrelated projection iterator")
        });
        validate_task_contracts(&connection, untouched)
            .expect("empty registry is a constant-size read");
        let tasks = (0..20)
            .map(|index| contract(&format!("shared-{index}"), &[]))
            .collect::<Vec<_>>();
        register(
            &mut connection,
            request("shared-one", "shared-plan", 1, tasks.clone()),
        )
        .expect("shared anchor fixture");
        register(
            &mut connection,
            request("shared-two", "shared-plan", 2, tasks.clone()),
        )
        .expect("shared retained history");
        let unrelated = contract("unrelated", &[]);
        let tx = Transaction::new_unchecked(&connection, TransactionBehavior::Deferred)
            .expect("batch read snapshot");
        let mut anchor_validations = 0;
        let inputs = std::iter::repeat_n(&unrelated, MAX_REGISTERED_TASKS + 1)
            .chain(tasks.iter())
            .chain(tasks.iter());
        validate_task_contracts_inner(&tx, inputs, |tx, project, plan| {
            anchor_validations += 1;
            assert_eq!(plan, "shared-plan");
            assert_eq!(validate_plan(tx, project, plan)?, Some(2));
            Ok(())
        })
        .expect("shared bounded validation");
        assert_eq!(
            anchor_validations, 1,
            "twenty bound contracts share one history validation"
        );
        tx.commit().expect("finish read snapshot");
        assert_eq!(counts(&connection), (1, 2, 20));
    }

    #[test]
    fn batch_guard_rejects_conflicting_or_corrupt_rebuild_bindings_without_mutation() {
        let mut connection = database();
        let original = contract("retained", &[]);
        register(
            &mut connection,
            request("retained-one", "plan", 1, vec![original.clone()]),
        )
        .expect("retained identity fixture");
        let mut changed = original.clone();
        changed.objective = "Altered under same ID".into();
        assert!(matches!(
            validate_task_contracts(&connection, [&changed]),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        assert!(matches!(
            validate_task_contracts(&connection, [&original, &changed]),
            Err(WorkplanStoreError::Parser(
                WorkplanError::TaskIdentityConflict(_)
            ))
        ));
        assert_eq!(counts(&connection), (1, 1, 1));
        validate_task_contracts(&connection, [&original]).expect("unchanged projection binding");
        connection.execute("UPDATE main.workplan_registry_revisions SET receipt_json=json_set(receipt_json,'$.acceptance_granted',json('true'))",[]).expect("inject cached receipt corruption before rebuild guard");
        assert!(matches!(
            validate_task_contracts(&connection, [&original]),
            Err(WorkplanStoreError::Integrity(_))
        ));
        assert_eq!(counts(&connection), (1, 1, 1));
        let current: i64 = connection
            .query_row(
                "SELECT current_revision FROM main.workplan_registry_plans",
                [],
                |row| row.get(0),
            )
            .expect("unchanged current pointer");
        assert_eq!(current, 1);
    }

    #[test]
    fn validation_failure_after_writes_rolls_back_revision_pointer_receipt_and_new_tasks() {
        let mut connection = database();
        register(
            &mut connection,
            request("one", "plan", 1, vec![contract("a", &[])]),
        )
        .expect("initial plan");
        let foreign = contract("b", &[]);
        connection.execute("INSERT INTO main.workplan_registry_tasks(task_id,project_id,contract_digest,contract_json) VALUES(?1,?2,?3,'{}')",params![foreign.task_id.as_str(),foreign.project_id.as_str(),content_digest(&foreign).expect("fixture digest")]).expect("inject orphan canonical corruption");
        let before = counts(&connection);
        assert!(matches!(
            register(
                &mut connection,
                request(
                    "rollback",
                    "plan",
                    2,
                    vec![contract("a", &[]), foreign, contract("c", &[])]
                )
            ),
            Err(WorkplanStoreError::Integrity(_))
        ));
        assert_eq!(counts(&connection), before);
        let current: i64 = connection
            .query_row(
                "SELECT current_revision FROM main.workplan_registry_plans",
                [],
                |row| row.get(0),
            )
            .expect("pointer after rollback");
        assert_eq!(current, 1);
        let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM main.workplan_registry_revisions WHERE command_id='rollback')",[],|row|row.get(0)).expect("receipt after rollback");
        assert!(!exists);
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.workplan_registry_tasks WHERE task_id='c')",
                [],
                |row| row.get(0),
            )
            .expect("new identity after rollback");
        assert!(!exists);
    }

    #[test]
    fn cached_receipt_source_order_and_current_pointer_tampering_fail_closed() {
        for issue in [
            "receipt",
            "source",
            "canonical",
            "pointer",
            "order",
            "large-source",
        ] {
            let mut connection = database();
            let request = request(
                "one",
                "plan",
                1,
                vec![contract("a", &[]), contract("b", &["a"])],
            );
            register(&mut connection, request.clone()).expect("first plan");
            register(
                &mut connection,
                super::tests::request("two", "plan", 2, vec![contract("a", &[])]),
            )
            .expect("second plan");
            match issue {
                "receipt" => {
                    connection.execute("UPDATE main.workplan_registry_revisions SET receipt_json=json_set(receipt_json,'$.acceptance_granted',json('true')) WHERE revision=1",[]).expect("forge cached receipt");
                }
                "source" => {
                    connection.execute("UPDATE main.workplan_registry_revisions SET source_json='{}' WHERE revision=1",[]).expect("forge source");
                }
                "canonical" => {
                    connection.execute("UPDATE main.workplan_registry_revisions SET canonical_json='{}' WHERE revision=1",[]).expect("forge canonical bytes");
                }
                "pointer" => {
                    connection
                        .execute(
                            "UPDATE main.workplan_registry_plans SET current_revision=1",
                            [],
                        )
                        .expect("rewind pointer");
                }
                "order" => {
                    connection.execute("UPDATE main.workplan_registry_revision_tasks SET position=2 WHERE task_id='a' AND revision=1",[]).expect("forge task order");
                }
                _ => {
                    connection.execute("UPDATE main.workplan_registry_revisions SET source_json=?1 WHERE revision=1",["x".repeat(MAX_WORKPLAN_BYTES+1)]).expect("oversized stored source");
                }
            }
            assert!(
                get(&mut connection, &project(), "plan", None).is_err(),
                "current read must reject historical {issue}"
            );
            assert!(
                register(&mut connection, request).is_err(),
                "cached replay must reject {issue}"
            );
            assert!(
                list(&mut connection, &project(), None, 10).is_err(),
                "list must reject {issue}"
            );
            assert_eq!(counts(&connection), (1, 2, 2));
        }
    }

    #[test]
    fn project_scoped_keyset_listing_and_core_validation_are_preserved() {
        let mut connection = database();
        register(&mut connection, request("a", "a-plan", 1, vec![])).expect("first listing plan");
        register(&mut connection, request("b", "b-plan", 1, vec![])).expect("second listing plan");
        let other = ProjectId::new("other").expect("other project");
        let plan = Workplan {
            schema_version: 1,
            plan_id: "a-plan".into(),
            revision: 1,
            project_id: other.clone(),
            tasks: vec![],
        };
        let source = serde_json::to_vec(&plan).expect("other scoped source");
        register(
            &mut connection,
            RegisterWorkplanRequest {
                command_id: "other".into(),
                project_id: other.clone(),
                plan_id: "a-plan".into(),
                expected_current_revision: "0".into(),
                source_digest: preview_workplan(&source)
                    .expect("other preview")
                    .source_digest,
                canonical_digest: content_digest(&plan).expect("other digest"),
                source,
            },
        )
        .expect("same plan name in separate project");
        let page = list(&mut connection, &project(), None, 1).expect("first page");
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.next_after.as_deref(), Some("a-plan"));
        let next =
            list(&mut connection, &project(), page.next_after.as_deref(), 1).expect("next page");
        assert_eq!(next.entries[0].plan_id, "b-plan");
        assert!(next.next_after.is_none());
        assert_eq!(
            list(&mut connection, &other, None, 10)
                .expect("scoped page")
                .entries
                .len(),
            1
        );
        assert!(
            get(&mut connection, &project(), "missing", None)
                .expect("missing plan")
                .is_none()
        );
        assert!(list(&mut connection, &project(), None, 0).is_err());
        let mut cyclic = request("cycle", "cycle", 1, vec![]);
        let plan = Workplan {
            schema_version: 1,
            plan_id: "cycle".into(),
            revision: 1,
            project_id: project(),
            tasks: vec![contract("x", &["y"]), contract("y", &["x"])],
        };
        cyclic.source = serde_json::to_vec(&plan).expect("cyclic source");
        assert!(matches!(
            register(&mut connection, cyclic),
            Err(WorkplanStoreError::Parser(WorkplanError::DependencyCycle))
        ));
    }

    #[test]
    fn two_connection_cas_race_commits_exactly_one_new_revision() {
        let directory = tempfile::tempdir().expect("race directory");
        let path = directory.path().join("race.sqlite");
        let mut setup = Connection::open(&path).expect("race setup DB");
        initialize(&setup).expect("race schema");
        register(
            &mut setup,
            request("first", "plan", 1, vec![contract("a", &[])]),
        )
        .expect("race baseline");
        drop(setup);
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for command in ["racer-one", "racer-two"] {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                let mut connection = Connection::open(path).expect("race connection");
                connection
                    .busy_timeout(Duration::from_secs(1))
                    .expect("bounded race busy");
                connection
                    .pragma_update(None, "foreign_keys", true)
                    .expect("race FK");
                barrier.wait();
                register(
                    &mut connection,
                    request(command, "plan", 2, vec![contract("a", &[])]),
                )
            }));
        }
        barrier.wait();
        let mut success = 0;
        for worker in workers {
            match worker.join().expect("race worker") {
                Ok(_) => success += 1,
                Err(WorkplanStoreError::StaleCurrent) => {}
                Err(error) => panic!("unexpected race result: {error}"),
            }
        }
        assert_eq!(success, 1);
        let mut connection = Connection::open(&path).expect("reopen race");
        initialize(&connection).expect("race reopen init");
        assert_eq!(
            get(&mut connection, &project(), "plan", None)
                .expect("read race current")
                .expect("race current")
                .plan
                .revision,
            2
        );
        assert_eq!(counts(&connection), (1, 2, 1));
    }

    #[test]
    fn initialization_bounds_schema_and_history_never_eviction_or_hidden_effects() {
        let mut connection = database();
        connection
            .pragma_update(None, "user_version", 77)
            .expect("external schema version");
        initialize(&connection).expect("idempotent registry init");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged user version"),
            77
        );
        let mut too_large = request("large", "plan", 1, vec![]);
        too_large.source = vec![b' '; MAX_WORKPLAN_BYTES + 1];
        assert!(matches!(
            register(&mut connection, too_large),
            Err(WorkplanStoreError::TooLarge)
        ));
        connection.execute_batch("CREATE TEMP TABLE workplan_registry_plans(marker TEXT);INSERT INTO temp.workplan_registry_plans VALUES('preserve');CREATE TABLE domain_events(marker TEXT)").expect("unrelated tables");
        let first = request("revision-1", "plan", 1, vec![]);
        register(&mut connection, first.clone()).expect("history first");
        for number in 2..=MAX_REVISIONS_PER_PLAN as u32 {
            register(
                &mut connection,
                request(&format!("revision-{number}"), "plan", number, vec![]),
            )
            .expect("bounded history revision");
        }
        assert!(matches!(
            register(&mut connection, request("full", "plan", 65, vec![])),
            Err(WorkplanStoreError::Capacity)
        ));
        assert_eq!(
            register(&mut connection, first)
                .expect("historical replay at capacity")
                .revision,
            "1"
        );
        assert_eq!(
            get(&mut connection, &project(), "plan", None)
                .expect("capacity current")
                .expect("current")
                .plan
                .revision,
            64
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT marker FROM temp.workplan_registry_plans",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("temp shadow untouched"),
            "preserve"
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM domain_events", [], |row| row
                    .get::<_, i64>(0))
                .expect("no task effects"),
            0
        );
        connection
            .pragma_update(None, "foreign_keys", false)
            .expect("disable fixture FK");
        assert!(matches!(
            get(&mut connection, &project(), "plan", None),
            Err(WorkplanStoreError::Schema)
        ));
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("restore fixture FK");
        connection
            .busy_timeout(Duration::from_secs(6))
            .expect("unbounded-for-module busy fixture");
        assert!(matches!(
            get(&mut connection, &project(), "plan", None),
            Err(WorkplanStoreError::Schema)
        ));
        connection
            .busy_timeout(Duration::from_secs(1))
            .expect("restore bounded busy");
        connection
            .execute_batch("BEGIN")
            .expect("caller transaction");
        assert!(matches!(
            get(&mut connection, &project(), "plan", None),
            Err(WorkplanStoreError::Schema)
        ));
        connection
            .execute_batch("ROLLBACK")
            .expect("caller rollback");
    }
}
