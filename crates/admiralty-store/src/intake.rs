//! WO2: bounded immutable work-order records on Store's existing sole writer.
//! No task events, dispatch, outbox, provider or authority grants. Inline original
//! source is supported; ArtifactClaim fails closed until verified pin composition.
use crate::{
    stored_text,
    workplans::{self, RegisterWorkplanRequest, WorkplanStoreError},
};
use admiralty_core::{
    ProjectId, Projection, RootId, TaskId, content_digest,
    intake::{
        self, AuthorizedIntakeScope, DuplicateCandidate, DuplicateCoverage, DuplicatePage,
        DuplicateQuery, DuplicateReason, IntakePolicy, IntakePreview, RecordIntakeReceipt,
        RecordIntakeRequest, SourcePayload, WorkOrderRevision,
    },
    limits::bounded_json_size,
    workplans::{Workplan, preview_workplan},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const MAX_ORDERS: usize = 256;
const MAX_REVISIONS: usize = 64;
const MAX_ROWS: usize = MAX_ORDERS * MAX_REVISIONS;
const MAX_ROW_BYTES: usize = 1024 * 1024;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
type Result<T> = std::result::Result<T, IntakeStoreError>;
type RevisionRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u16,
    String,
    u32,
);
type SourceRow = (String, String, String, u16, String, String, Option<String>);
#[derive(Debug, thiserror::Error)]
pub enum IntakeStoreError {
    #[error("intake SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("intake JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] intake::IntakeError),
    #[error(transparent)]
    Workplan(#[from] WorkplanStoreError),
    #[error("current independent intake authorization is absent or mismatched")]
    Denied,
    #[error("verified artifact and interlocked pin composition is unavailable")]
    UnsupportedSource,
    #[error("explicit reviewed outcome/task/source-span selection is incomplete or differs")]
    Selection,
    #[error("immutable intake identity has conflicting content")]
    Collision,
    #[error("work-order or plan compare-and-swap is stale")]
    StaleCurrent,
    #[error("bounded intake history is full; nothing was evicted")]
    Capacity,
    #[error("stored intake consistency failed")]
    Integrity,
}
/// Supplied by independently installed current service authorization, never JSON.
pub struct AuthorizedIntakeContext {
    pub scope: AuthorizedIntakeScope,
    pub policy: IntakePolicy,
}
/// Installed at trusted Store startup. No default issuer, setter or request-level
/// self-issued policy. The writer invokes this for EVERY command, including retry.
pub trait IntakeAuthority: Send + Sync {
    fn authorize(&self, request: &RecordIntakeRequest) -> Result<AuthorizedIntakeContext>;
}
pub(crate) fn validate_record_request(request: &RecordIntakeRequest) -> Result<()> {
    request.validate()?;
    encoded(request, MAX_ROW_BYTES)?;
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntakeRecordResult {
    pub receipt: RecordIntakeReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegisteredWorkOrder {
    pub work_order: WorkOrderRevision,
    pub receipt: RecordIntakeReceipt,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicySnapshot {
    revision: String,
    max_source_bytes: usize,
    max_requests: usize,
    max_outcomes: usize,
    max_json_item_text_bytes: usize,
}
impl PolicySnapshot {
    fn capture(p: &IntakePolicy) -> Self {
        Self {
            revision: p.revision.clone(),
            max_source_bytes: p.max_source_bytes,
            max_requests: p.max_requests,
            max_outcomes: p.max_outcomes,
            max_json_item_text_bytes: p.max_json_item_text_bytes,
        }
    }
    fn policy(&self) -> IntakePolicy {
        IntakePolicy {
            revision: self.revision.clone(),
            max_source_bytes: self.max_source_bytes,
            max_requests: self.max_requests,
            max_outcomes: self.max_outcomes,
            max_json_item_text_bytes: self.max_json_item_text_bytes,
        }
    }
}
fn encoded<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<String> {
    bounded_json_size(value, limit).map_err(|_| IntakeStoreError::Capacity)?;
    Ok(serde_json::to_string(value)?)
}
fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(content_digest(value)?)
}
fn ids(scope: &AuthorizedIntakeScope) -> Result<(&ProjectId, &RootId)> {
    // Bound before any clone, allocation or SQLite parameter binding. This is
    // independently supplied scope, but still must obey the intake reference
    // contract; a typed context is not permission to consume unbounded work.
    let reference = &scope.authority_ref;
    if reference.len() > 1024
        || reference.trim().is_empty()
        || reference
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || reference.contains(['/', '\\'])
        || reference.starts_with('.')
        || reference.contains("..")
        || ["javascript:", "data:", "file:", "http:", "https:", "ssh:"]
            .iter()
            .any(|prefix| reference.to_ascii_lowercase().starts_with(prefix))
    {
        return Err(IntakeStoreError::Denied);
    }
    match (&scope.project_id, &scope.root_id) {
        (Some(p), Some(r)) if scope.authority_generation > 0 => Ok((p, r)),
        _ => Err(IntakeStoreError::Denied),
    }
}
fn known_scope(request: &RecordIntakeRequest, ctx: &AuthorizedIntakeContext) -> Result<()> {
    let (project, root) = ids(&ctx.scope)?;
    let snapshot = &request.work_order.scope;
    if snapshot.principal != ctx.scope.principal
        || snapshot.project_id.as_ref() != Some(project)
        || snapshot.root_id.as_ref() != Some(root)
        || snapshot.authority_ref != ctx.scope.authority_ref
        || snapshot.authority_generation != ctx.scope.authority_generation
        || request.authority_generation != ctx.scope.authority_generation
    {
        return Err(IntakeStoreError::Denied);
    }
    Ok(())
}
struct Prepared {
    request_json: String,
    request_digest: String,
    preview: IntakePreview,
    index: u16,
    policy_json: String,
    source_key: String,
    origin_key: Option<String>,
}
fn prepare(request: &RecordIntakeRequest, ctx: &AuthorizedIntakeContext) -> Result<Prepared> {
    request.validate_with_policy(&ctx.policy)?;
    known_scope(request, ctx)?;
    let request_json = encoded(request, MAX_ROW_BYTES)?;
    let raw = match &request.work_order.source_payload {
        SourcePayload::InlineUtf8 { text } => text.as_bytes(),
        SourcePayload::ArtifactClaim { .. } => return Err(IntakeStoreError::UnsupportedSource),
    };
    let preview = intake::preview_intake(
        raw,
        request.work_order.format,
        &request.work_order.provenance,
        &ctx.scope,
        &ctx.policy,
    )?;
    if preview.manifest_digest != request.preview_manifest_digest
        || preview.raw_digest != request.work_order.raw_digest
        || preview.canonical_digest != request.work_order.canonical_digest
        || preview.parser_revision != request.work_order.parser_revision
        || preview.policy_digest != request.policy_digest
    {
        return Err(IntakeStoreError::Selection);
    }
    let index = request
        .selected_tasks
        .first()
        .ok_or(IntakeStoreError::Selection)?
        .binding
        .request_index;
    let item = preview
        .requests
        .get(usize::from(index))
        .ok_or(IntakeStoreError::Selection)?;
    if item
        .claimed_project_id
        .as_ref()
        .is_some_and(|p| Some(p) != ctx.scope.project_id.as_ref())
        || item.title != request.work_order.title
        || item.rationale != request.work_order.rationale
        || item.constraints != request.work_order.constraints
        || item.requested_timing != request.work_order.requested_timing
    {
        return Err(IntakeStoreError::Selection);
    }
    if let Some(outcome) = &request.work_order.outcome
        && (request.selected_tasks.len() != 1
            || item
                .outcomes
                .iter()
                .find(|o| o.outcome.id == request.selected_tasks[0].binding.outcome_id)
                .is_none_or(|o| &o.outcome.description != outcome))
    {
        return Err(IntakeStoreError::Selection);
    }
    for selected in &request.selected_tasks {
        let b = &selected.binding;
        let outcome = item
            .outcomes
            .iter()
            .find(|o| o.outcome.id == b.outcome_id)
            .ok_or(IntakeStoreError::Selection)?;
        if b.request_index != index
            || b.outcome_digest != outcome.outcome_digest
            || b.source_spans != outcome.outcome.source_spans
            || outcome.outcome.task.as_ref() != Some(&selected.task)
            || outcome.outcome.delivery.as_ref() != Some(&selected.delivery)
            || outcome.contract_digest.as_ref() != Some(&b.contract_digest)
            || outcome.outcome.source_spans.is_empty()
        {
            return Err(IntakeStoreError::Selection);
        }
    }
    let (project, root) = ids(&ctx.scope)?;
    let provenance = &request.work_order.provenance;
    let source_key = hash(&(
        "wo2-source-item-v1",
        project,
        root,
        &provenance.source_id,
        &provenance.revision,
        index,
    ))?;
    let origin_key = provenance
        .origin
        .as_ref()
        .map(|origin| hash(&("wo2-origin-item-v1", project, root, origin, index)))
        .transpose()?;
    let policy_json = encoded(&PolicySnapshot::capture(&ctx.policy), 8192)?;
    Ok(Prepared {
        request_digest: hash(request)?,
        request_json,
        preview,
        index,
        policy_json,
        source_key,
        origin_key,
    })
}
fn bounds(c: &Connection) -> Result<()> {
    let (orders,revisions,bytes):(i64,i64,i64)=c.query_row("SELECT (SELECT count(*) FROM work_order_intake_orders),(SELECT count(*) FROM work_order_intake_revisions),COALESCE((SELECT sum(length(CAST(request_json AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(policy_json AS BLOB))) FROM work_order_intake_revisions),0)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let (bindings,binding_bytes):(i64,i64)=c.query_row("SELECT count(*),COALESCE(sum(length(CAST(binding_json AS BLOB))+length(CAST(delivery_json AS BLOB))+length(CAST(outcome_json AS BLOB))),0) FROM work_order_intake_bindings",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let sources: i64 = c.query_row("SELECT count(*) FROM work_order_intake_sources", [], |r| {
        r.get(0)
    })?;
    if orders < 0
        || orders as usize > MAX_ORDERS
        || revisions < 0
        || revisions as usize > MAX_ROWS
        || sources < 0
        || sources > revisions
        || bytes < 0
        || bytes as usize > MAX_HISTORY_BYTES
        || bindings < 0
        || bindings as usize > 20_000
        || binding_bytes < 0
        || binding_bytes as usize > MAX_HISTORY_BYTES
    {
        return Err(IntakeStoreError::Capacity);
    }
    Ok(())
}
pub(crate) fn initialize(c: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("../migrations/0006_work_order_intake.sql"))?;
    tx.commit()?;
    validate_all(c)
}
fn scope_from(request: &RecordIntakeRequest, policy: &PolicySnapshot) -> AuthorizedIntakeContext {
    let s = &request.work_order.scope;
    AuthorizedIntakeContext {
        scope: AuthorizedIntakeScope {
            principal: s.principal.clone(),
            project_id: s.project_id.clone(),
            root_id: s.root_id.clone(),
            authority_ref: s.authority_ref.clone(),
            authority_generation: s.authority_generation,
        },
        policy: policy.policy(),
    }
}
fn current(
    tx: &Transaction<'_>,
    project: &ProjectId,
    root: &RootId,
    id: &str,
) -> Result<Option<u32>> {
    let head:Option<u32>=tx.query_row("SELECT head_revision FROM work_order_intake_orders WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3",params![project.as_str(),root.as_str(),id],|r|r.get(0)).optional()?;
    let (count,max):(i64,Option<u32>)=tx.query_row("SELECT count(*),max(revision) FROM work_order_intake_revisions WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3",params![project.as_str(),root.as_str(),id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if head.is_none() && count == 0 {
        return Ok(None);
    }
    if head != max
        || head.is_none()
        || head.is_some_and(|h| h == 0 || h as usize > MAX_REVISIONS || i64::from(h) != count)
    {
        return Err(IntakeStoreError::Integrity);
    }
    Ok(head)
}
fn expected_receipt(request: &RecordIntakeRequest) -> Result<RecordIntakeReceipt> {
    Ok(RecordIntakeReceipt {
        schema_version: 1,
        command_id: request.command_id.clone(),
        request_digest: content_digest(request)?,
        work_order_id: request.work_order.id.clone(),
        work_order_revision: request.work_order.revision,
        plan_id: request.plan_id.clone(),
        plan_revision: request
            .expected_plan_revision
            .checked_add(1)
            .ok_or(IntakeStoreError::Capacity)?,
        preview_manifest_digest: request.preview_manifest_digest.clone(),
        task_bindings: request
            .selected_tasks
            .iter()
            .map(|s| s.binding.clone())
            .collect(),
        duplicate: false,
    })
}
struct Stored {
    request: RecordIntakeRequest,
    receipt: RecordIntakeReceipt,
}
fn read_revision(
    tx: &Transaction<'_>,
    project: &ProjectId,
    root: &RootId,
    id: &str,
    revision: u32,
) -> Result<Option<Stored>> {
    let row:Option<RevisionRow>=tx.query_row("SELECT request_json,receipt_json,policy_json,request_digest,preview_digest,raw_digest,canonical_digest,source_key,request_index,plan_id,plan_revision FROM work_order_intake_revisions WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3 AND revision=?4",params![project.as_str(),root.as_str(),id,revision],|r|Ok((stored_text(r,0,MAX_ROW_BYTES)?,stored_text(r,1,MAX_ROW_BYTES)?,stored_text(r,2,8192)?,stored_text(r,3,71)?,stored_text(r,4,71)?,stored_text(r,5,71)?,stored_text(r,6,71)?,stored_text(r,7,71)?,r.get(8)?,stored_text(r,9,128)?,r.get(10)?))).optional()?;
    let Some((
        request_json,
        receipt_json,
        policy_json,
        request_digest,
        preview_digest,
        raw_digest,
        canonical_digest,
        source_key,
        index,
        plan_id,
        plan_revision,
    )) = row
    else {
        return Ok(None);
    };
    let request = intake::parse_record_request(request_json.as_bytes())?;
    let policy: PolicySnapshot = serde_json::from_str(&policy_json)?;
    let prepared = prepare(&request, &scope_from(&request, &policy))?;
    let metadata:(String,String,String,String)=tx.query_row("SELECT command_id,principal_id,authority_ref,authority_generation FROM work_order_intake_revisions WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3 AND revision=?4",params![project.as_str(),root.as_str(),id,revision],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,128)?,stored_text(r,2,1024)?,stored_text(r,3,20)?)))?;
    if metadata
        != (
            request.command_id.to_string(),
            request.work_order.scope.principal.to_string(),
            request.work_order.scope.authority_ref.clone(),
            request.work_order.scope.authority_generation.to_string(),
        )
    {
        return Err(IntakeStoreError::Integrity);
    }

