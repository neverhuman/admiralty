//! Immutable original request custody before enrichment. No task, plan, outbox
//! or admission effects. Public snapshots are binding data, never authorization.
use crate::{intake::AuthorizedIntakeContext, stored_text};
use admiralty_core::{
    CommandId, ProjectId, RootId, TaskId, content_digest,
    intake::{
        self, AuthorizedIntakeScope, IntakeFormat, IntakePolicy, IntakePreview, ScopeSnapshot,
        SourcePayload, SourceProvenance,
    },
    limits::bounded_json_size,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
const MAX_CAPTURES: usize = 128;
const MAX_REVISIONS: u32 = 32;
const MAX_ROWS: usize = MAX_CAPTURES * MAX_REVISIONS as usize;
const MAX_JSON: usize = 8 * 1024 * 1024;
const MAX_SMALL_JSON: usize = 8192;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MIGRATION: &str = include_str!("../migrations/0007_intake_capture.sql");
pub type Result<T> = std::result::Result<T, CaptureError>;
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("capture JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] intake::IntakeError),
    #[error("current independent capture authorization is absent or mismatched")]
    Denied,
    #[error("capture input or lineage is invalid")]
    Invalid,
    #[error("verified artifact custody and interlocked pin composition is unavailable")]
    UnsupportedSource,
    #[error("immutable command or source identity has conflicting content")]
    Collision,
    #[error("capture compare-and-swap or parent receipt is stale")]
    StaleCurrent,
    #[error("bounded capture storage is full; nothing was evicted")]
    Capacity,
    #[error("stored capture consistency failed")]
    Integrity,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaptureIntakeRequest {
    pub schema_version: u32,
    pub command_id: CommandId,
    pub capture_id: String,
    pub revision: u32,
    pub expected_revision: u32,
    pub parent_receipt_digest: Option<String>,
    pub scope: ScopeSnapshot,
    pub format: IntakeFormat,
    pub provenance: SourceProvenance,
    pub source_payload: SourcePayload,
    pub preview_manifest_digest: String,
    pub policy_revision: String,
    pub policy_digest: String,
}
/// Startup-installed current authorization. The writer calls this even on exact
/// historical retry, before looking up a receipt. No default issuer or setter.
pub trait IntakeCaptureAuthority: Send + Sync {
    fn authorize(&self, request: &CaptureIntakeRequest) -> Result<AuthorizedIntakeContext>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaptureReceipt {
    pub schema_version: u32,
    pub command_id: CommandId,
    pub capture_id: String,
    pub revision: u32,
    pub request_digest: String,
    pub raw_digest: String,
    pub canonical_digest: String,
    pub preview_manifest_digest: String,
    pub parent_receipt_digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaptureResult {
    pub receipt: CaptureReceipt,
    pub receipt_digest: String,
    pub replayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapturedIntake {
    pub request: CaptureIntakeRequest,
    pub preview: IntakePreview,
    pub receipt: CaptureReceipt,
    pub receipt_digest: String,
}
#[derive(Serialize, Deserialize)]
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
fn encoded<T: Serialize + ?Sized>(v: &T, limit: usize) -> Result<String> {
    bounded_json_size(v, limit).map_err(|_| CaptureError::Capacity)?;
    Ok(serde_json::to_string(v)?)
}
fn hash<T: Serialize>(v: &T) -> Result<String> {
    Ok(content_digest(v)?)
}
fn digest(s: &str) -> bool {
    s.len() == 71
        && s.starts_with("sha256:")
        && s[7..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn identifier(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 128 {
        return Err(CaptureError::Invalid);
    }
    TaskId::new(s).map_err(|_| CaptureError::Invalid)?;
    Ok(())
}
fn reference(s: &str) -> Result<()> {
    if s.len() > 1024
        || s.trim().is_empty()
        || s.bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || s.contains(['/', '\\'])
        || s.starts_with('.')
        || s.contains("..")
    {
        return Err(CaptureError::Invalid);
    }
    let lower = s.to_ascii_lowercase();
    if ["javascript:", "data:", "file:", "http:", "https:", "ssh:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Err(CaptureError::Invalid);
    }
    Ok(())
}
fn ids(scope: &AuthorizedIntakeScope) -> Result<(&ProjectId, &RootId)> {
    reference(&scope.authority_ref).map_err(|_| CaptureError::Denied)?;
    match (&scope.project_id, &scope.root_id) {
        (Some(p), Some(r)) if scope.authority_generation > 0 => Ok((p, r)),
        _ => Err(CaptureError::Denied),
    }
}
fn same_scope(snapshot: &ScopeSnapshot, scope: &AuthorizedIntakeScope) -> bool {
    snapshot.principal == scope.principal
        && snapshot.project_id == scope.project_id
        && snapshot.root_id == scope.root_id
        && snapshot.authority_ref == scope.authority_ref
        && snapshot.authority_generation == scope.authority_generation
}
/// Bounded prequeue validation; parsing/re-preview/current authority stay inside
/// the writer turn. A full 1MiB plain-text request fits even with JSON escaping.
pub(crate) fn validate_request(r: &CaptureIntakeRequest) -> Result<()> {
    if r.schema_version != 1
        || r.revision == 0
        || r.revision > MAX_REVISIONS
        || r.expected_revision.checked_add(1) != Some(r.revision)
        || (r.expected_revision == 0) != r.parent_receipt_digest.is_none()
        || r.parent_receipt_digest.as_ref().is_some_and(|d| !digest(d))
        || !digest(&r.preview_manifest_digest)
        || !digest(&r.policy_digest)
    {
        return Err(CaptureError::Invalid);
    }
    identifier(&r.capture_id)?;
    reference(&r.scope.authority_ref)?;
    reference(&r.policy_revision)?;
    if r.scope.authority_generation == 0 {
        return Err(CaptureError::Invalid);
    }
    let SourcePayload::InlineUtf8 { text } = &r.source_payload else {
        return Err(CaptureError::UnsupportedSource);
    };
    if text.len() > intake::MAX_SOURCE_BYTES {
        return Err(CaptureError::Capacity);
    }
    // All request-owned metadata is bounded before serialization/clone/hash.
    let p = &r.provenance;
    identifier(&p.source_id)?;
    reference(&p.revision)?;
    if p.captured_at.len() > 128 {
        return Err(CaptureError::Invalid);
    }
    if let intake::RequesterAttribution::Claimed { requester } = &p.requester
        && requester.len() > 512
    {
        return Err(CaptureError::Invalid);
    }
    if let Some(o) = &p.origin {
        identifier(&o.system)?;
        reference(&o.record_id)?;
        reference(&o.revision)?;
    }
    bounded_json_size(r, MAX_JSON).map_err(|_| CaptureError::Capacity)?;
    Ok(())
}
struct Prepared {
    request_json: String,
    preview: IntakePreview,
    preview_json: String,
    policy_json: String,
    source_key: String,
    provenance_digest: String,
    receipt: CaptureReceipt,
    receipt_json: String,
    receipt_digest: String,
}
fn prepare(r: &CaptureIntakeRequest, ctx: &AuthorizedIntakeContext) -> Result<Prepared> {
    validate_request(r)?;
    let (project, root) = ids(&ctx.scope)?;
    if !same_scope(&r.scope, &ctx.scope)
        || r.policy_revision != ctx.policy.revision
        || r.policy_digest != ctx.policy.identity_digest()?
    {
        return Err(CaptureError::Denied);
    }
    let SourcePayload::InlineUtf8 { text } = &r.source_payload else {
        return Err(CaptureError::UnsupportedSource);
    };
    let preview = intake::preview_intake(
        text.as_bytes(),
        r.format,
        &r.provenance,
        &ctx.scope,
        &ctx.policy,
    )?;
    if preview.manifest_digest != r.preview_manifest_digest {
        return Err(CaptureError::Invalid);
    }
    let request_json = encoded(r, MAX_JSON)?;
    let request_digest = hash(&("admiralty.capture.request/v1", r))?;
    let source_key = hash(&(
        "admiralty.capture.source/v1",
        project,
        root,
        &r.provenance.source_id,
        &r.provenance.revision,
    ))?;
    let provenance_digest = hash(&("admiralty.capture.provenance/v1", &r.provenance))?;
    let receipt = CaptureReceipt {
        schema_version: 1,
        command_id: r.command_id.clone(),
        capture_id: r.capture_id.clone(),
        revision: r.revision,
        request_digest,
        raw_digest: preview.raw_digest.clone(),
        canonical_digest: preview.canonical_digest.clone(),
        preview_manifest_digest: preview.manifest_digest.clone(),
        parent_receipt_digest: r.parent_receipt_digest.clone(),
    };
    Ok(Prepared {
        request_json,
        preview_json: encoded(&preview, MAX_JSON)?,
        policy_json: encoded(&PolicySnapshot::capture(&ctx.policy), MAX_SMALL_JSON)?,
        source_key,
        provenance_digest,
        receipt_json: encoded(&receipt, MAX_SMALL_JSON)?,
        receipt_digest: hash(&("admiralty.capture.receipt/v1", &receipt))?,
        receipt,
        preview,
    })
}
fn verify_schema(c: &Connection) -> Result<()> {
    // Compare fixed qualified schema against an empty in-memory reference, never
    // execute SQL supplied by a database. Also detects replaced immutable triggers.
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(MIGRATION)?;
    let mut q=expected.prepare("SELECT type,name,sql FROM main.sqlite_master WHERE sql IS NOT NULL AND name LIKE 'intake_capture_%' ORDER BY name")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let kind = stored_text(r, 0, 16)?;
        let name = stored_text(r, 1, 128)?;
        let sql = stored_text(r, 2, 16 * 1024)?;
        let actual: Option<(String, String)> = c
            .query_row(
                "SELECT type,sql FROM main.sqlite_master WHERE name=?1",
                [name],
                |r| Ok((stored_text(r, 0, 16)?, stored_text(r, 1, 16 * 1024)?)),
            )
            .optional()?;
        if actual != Some((kind, sql)) {
            return Err(CaptureError::Integrity);
        }
    }
    Ok(())
}
fn bounds(c: &Connection) -> Result<()> {
    let (heads,rows,sources,bytes):(i64,i64,i64,i64)=c.query_row("SELECT (SELECT count(*) FROM main.intake_capture_heads),(SELECT count(*) FROM main.intake_capture_revisions),(SELECT count(*) FROM main.intake_capture_sources),COALESCE((SELECT sum(length(CAST(request_json AS BLOB))+length(CAST(preview_json AS BLOB))+length(CAST(policy_json AS BLOB))+length(CAST(receipt_json AS BLOB))+length(CAST(project_id AS BLOB))+length(CAST(root_id AS BLOB))+length(CAST(capture_id AS BLOB))+length(CAST(command_id AS BLOB))+length(CAST(principal_id AS BLOB))+length(CAST(authority_ref AS BLOB))+length(CAST(authority_generation AS BLOB))+length(CAST(source_key AS BLOB))+length(CAST(request_digest AS BLOB))+length(CAST(receipt_digest AS BLOB))) FROM main.intake_capture_revisions),0)+COALESCE((SELECT sum(length(CAST(source_key AS BLOB))+length(CAST(project_id AS BLOB))+length(CAST(root_id AS BLOB))+length(CAST(capture_id AS BLOB))+length(CAST(provenance_digest AS BLOB))+length(CAST(raw_digest AS BLOB))) FROM main.intake_capture_sources),0)+COALESCE((SELECT sum(length(CAST(project_id AS BLOB))+length(CAST(root_id AS BLOB))+length(CAST(capture_id AS BLOB))) FROM main.intake_capture_heads),0)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    if heads < 0
        || heads as usize > MAX_CAPTURES
        || rows < 0
        || rows as usize > MAX_ROWS
        || sources != rows
        || bytes < 0
        || bytes as usize > MAX_HISTORY_BYTES
    {
        return Err(CaptureError::Capacity);
    }
    Ok(())
}
pub(crate) fn initialize(c: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    tx.execute_batch(MIGRATION)?;
    verify_schema(&tx)?;
    tx.commit()?;
    validate_all(c)
}
struct RevisionRow {
    request: String,
    preview: String,
    policy: String,
    receipt: String,
    request_digest: String,
    receipt_digest: String,
    source_key: String,
    command: String,
    principal: String,
    authority: String,
    generation: String,
}
fn read_revision(
    tx: &Transaction<'_>,
    project: &ProjectId,
    root: &RootId,
    id: &str,
    revision: u32,
) -> Result<Option<CapturedIntake>> {
    let row=tx.query_row("SELECT request_json,preview_json,policy_json,receipt_json,request_digest,receipt_digest,source_key,command_id,principal_id,authority_ref,authority_generation,CASE WHEN json_valid(request_json) THEN COALESCE(json_type(request_json,'$.source_payload.text')='text' AND length(CAST(json_extract(request_json,'$.source_payload.text') AS BLOB))<=1048576,0) ELSE 0 END FROM main.intake_capture_revisions WHERE project_id=?1 AND root_id=?2 AND capture_id=?3 AND revision=?4",params![project.as_str(),root.as_str(),id,revision],|r|{if !r.get::<_,bool>(11)? {return Err(rusqlite::Error::InvalidQuery);} Ok(RevisionRow{request:stored_text(r,0,MAX_JSON)?,preview:stored_text(r,1,MAX_JSON)?,policy:stored_text(r,2,MAX_SMALL_JSON)?,receipt:stored_text(r,3,MAX_SMALL_JSON)?,request_digest:stored_text(r,4,71)?,receipt_digest:stored_text(r,5,71)?,source_key:stored_text(r,6,71)?,command:stored_text(r,7,128)?,principal:stored_text(r,8,128)?,authority:stored_text(r,9,1024)?,generation:stored_text(r,10,20)?})}).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    // SQL checks decoded raw byte length before copying/decoding the serialized
    // request; stored_text checks each serialized blob before any Rust allocation.
    // Request has fixed structure and a bounded inline string. Never decode the
    // stored preview's potentially hostile arrays: recompute from original bytes.
    let request: CaptureIntakeRequest = serde_json::from_str(&row.request)?;
    validate_request(&request)?;
    let policy: PolicySnapshot = serde_json::from_str(&row.policy)?;
    let s = &request.scope;
    let ctx = AuthorizedIntakeContext {
        scope: AuthorizedIntakeScope {
            principal: s.principal.clone(),
            project_id: s.project_id.clone(),
            root_id: s.root_id.clone(),
            authority_ref: s.authority_ref.clone(),
            authority_generation: s.authority_generation,
        },
        policy: policy.policy(),
    };
    let p = prepare(&request, &ctx)?;
    if request.capture_id != id
        || request.revision != revision
        || request.scope.project_id.as_ref() != Some(project)
        || request.scope.root_id.as_ref() != Some(root)
        || row.request != p.request_json
        || row.preview != p.preview_json
        || row.policy != p.policy_json
        || row.receipt != p.receipt_json
        || row.request_digest != p.receipt.request_digest
        || row.receipt_digest != p.receipt_digest
        || row.source_key != p.source_key
        || row.command != request.command_id.as_str()
        || row.principal != s.principal.as_str()
        || row.authority != s.authority_ref
        || row.generation != s.authority_generation.to_string()
    {
        return Err(CaptureError::Integrity);
    }
    let source:Option<(String,String,String,u32,String,String)>=tx.query_row("SELECT project_id,root_id,capture_id,revision,provenance_digest,raw_digest FROM main.intake_capture_sources WHERE source_key=?1",[&p.source_key],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,128)?,stored_text(r,2,128)?,r.get(3)?,stored_text(r,4,71)?,stored_text(r,5,71)?))).optional()?;
    if source
        != Some((
            project.to_string(),
            root.to_string(),
            id.to_owned(),
            revision,
            p.provenance_digest,
            p.preview.raw_digest.clone(),
        ))
    {
        return Err(CaptureError::Integrity);
    }
    Ok(Some(CapturedIntake {
        request,
        preview: p.preview,
        receipt: p.receipt,
        receipt_digest: p.receipt_digest,
    }))
}
fn current(tx: &Transaction<'_>, p: &ProjectId, r: &RootId, id: &str) -> Result<Option<u32>> {
    let head:Option<u32>=tx.query_row("SELECT current_revision FROM main.intake_capture_heads WHERE project_id=?1 AND root_id=?2 AND capture_id=?3",params![p.as_str(),r.as_str(),id],|r|r.get(0)).optional()?;
    let (count,max):(i64,Option<u32>)=tx.query_row("SELECT count(*),max(revision) FROM main.intake_capture_revisions WHERE project_id=?1 AND root_id=?2 AND capture_id=?3",params![p.as_str(),r.as_str(),id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if head.is_none() && count == 0 {
        return Ok(None);
    }
    if head != max || head.is_none_or(|h| h == 0 || h > MAX_REVISIONS || i64::from(h) != count) {
        return Err(CaptureError::Integrity);
    }
    Ok(head)
}
fn validate_capture(
    tx: &Transaction<'_>,
    p: &ProjectId,
    r: &RootId,
    id: &str,
) -> Result<Option<CapturedIntake>> {
    let Some(head) = current(tx, p, r, id)? else {
        return Ok(None);
    };
    let mut prior: Option<CapturedIntake> = None;
    for revision in 1..=head {
        let row = read_revision(tx, p, r, id, revision)?.ok_or(CaptureError::Integrity)?;
        if row.request.parent_receipt_digest.as_ref() != prior.as_ref().map(|p| &p.receipt_digest) {
            return Err(CaptureError::Integrity);
        }
        prior = Some(row);
    }
    Ok(prior)
}
pub(crate) fn record(
    c: &mut Connection,
    request: CaptureIntakeRequest,
    ctx: AuthorizedIntakeContext,
) -> Result<CaptureResult> {
    let prepared = prepare(&request, &ctx)?;
    let (project, root) = ids(&ctx.scope)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_schema(&tx)?;
    bounds(&tx)?;
    let prior:Option<(String,String,String,u32)>=tx.query_row("SELECT project_id,root_id,capture_id,revision FROM main.intake_capture_revisions WHERE command_id=?1",[request.command_id.as_str()],|r|Ok((stored_text(r,0,128)?,stored_text(r,1,128)?,stored_text(r,2,128)?,r.get(3)?))).optional()?;
    if let Some((p, r, id, revision)) = prior {
        if p != project.as_str() || r != root.as_str() {
            return Err(CaptureError::Collision);
        }
        validate_capture(&tx, project, root, &id)?;
        let original =
            read_revision(&tx, project, root, &id, revision)?.ok_or(CaptureError::Integrity)?;
        if original.request != request {
            return Err(CaptureError::Collision);
        }
        tx.commit()?;
        return Ok(CaptureResult {
            receipt: original.receipt,
            receipt_digest: original.receipt_digest,
            replayed: true,
        });
    }
    let prior = validate_capture(&tx, project, root, &request.capture_id)?;
    let head = prior.as_ref().map_or(0, |p| p.request.revision);
    if head != request.expected_revision
        || request.parent_receipt_digest.as_ref() != prior.as_ref().map(|p| &p.receipt_digest)
    {
        return Err(CaptureError::StaleCurrent);
    }
    if head == 0 {
        let count: i64 =
            tx.query_row("SELECT count(*) FROM main.intake_capture_heads", [], |r| {
                r.get(0)
            })?;
        if count as usize >= MAX_CAPTURES {
            return Err(CaptureError::Capacity);
        }
    }
    let source_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.intake_capture_sources WHERE source_key=?1)",
        [&prepared.source_key],
        |r| r.get(0),
    )?;
    if source_exists {
        return Err(CaptureError::Collision);
    }
    if head == 0 {
        tx.execute("INSERT INTO main.intake_capture_heads(project_id,root_id,capture_id,current_revision) VALUES(?1,?2,?3,?4)",params![project.as_str(),root.as_str(),request.capture_id,request.revision])?;
    }
    tx.execute("INSERT INTO main.intake_capture_sources(source_key,project_id,root_id,capture_id,revision,provenance_digest,raw_digest) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![prepared.source_key,project.as_str(),root.as_str(),request.capture_id,request.revision,prepared.provenance_digest,prepared.preview.raw_digest])?;
    tx.execute("INSERT INTO main.intake_capture_revisions(project_id,root_id,capture_id,revision,command_id,principal_id,authority_ref,authority_generation,source_key,request_digest,receipt_digest,request_json,preview_json,policy_json,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![project.as_str(),root.as_str(),request.capture_id,request.revision,request.command_id.as_str(),ctx.scope.principal.as_str(),ctx.scope.authority_ref,ctx.scope.authority_generation.to_string(),prepared.source_key,prepared.receipt.request_digest,prepared.receipt_digest,prepared.request_json,prepared.preview_json,prepared.policy_json,prepared.receipt_json])?;
    if head!=0&&tx.execute("UPDATE main.intake_capture_heads SET current_revision=?1 WHERE project_id=?2 AND root_id=?3 AND capture_id=?4 AND current_revision=?5",params![request.revision,project.as_str(),root.as_str(),request.capture_id,head])?!=1{return Err(CaptureError::StaleCurrent);}
    bounds(&tx)?;
    validate_capture(&tx, project, root, &request.capture_id)?;
    tx.commit()?;
    Ok(CaptureResult {
        receipt: prepared.receipt,
        receipt_digest: prepared.receipt_digest,
        replayed: false,
    })
}
pub(crate) fn get(
    c: &mut Connection,
    scope: &AuthorizedIntakeScope,
    id: &str,
    revision: Option<u32>,
) -> Result<Option<CapturedIntake>> {
    let (project, root) = ids(scope)?;
    identifier(id)?;
    if revision.is_some_and(|r| r == 0 || r > MAX_REVISIONS) {
        return Err(CaptureError::Invalid);
    }
    let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
    verify_schema(&tx)?;
    bounds(&tx)?;
    let readable:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM main.intake_capture_heads h JOIN main.intake_capture_revisions r ON r.project_id=h.project_id AND r.root_id=h.root_id AND r.capture_id=h.capture_id AND r.revision=COALESCE(?4,h.current_revision) WHERE h.project_id=?1 AND h.root_id=?2 AND h.capture_id=?3 AND r.principal_id=?5 AND r.authority_ref=?6 AND r.authority_generation=?7)",params![project.as_str(),root.as_str(),id,revision,scope.principal.as_str(),scope.authority_ref,scope.authority_generation.to_string()],|r|r.get(0))?;
    if !readable {
        return Ok(None);
    }
    let current = validate_capture(&tx, project, root, id)?.ok_or(CaptureError::Integrity)?;
    let row = if let Some(revision) = revision {
        read_revision(&tx, project, root, id, revision)?.ok_or(CaptureError::Integrity)?
    } else {
        current
    };
    if !same_scope(&row.request.scope, scope) {
        return Err(CaptureError::Integrity);
    }
    tx.commit()?;
    Ok(Some(row))
}
pub(crate) fn validate_all(c: &Connection) -> Result<()> {
    verify_schema(c)?;
    bounds(c)?;
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Deferred)?;
    let mut q=tx.prepare("SELECT project_id,root_id,capture_id FROM main.intake_capture_heads ORDER BY project_id,root_id,capture_id LIMIT 129")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let p = ProjectId::new(stored_text(r, 0, 128)?).map_err(|_| CaptureError::Integrity)?;
        let root = RootId::new(stored_text(r, 1, 128)?).map_err(|_| CaptureError::Integrity)?;
        let id = stored_text(r, 2, 128)?;
        identifier(&id)?;
        validate_capture(&tx, &p, &root, &id)?.ok_or(CaptureError::Integrity)?;
    }
    drop(rows);
    drop(q);
    for table in [
        "intake_capture_heads",
        "intake_capture_sources",
        "intake_capture_revisions",
    ] {
        if tx
            .prepare(&format!("PRAGMA main.foreign_key_check({table})"))?
            .exists([])?
        {
            return Err(CaptureError::Integrity);
        }
    }
    tx.commit()?;
    Ok(())
}