    let receipt: RecordIntakeReceipt = serde_json::from_str(&receipt_json)?;
    receipt.validate()?;
    if prepared.request_json != request_json
        || prepared.policy_json != policy_json
        || prepared.request_digest != request_digest
        || prepared.preview.manifest_digest != preview_digest
        || prepared.preview.raw_digest != raw_digest
        || prepared.preview.canonical_digest != canonical_digest
        || prepared.source_key != source_key
        || prepared.index != index
        || request.work_order.id != id
        || request.work_order.revision != revision
        || request.work_order.scope.project_id.as_ref() != Some(project)
        || request.work_order.scope.root_id.as_ref() != Some(root)
        || receipt != expected_receipt(&request)?
        || encoded(&receipt, MAX_ROW_BYTES)? != receipt_json
        || receipt.plan_id != plan_id
        || receipt.plan_revision != plan_revision
    {
        return Err(IntakeStoreError::Integrity);
    }
    let plan = workplans::get_in_transaction(tx, project, &plan_id, Some(plan_revision))?
        .ok_or(IntakeStoreError::Integrity)?;
    for selected in &request.selected_tasks {
        if plan
            .plan
            .tasks
            .iter()
            .find(|t| t.task_id == selected.task.task_id)
            != Some(&selected.task)
        {
            return Err(IntakeStoreError::Integrity);
        }
    }
    validate_source(tx, &request, &prepared, false)?;
    let count:i64=tx.query_row("SELECT count(*) FROM work_order_intake_bindings WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3 AND revision=?4",params![project.as_str(),root.as_str(),id,revision],|r|r.get(0))?;
    if count != request.selected_tasks.len() as i64 {
        return Err(IntakeStoreError::Integrity);
    }
    for selected in &request.selected_tasks {
        let outcome = &prepared.preview.requests[usize::from(index)]
            .outcomes
            .iter()
            .find(|o| o.outcome.id == selected.binding.outcome_id)
            .ok_or(IntakeStoreError::Integrity)?
            .outcome;
        let raw:Option<(String,String,String,String,String)>=tx.query_row("SELECT task_id,contract_digest,binding_json,delivery_json,outcome_json FROM work_order_intake_bindings WHERE project_id=?1 AND root_id=?2 AND work_order_id=?3 AND revision=?4 AND outcome_id=?5",params![project.as_str(),root.as_str(),id,revision,selected.binding.outcome_id],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,71)?,stored_text(r,2,32768)?,stored_text(r,3,262144)?,stored_text(r,4,262144)?))).optional()?;
        if raw
            != Some((
                selected.task.task_id.to_string(),
                selected.binding.contract_digest.clone(),
                encoded(&selected.binding, 32768)?,
                encoded(&selected.delivery, 262144)?,
                encoded(outcome, 262144)?,
            ))
        {
            return Err(IntakeStoreError::Integrity);
        }
    }
    Ok(Some(Stored { request, receipt }))
}
fn validate_source(
    tx: &Transaction<'_>,
    request: &RecordIntakeRequest,
    p: &Prepared,
    insert: bool,
) -> Result<()> {
    let project = request
        .work_order
        .scope
        .project_id
        .as_ref()
        .ok_or(IntakeStoreError::Denied)?;
    let root = request
        .work_order
        .scope
        .root_id
        .as_ref()
        .ok_or(IntakeStoreError::Denied)?;
    let row:Option<SourceRow>=tx.query_row("SELECT project_id,root_id,work_order_id,request_index,raw_digest,canonical_digest,origin_key FROM work_order_intake_sources WHERE source_key=?1",[&p.source_key],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,128)?,stored_text(r,2,128)?,r.get(3)?,stored_text(r,4,71)?,stored_text(r,5,71)?,if matches!(r.get_ref(6)?,rusqlite::types::ValueRef::Null){None}else{Some(stored_text(r,6,71)?)}))).optional()?;
    let expected = (
        project.to_string(),
        root.to_string(),
        request.work_order.id.clone(),
        p.index,
        request.work_order.raw_digest.clone(),
        request.work_order.canonical_digest.clone(),
        p.origin_key.clone(),
    );
    match row {
        Some(row) if row == expected => Ok(()),
        Some(_) => Err(IntakeStoreError::Collision),
        None if insert => {
            if let Some(origin) = &p.origin_key {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM work_order_intake_sources WHERE origin_key=?1)",
                    [origin],
                    |r| r.get(0),
                )?;
                if exists {
                    return Err(IntakeStoreError::Collision);
                }
            }
            tx.execute("INSERT INTO work_order_intake_sources(source_key,origin_key,project_id,root_id,work_order_id,request_index,raw_digest,canonical_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![p.source_key,p.origin_key,project.as_str(),root.as_str(),request.work_order.id,p.index,request.work_order.raw_digest,request.work_order.canonical_digest])?;
            Ok(())
        }
        None => Err(IntakeStoreError::Integrity),
    }
}
pub(crate) fn record(
    c: &mut Connection,
    request: RecordIntakeRequest,
    ctx: AuthorizedIntakeContext,
    existing: &BTreeMap<TaskId, String>,
) -> Result<IntakeRecordResult> {
    let prepared = prepare(&request, &ctx)?;
    let (project, root) = ids(&ctx.scope)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    bounds(&tx)?;
    let prior:Option<(String,String,String,u32,String)>=tx.query_row("SELECT project_id,root_id,work_order_id,revision,request_digest FROM work_order_intake_revisions WHERE command_id=?1",[request.command_id.as_str()],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,128)?,stored_text(r,2,128)?,r.get(3)?,stored_text(r,4,71)?))).optional()?;
    if let Some((p, r, id, revision, digest)) = prior {
        if p != project.as_str() || r != root.as_str() || digest != prepared.request_digest {
            return Err(IntakeStoreError::Collision);
        }
        validate_order(&tx, project, root, &id)?;
        let stored =
            read_revision(&tx, project, root, &id, revision)?.ok_or(IntakeStoreError::Integrity)?;
        if stored.request != request {
            return Err(IntakeStoreError::Collision);
        }
        let receipt_digest = hash(&stored.receipt)?;
        tx.commit()?;
        return Ok(IntakeRecordResult {
            receipt: stored.receipt,
            receipt_digest,
            replayed: true,
        });
    }
    let head = validate_order(&tx, project, root, &request.work_order.id)?.unwrap_or(0);
    if head != request.expected_work_order_revision {
        return Err(IntakeStoreError::StaleCurrent);
    }
    if request.work_order.revision as usize > MAX_REVISIONS {
        return Err(IntakeStoreError::Capacity);
    }
    let orders: i64 = tx.query_row("SELECT count(*) FROM work_order_intake_orders", [], |r| {
        r.get(0)
    })?;
    if head == 0 && orders as usize >= MAX_ORDERS {
        return Err(IntakeStoreError::Capacity);
    }
    let old = workplans::get_in_transaction(&tx, project, &request.plan_id, None)?;
    let actual = old.as_ref().map_or(0, |p| p.plan.revision);
    if actual != request.expected_plan_revision {
        return Err(IntakeStoreError::StaleCurrent);
    }
    let mut tasks = old.map_or_else(Vec::new, |p| p.plan.tasks);
    for selected in &request.selected_tasks {
        if let Some(old) = tasks.iter().find(|t| t.task_id == selected.task.task_id) {
            if old != &selected.task {
                return Err(IntakeStoreError::Collision);
            }
        } else {
            tasks.push(selected.task.clone());
        }
    }
    let next = request
        .expected_plan_revision
        .checked_add(1)
        .ok_or(IntakeStoreError::Capacity)?;
    let plan = Workplan {
        schema_version: 1,
        plan_id: request.plan_id.clone(),
        revision: next,
        project_id: project.clone(),
        tasks,
    };
    let source = encoded(&plan, MAX_ROW_BYTES)?.into_bytes();
    let preview = preview_workplan(&source).map_err(WorkplanStoreError::from)?;
    let plan_request = RegisterWorkplanRequest {
        command_id: format!(
            "wo2-{}",
            prepared
                .request_digest
                .strip_prefix("sha256:")
                .ok_or(IntakeStoreError::Integrity)?
        ),
        project_id: project.clone(),
        plan_id: request.plan_id.clone(),
        expected_current_revision: actual.to_string(),
        source,
        source_digest: preview.source_digest,
        canonical_digest: content_digest(&plan)?,
    };
    if head == 0 {
        tx.execute("INSERT INTO work_order_intake_orders(project_id,root_id,work_order_id,head_revision) VALUES(?1,?2,?3,?4)",params![project.as_str(),root.as_str(),request.work_order.id,request.work_order.revision])?;
    }
    validate_source(&tx, &request, &prepared, true)?;
    workplans::register_in_transaction(&tx, plan_request, existing)?;
    let receipt = expected_receipt(&request)?;
    let receipt_json = encoded(&receipt, MAX_ROW_BYTES)?;
    tx.execute("INSERT INTO work_order_intake_revisions(project_id,root_id,work_order_id,revision,command_id,principal_id,authority_ref,authority_generation,request_digest,preview_digest,raw_digest,canonical_digest,source_key,request_index,plan_id,plan_revision,request_json,receipt_json,policy_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",params![project.as_str(),root.as_str(),request.work_order.id,request.work_order.revision,request.command_id.as_str(),ctx.scope.principal.as_str(),ctx.scope.authority_ref,ctx.scope.authority_generation.to_string(),prepared.request_digest,request.preview_manifest_digest,request.work_order.raw_digest,request.work_order.canonical_digest,prepared.source_key,prepared.index,request.plan_id,next,prepared.request_json,receipt_json,prepared.policy_json])?;
    for selected in &request.selected_tasks {
        let outcome = &prepared.preview.requests[usize::from(prepared.index)]
            .outcomes
            .iter()
            .find(|o| o.outcome.id == selected.binding.outcome_id)
            .ok_or(IntakeStoreError::Selection)?
            .outcome;
        tx.execute("INSERT INTO work_order_intake_bindings(project_id,root_id,work_order_id,revision,outcome_id,task_id,contract_digest,binding_json,delivery_json,outcome_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![project.as_str(),root.as_str(),request.work_order.id,request.work_order.revision,selected.binding.outcome_id,selected.task.task_id.as_str(),selected.binding.contract_digest,encoded(&selected.binding,32768)?,encoded(&selected.delivery,262144)?,encoded(outcome,262144)?])?;
    }
    if head!=0&&tx.execute("UPDATE work_order_intake_orders SET head_revision=?1 WHERE project_id=?2 AND root_id=?3 AND work_order_id=?4 AND head_revision=?5",params![request.work_order.revision,project.as_str(),root.as_str(),request.work_order.id,head])?!=1{return Err(IntakeStoreError::StaleCurrent);}
    bounds(&tx)?;
    validate_order(&tx, project, root, &request.work_order.id)?;
    let receipt_digest = hash(&receipt)?;
    tx.commit()?;
    Ok(IntakeRecordResult {
        receipt,
        receipt_digest,
        replayed: false,
    })
}
fn validate_order(
    tx: &Transaction<'_>,
    project: &ProjectId,
    root: &RootId,
    id: &str,
) -> Result<Option<u32>> {
    let head = current(tx, project, root, id)?;
    if let Some(head) = head {
        for revision in 1..=head {
            read_revision(tx, project, root, id, revision)?.ok_or(IntakeStoreError::Integrity)?;
        }
    }
    Ok(head)
}
pub(crate) fn get_work_order(
    c: &mut Connection,
    scope: &AuthorizedIntakeScope,
    id: &str,
    revision: Option<u32>,
) -> Result<Option<RegisteredWorkOrder>> {
    let (project, root) = ids(scope)?;
    // TaskId::new(&str) owns a copy before checking its own cap. Reject a large
    // caller-owned identifier here, before that otherwise unnecessary copy.
    if id.is_empty() || id.len() > 128 {
        return Err(IntakeStoreError::Selection);
    }
    TaskId::new(id).map_err(|_| IntakeStoreError::Selection)?;
    if revision == Some(0) {
        return Err(IntakeStoreError::Selection);
    }
    let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
    bounds(&tx)?;
    let readable:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM work_order_intake_orders h JOIN work_order_intake_revisions r ON r.project_id=h.project_id AND r.root_id=h.root_id AND r.work_order_id=h.work_order_id AND r.revision=COALESCE(?4,h.head_revision) WHERE h.project_id=?1 AND h.root_id=?2 AND h.work_order_id=?3 AND r.principal_id=?5 AND r.authority_ref=?6 AND r.authority_generation=?7)",params![project.as_str(),root.as_str(),id,revision,scope.principal.as_str(),scope.authority_ref,scope.authority_generation.to_string()],|r|r.get(0))?;
    if !readable {
        return Ok(None);
    }
    let Some(head) = validate_order(&tx, project, root, id)? else {
        return Ok(None);
    };
    let result = read_revision(&tx, project, root, id, revision.unwrap_or(head))?.map(|s| {
        RegisteredWorkOrder {
            work_order: s.request.work_order,
            receipt: s.receipt,
        }
    });
    tx.commit()?;
    Ok(result)
}
pub(crate) fn validate_all(c: &Connection) -> Result<()> {
    bounds(c)?;
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Deferred)?;
    let orphan:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM work_order_intake_sources s LEFT JOIN work_order_intake_revisions r USING(source_key) WHERE r.source_key IS NULL)",[],|r|r.get(0))?;
    if orphan {
        return Err(IntakeStoreError::Integrity);
    }
    let mut q=tx.prepare("SELECT project_id,root_id,work_order_id FROM work_order_intake_orders ORDER BY project_id,root_id,work_order_id LIMIT 257")?;
    let mut rows = q.query([])?;
    let mut count = 0;
    while let Some(r) = rows.next()? {
        count += 1;
        if count > MAX_ORDERS {
            return Err(IntakeStoreError::Capacity);
        }
        let project =
            ProjectId::new(stored_text(r, 0, 128)?).map_err(|_| IntakeStoreError::Integrity)?;
        let root = RootId::new(stored_text(r, 1, 128)?).map_err(|_| IntakeStoreError::Integrity)?;
        let id = stored_text(r, 2, 128)?;
        validate_order(&tx, &project, &root, &id)?;
    }
    drop(rows);
    drop(q);
    for table in [
        "work_order_intake_orders",
        "work_order_intake_sources",
        "work_order_intake_revisions",
        "work_order_intake_bindings",
    ] {
        if tx
            .prepare(&format!("PRAGMA foreign_key_check({table})"))?
            .exists([])?
        {
            return Err(IntakeStoreError::Integrity);
        }
    }
    tx.commit()?;
    Ok(())
}

/// Bounded exact/advisory history lookup, never a similarity engine or grant.
/// Older tasks without intake provenance are explicitly not exhaustively covered.
pub(crate) fn find_duplicate_candidates(
    c: &mut Connection,
    scope: &AuthorizedIntakeScope,
    query: &DuplicateQuery,
    projection: &Projection,
) -> Result<DuplicatePage> {
    query.validate()?;
    let (project, root) = ids(scope)?;
    if &query.project_id != project {
        return Err(IntakeStoreError::Denied);
    }
    let cursor = if let Some(cursor) = &query.cursor {
        let parts: Vec<_> = cursor.split(':').collect();
        if parts.len() != 3 {
            return Err(IntakeStoreError::Selection);
        }
        TaskId::new(parts[0]).map_err(|_| IntakeStoreError::Selection)?;
        TaskId::new(parts[2]).map_err(|_| IntakeStoreError::Selection)?;
        let revision = parts[1]
            .parse::<u32>()
            .map_err(|_| IntakeStoreError::Selection)?;
        if revision == 0 || revision.to_string() != parts[1] {
            return Err(IntakeStoreError::Selection);
        }
        (parts[0].to_owned(), revision, parts[2].to_owned())
    } else {
        (String::new(), 0, String::new())
    };
    let origin = query
        .origin
        .as_ref()
        .map(|o| hash(&("wo2-origin-query-v1", project, root, o)))
        .transpose()?;
    // Origin identities in records include request-item index. Compare the exact
    // bounded structured origin from original source below, not hash-prefix guesses.
    let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
    bounds(&tx)?;
    let mut statement=tx.prepare("SELECT r.work_order_id,r.revision,b.outcome_id FROM work_order_intake_revisions r JOIN work_order_intake_bindings b USING(project_id,root_id,work_order_id,revision) WHERE r.project_id=?1 AND r.root_id=?2 AND (r.work_order_id,r.revision,b.outcome_id)>(?3,?4,?5) AND (r.raw_digest=?6 OR r.canonical_digest=?7 OR r.command_id=?8 OR ?9 IS NOT NULL) AND r.principal_id=?10 AND r.authority_ref=?11 AND r.authority_generation=?12 ORDER BY r.work_order_id,r.revision,b.outcome_id LIMIT ?13")?;
    // A non-null origin enables a bounded scan, never an unbounded all-state pass.
    let mut rows = statement.query(params![
        project.as_str(),
        root.as_str(),
        cursor.0,
        cursor.1,
        cursor.2,
        query.raw_digest,
        query.canonical_digest,
        query.command_id.as_ref().map(|c| c.as_str()),
        origin,
        scope.principal.as_str(),
        scope.authority_ref,
        scope.authority_generation.to_string(),
        i64::from(query.limit) + 1
    ])?;
    let mut candidates = Vec::new();
    // Each page scans at most query.limit records, so this cache has at most
    // that many entries. A revision shared by several selected outcomes is
    // validated once, including its original source, policy, receipt, plan and
    // exact binding rows. Lookup columns are selection hints, never proof.
    let mut validated = BTreeMap::new();
    let mut last = None;
    let mut scanned = 0;
    let mut more = false;
    while let Some(row) = rows.next()? {
        scanned += 1;
        if scanned > usize::from(query.limit) {
            more = true;
            break;
        }
        let wo = stored_text(row, 0, 128)?;
        let revision: u32 = row.get(1)?;
        let outcome = stored_text(row, 2, 128)?;
        let stored = match validated.entry((wo.clone(), revision)) {
            std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                read_revision(&tx, project, root, &wo, revision)?
                    .ok_or(IntakeStoreError::Integrity)?,
            ),
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        };
        let original = &stored.request;
        let original_scope = &original.work_order.scope;
        if original_scope.principal != scope.principal
            || original_scope.project_id.as_ref() != Some(project)
            || original_scope.root_id.as_ref() != Some(root)
            || original_scope.authority_ref != scope.authority_ref
            || original_scope.authority_generation != scope.authority_generation
            || original.authority_generation != scope.authority_generation
        {
            return Err(IntakeStoreError::Integrity);
        }
        let binding = &original
            .selected_tasks
            .iter()
            .find(|selected| selected.binding.outcome_id == outcome)
            .ok_or(IntakeStoreError::Integrity)?
            .binding;
        let id = binding.task_id.clone();
        let contract_digest = binding.contract_digest.clone();
        last = Some(format!("{wo}:{revision}:{outcome}"));
        let command = original.command_id.as_str();
        let raw = &original.work_order.raw_digest;
        let canonical = &original.work_order.canonical_digest;
        let exact_command = query
            .command_id
            .as_ref()
            .is_some_and(|c| c.as_str() == command);
        let exact_origin = query
            .origin
            .as_ref()
            .is_some_and(|origin| original.work_order.provenance.origin.as_ref() == Some(origin));
        let equal = raw == &query.raw_digest && canonical == &query.canonical_digest;
        let reason = if exact_command {
            if equal {
                DuplicateReason::ExactCommandIdentity
            } else {
                DuplicateReason::ExactCommandCollision
            }
        } else if exact_origin {
            if equal {
                DuplicateReason::ExactSourceIdentity
            } else {
                DuplicateReason::ExactSourceCollision
            }
        } else if raw == &query.raw_digest {
            DuplicateReason::AdvisoryRawContent
        } else if canonical == &query.canonical_digest {
            DuplicateReason::AdvisoryCanonicalContent
        } else {
            continue;
        };
        let (lifecycle, freshness) = if let Some(task) = projection.tasks.get(&id) {
            if task.contract.project_id != *project
                || task.contract.root_id != *root
                || content_digest(&task.contract)? != contract_digest
            {
                return Err(IntakeStoreError::Integrity);
            }
            (
                serde_json::to_string(&task.state)?
                    .trim_matches('"')
                    .to_owned(),
                "current_store_projection".into(),
            )
        } else {
            ("registered_only".into(), "no_lifecycle_observation".into())
        };
        candidates.push(DuplicateCandidate {
            task_id: id,
            contract_revision: binding.contract_revision,
            contract_digest,
            reason,
            lifecycle,
            freshness,
            evidence_refs: vec![format!("work-order:{wo}:{revision}")],
        });
    }
    drop(rows);
    drop(statement);
    tx.commit()?;
    let coverage = if more {
        DuplicateCoverage::Partial {
            continuation: last.ok_or(IntakeStoreError::Integrity)?,
        }
    } else {
        DuplicateCoverage::Incomplete {
            reason: "bounded intake lineage only; older task/request provenance may be absent"
                .into(),
        }
    };
    let page = DuplicatePage {
        schema_version: 1,
        query_digest: query.identity_digest()?,
        candidates,
        coverage,
    };
    page.validate_for(query)?;
    Ok(page)
}
