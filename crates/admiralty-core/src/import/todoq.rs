//! Bounded, inert observations of supplied todoq snapshots. No I/O or effects.
//! Compatibility reference: neverhuman/nightshift-todo-driver at
//! b26ddcbfc12e6be527093dd3afa3e63040c03d1a, queue.py/load_family and todo.py/load.
//! Git pins are caller-supplied claims; raw SHA-256 verifies only supplied bytes,
//! not membership in a Git commit. Raw sources retain unknown metadata verbatim.
use crate::{
    ProjectId, RootId, content_digest,
    intake::{IntakeFormat, SourceSpan},
    limits::bounded_json_size,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use toml::{Table, Value};

pub const PARSER_REVISION: &str = "todoq-b26ddcb-inert-v1";
pub const REFERENCE_COMMIT: &str = "b26ddcbfc12e6be527093dd3afa3e63040c03d1a";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 1000;
const MAX_NODES: usize = 32768;
const MAX_DEPTH: usize = 32;
const MAX_FIELD: usize = 65536;
const MAX_REPOS: usize = 64;
const MAX_EDGES: usize = 4096;
const MAX_DIAGNOSTICS: usize = 4096;
const MAX_OUTPUT: usize = 8 * MAX_BYTES;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("invalid import limits")]
    Policy,
    #[error("bounded import capacity exceeded")]
    Capacity,
    #[error("invalid supplied source identity")]
    Pin,
    #[error("supplied bytes differ from declared raw digest")]
    Digest,
    #[error("immutable source identity/content collision")]
    SourceCollision,
    #[error("ambiguous repository mapping")]
    Mapping,
    #[error("family metadata is malformed")]
    Family,
    #[error("invalid snapshot or page cursor")]
    Cursor,
}
type Result<T> = std::result::Result<T, ImportError>;

/// Trusted work limits, not imported policy or an execution grant.
#[derive(Clone, Debug)]
pub struct ImportPolicy {
    pub max_bytes: usize,
    pub max_records: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_field_bytes: usize,
}
impl Default for ImportPolicy {
    fn default() -> Self {
        Self {
            max_bytes: MAX_BYTES,
            max_records: MAX_RECORDS,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            max_field_bytes: MAX_FIELD,
        }
    }
}
impl ImportPolicy {
    fn validate(&self) -> Result<()> {
        if [
            (self.max_bytes, MAX_BYTES),
            (self.max_records, MAX_RECORDS),
            (self.max_nodes, MAX_NODES),
            (self.max_depth, MAX_DEPTH),
            (self.max_field_bytes, MAX_FIELD),
        ]
        .iter()
        .any(|(n, max)| *n == 0 || n > max)
        {
            return Err(ImportError::Policy);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct QueuePin {
    pub repository: String,
    pub ref_name: String,
    pub commit_oid: String,
}
pub struct PinnedDocument<'a> {
    pub path: &'a str,
    pub blob_oid: &'a str,
    pub raw_sha256: &'a str,
    pub bytes: &'a [u8],
}
/// Independently supplied mapping data, never derived from imported prose.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepositoryMapping {
    pub family: String,
    pub repository_alias: String,
    pub project_id: ProjectId,
    pub root_id: RootId,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PriorSource {
    pub source_key: String,
    pub raw_sha256: String,
}
pub struct PinnedSnapshot<'a> {
    pub queue: &'a QueuePin,
    pub family: PinnedDocument<'a>,
    pub todos: &'a [PinnedDocument<'a>],
    pub mappings: &'a [RepositoryMapping],
    pub prior_sources: &'a [PriorSource],
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SourceObservation {
    pub source_key: String,
    pub path: String,
    pub blob_oid: String,
    pub raw_sha256: String,
    pub raw_text: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RepositoryObservation {
    pub name: String,
    pub url: String,
    pub gate: Option<String>,
    pub setup: Option<String>,
    pub order: Option<i64>,
    pub mapping: Option<RepositoryMapping>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FamilyObservation {
    pub name: String,
    pub landing: Option<String>,
    pub batch_branch: Option<String>,
    pub base_branch: Option<String>,
    pub shift_tz: Option<String>,
    pub dayshift_prefix: Option<String>,
    pub legacy_bulletshift_prefix: Option<String>,
    pub nightshift_prefix: Option<String>,
    pub worker_notes: Option<String>,
    pub deny_commands: Vec<String>,
    pub auto_shift_pr: Option<bool>,
    pub repositories: Vec<RepositoryObservation>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    MalformedToml,
    MissingFence,
    MissingField,
    WrongType,
    FieldBound,
    UnknownMetadata,
    UnknownStatus,
    UnknownMode,
    UnknownLanding,
    FamilyMismatch,
    MissingRepository,
    UnmappedRepository,
    MissingDependency,
    DuplicateTaskId,
    DuplicateDependency,
    CycleOrCycleDependent,
    Placeholder,
    CriteriaUnresolved,
    ImportedHistoryUnverified,
    RequesterAliasConflict,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub source_path: String,
    pub code: DiagnosticCode,
    pub field: Option<String>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct IntakeSuggestion {
    pub format: IntakeFormat,
    pub text: String,
    pub source_span: SourceSpan,
    pub claimed_project_id: Option<ProjectId>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TodoObservation {
    pub id: String,
    pub family: String,
    pub title: String,
    pub repositories: Vec<String>,
    pub blocked_by: Vec<String>,
    pub declared_status: Option<String>,
    pub declared_mode: Option<String>,
    pub declared_merged: Option<bool>,
    pub priority: Option<i64>,
    pub requested_by: Option<String>,
    pub legacy_filed_by: Option<String>,
    /// Historical source text only; never an active claim or lease.
    pub historical_claimant: Option<String>,
    pub observed_lease_until: Option<String>,
    pub suggestion: Option<IntakeSuggestion>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RecordObservation {
    /// Index into the containing snapshot or page sources; stable source_key is unchanged.
    pub source_index: u16,
    /// None retains a malformed supplied file as an explicit observation.
    pub todo: Option<TodoObservation>,
    pub diagnostics: Vec<Diagnostic>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SnapshotObservation {
    pub schema_version: u32,
    pub parser_revision: String,
    pub queue: QueuePin,
    pub family: FamilyObservation,
    /// Index zero is family.toml; records refer to their exact raw source.
    pub sources: Vec<SourceObservation>,
    pub records: Vec<RecordObservation>,
    pub family_diagnostics: Vec<Diagnostic>,
    pub mapping_digest: String,
    pub snapshot_digest: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ObservationPage {
    pub schema_version: u32,
    pub snapshot_digest: String,
    pub queue: QueuePin,
    pub family: FamilyObservation,
    pub family_diagnostics: Vec<Diagnostic>,
    /// Original snapshot record ordinal of this page's first record.
    pub record_offset: u16,
    pub records: Vec<RecordObservation>,
    /// Family source at zero; selected records use page-local indices 1..=records.len().
    /// Exact source_key/path/blob/digest/raw bytes remain identical to the snapshot.
    pub sources: Vec<SourceObservation>,
    pub continuation: Option<String>,
}
impl SnapshotObservation {
    fn digest(&self) -> Result<String> {
        bounded_json_size(self, MAX_OUTPUT).map_err(|_| ImportError::Capacity)?;
        content_digest(&(
            "admiralty.todoq.snapshot/v1",
            self.schema_version,
            &self.parser_revision,
            &self.queue,
            &self.family,
            &self.sources,
            &self.records,
            &self.family_diagnostics,
            &self.mapping_digest,
        ))
        .map_err(|_| ImportError::Capacity)
    }
    /// Cursor pinning is integrity/continuation, not access authorization.
    pub fn page(&self, cursor: Option<&str>, limit: u16) -> Result<ObservationPage> {
        if limit == 0
            || limit > 100
            || self.schema_version != 1
            || self.parser_revision != PARSER_REVISION
            || !is_digest(&self.snapshot_digest)
            || self.snapshot_digest != self.digest()?
        {
            return Err(ImportError::Cursor);
        }
        let start = if let Some(cursor) = cursor {
            if cursor.len() > 96 {
                return Err(ImportError::Cursor);
            }
            let (digest, index) = cursor.rsplit_once(':').ok_or(ImportError::Cursor)?;
            let index = index.parse::<usize>().map_err(|_| ImportError::Cursor)?;
            if digest != self.snapshot_digest
                || cursor != format!("{digest}:{index}")
                || index > self.records.len()
            {
                return Err(ImportError::Cursor);
            }
            index
        } else {
            0
        };
        let end = start
            .saturating_add(usize::from(limit))
            .min(self.records.len());
        let mut records = self.records[start..end].to_vec();
        let mut sources = Vec::with_capacity(records.len() + 1);
        sources.push(self.sources.first().ok_or(ImportError::Cursor)?.clone());
        for (local, record) in records.iter_mut().enumerate() {
            sources.push(
                self.sources
                    .get(usize::from(record.source_index))
                    .ok_or(ImportError::Cursor)?
                    .clone(),
            );
            record.source_index = u16::try_from(local + 1).map_err(|_| ImportError::Cursor)?;
        }
        Ok(ObservationPage {
            schema_version: 1,
            snapshot_digest: self.snapshot_digest.clone(),
            queue: self.queue.clone(),
            family: self.family.clone(),
            family_diagnostics: self.family_diagnostics.clone(),
            record_offset: u16::try_from(start).map_err(|_| ImportError::Cursor)?,
            records,
            sources,
            continuation: (end < self.records.len())
                .then(|| format!("{}:{end}", self.snapshot_digest)),
        })
    }
}
fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn id(s: &str) -> bool {
    !s.trim().is_empty() && text(s, 128) && !s.bytes().any(|b| b.is_ascii_control())
}
fn oid(s: &str) -> bool {
    [40, 64].contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && s.bytes().any(|b| b != b'0')
}
fn pin_ok(p: &QueuePin) -> bool {
    id(&p.repository)
        && text(&p.ref_name, 256)
        && !p.ref_name.is_empty()
        && !p
            .ref_name
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        && !p.ref_name.contains(['\\'])
        && !p.ref_name.contains("..")
        && oid(&p.commit_oid)
}
fn path_ok(p: &str) -> bool {
    text(p, 1024)
        && !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.bytes().any(|b| b.is_ascii_control())
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}
fn raw_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn source_key(queue: &QueuePin, doc: &PinnedDocument<'_>) -> Result<String> {
    content_digest(&("admiralty.todoq.source/v1", queue, doc.path, doc.blob_oid))
        .map_err(|_| ImportError::Capacity)
}
fn source(queue: &QueuePin, doc: &PinnedDocument<'_>) -> Result<SourceObservation> {
    Ok(SourceObservation {
        source_key: source_key(queue, doc)?,
        path: doc.path.into(),
        blob_oid: doc.blob_oid.into(),
        raw_sha256: doc.raw_sha256.into(),
        raw_text: std::str::from_utf8(doc.bytes)
            .map_err(|_| ImportError::Pin)?
            .into(),
    })
}

/// The real TOML parser owns grammar and duplicate-key checks. Its bounded
/// default recursion limit remains enabled; aggregate input bytes are checked
/// before parsing. This borrowed tree walk precedes projection cloning/hashing.
fn tree_bounds(table: &Table, policy: &ImportPolicy, nodes: &mut usize) -> Result<()> {
    let mut stack: Vec<(&Value, usize)> = Vec::new();
    if table.len() > policy.max_nodes {
        return Err(ImportError::Capacity);
    }
    for (key, value) in table {
        if !text(key, 128) {
            return Err(ImportError::Capacity);
        }
        stack.push((value, 1));
    }
    while let Some((v, depth)) = stack.pop() {
        *nodes = nodes.checked_add(1).ok_or(ImportError::Capacity)?;
        if *nodes > policy.max_nodes || depth > policy.max_depth {
            return Err(ImportError::Capacity);
        }
        match v {
            Value::String(s) if !text(s, policy.max_field_bytes) => {
                return Err(ImportError::Capacity);
            }
            Value::Array(a) => {
                if a.len() > policy.max_nodes.saturating_sub(*nodes + stack.len()) {
                    return Err(ImportError::Capacity);
                }
                for v in a {
                    stack.push((v, depth + 1));
                }
            }
            Value::Table(t) => {
                if t.len() > policy.max_nodes.saturating_sub(*nodes + stack.len()) {
                    return Err(ImportError::Capacity);
                }
                for (k, v) in t {
                    if !text(k, 128) {
                        return Err(ImportError::Capacity);
                    }
                    stack.push((v, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug)]
struct Malformed {
    code: DiagnosticCode,
    field: &'static str,
}
fn required<'a>(t: &'a Table, k: &'static str) -> std::result::Result<&'a str, Malformed> {
    let s = t
        .get(k)
        .ok_or(Malformed {
            code: DiagnosticCode::MissingField,
            field: k,
        })?
        .as_str()
        .ok_or(Malformed {
            code: DiagnosticCode::WrongType,
            field: k,
        })?;
    if !(if k == "title" {
        !s.trim().is_empty() && text(s, 4096)
    } else {
        id(s)
    }) {
        return Err(Malformed {
            code: DiagnosticCode::FieldBound,
            field: k,
        });
    }
    Ok(s)
}
fn optional<'a>(t: &'a Table, k: &'static str) -> std::result::Result<Option<&'a str>, Malformed> {
    t.get(k)
        .map(|v| {
            v.as_str().ok_or(Malformed {
                code: DiagnosticCode::WrongType,
                field: k,
            })
        })
        .transpose()
}
fn strings<'a>(
    t: &'a Table,
    k: &'static str,
    max: usize,
) -> std::result::Result<Vec<&'a str>, Malformed> {
    let Some(v) = t.get(k) else { return Ok(vec![]) };
    let a = v.as_array().ok_or(Malformed {
        code: DiagnosticCode::WrongType,
        field: k,
    })?;
    if a.len() > max {
        return Err(Malformed {
            code: DiagnosticCode::FieldBound,
            field: k,
        });
    }
    a.iter()
        .map(|v| {
            v.as_str()
                .filter(|s| {
                    if k == "deny_commands" {
                        text(s, MAX_FIELD)
                    } else {
                        id(s)
                    }
                })
                .ok_or(Malformed {
                    code: DiagnosticCode::WrongType,
                    field: k,
                })
        })
        .collect()
}
fn optional_bool(t: &Table, k: &'static str) -> std::result::Result<Option<bool>, Malformed> {
    t.get(k)
        .map(|v| {
            v.as_bool().ok_or(Malformed {
                code: DiagnosticCode::WrongType,
                field: k,
            })
        })
        .transpose()
}
fn optional_int(t: &Table, k: &'static str) -> std::result::Result<Option<i64>, Malformed> {
    t.get(k)
        .map(|v| {
            v.as_integer().ok_or(Malformed {
                code: DiagnosticCode::WrongType,
                field: k,
            })
        })
        .transpose()
}
fn diag(path: &str, code: DiagnosticCode, field: Option<&str>) -> Diagnostic {
    Diagnostic {
        source_path: path.into(),
        code,
        field: field.map(str::to_owned),
    }
}
fn add(ds: &mut Vec<Diagnostic>, d: Diagnostic, count: &mut usize) -> Result<()> {
    if *count >= MAX_DIAGNOSTICS {
        return Err(ImportError::Capacity);
    }
    *count += 1;
    ds.push(d);
    Ok(())
}
fn unknown(
    t: &Table,
    known: &[&str],
    path: &str,
    prefix: &str,
    ds: &mut Vec<Diagnostic>,
    count: &mut usize,
) -> Result<()> {
    for key in t.keys().filter(|key| !known.contains(&key.as_str())) {
        add(
            ds,
            diag(
                path,
                DiagnosticCode::UnknownMetadata,
                Some(&format!(
                    "{prefix}/{}",
                    key.replace('~', "~0").replace('/', "~1")
                )),
            ),
            count,
        )?;
    }
    Ok(())
}
fn malformed(path: &str, m: Malformed) -> Diagnostic {
    diag(path, m.code, Some(m.field))
}
fn family(
    table: &Table,
    mappings: &[RepositoryMapping],
    count: &mut usize,
) -> Result<(FamilyObservation, Vec<Diagnostic>)> {
    let t = table
        .get("family")
        .and_then(Value::as_table)
        .ok_or(ImportError::Family)?;
    let name = required(t, "name").map_err(|_| ImportError::Family)?;
    let mut diagnostics = vec![];
    unknown(
        table,
        &["family", "repo"],
        "family.toml",
        "",
        &mut diagnostics,
        count,
    )?;
    unknown(
        t,
        &[
            "name",
            "batch_branch",
            "base_branch",
            "landing",
            "shift_tz",
            "dayshift_prefix",
            "bulletshift_prefix",
            "nightshift_prefix",
            "deny_commands",
            "worker_notes",
            "auto_shift_pr",
        ],
        "family.toml",
        "/family",
        &mut diagnostics,
        count,
    )?;
    let rows = table
        .get("repo")
        .map(|v| v.as_array().ok_or(ImportError::Family))
        .transpose()?
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if rows.len() > MAX_REPOS {
        return Err(ImportError::Capacity);
    }
    let mut repositories = vec![];
    let mut seen = BTreeSet::new();
    for v in rows {
        let r = v.as_table().ok_or(ImportError::Family)?;
        let alias = required(r, "name").map_err(|_| ImportError::Family)?;
        if !seen.insert(alias) {
            return Err(ImportError::Family);
        }
        let url = optional(r, "url")
            .map_err(|_| ImportError::Family)?
            .ok_or(ImportError::Family)?;
        let mapping = mappings
            .iter()
            .find(|m| m.family == name && m.repository_alias == alias)
            .cloned();
        if mapping.is_none() {
            add(
                &mut diagnostics,
                diag(
                    "family.toml",
                    DiagnosticCode::UnmappedRepository,
                    Some(alias),
                ),
                count,
            )?;
        }
        unknown(
            r,
            &["name", "url", "gate", "setup", "order"],
            "family.toml",
            &format!("/repo/{alias}"),
            &mut diagnostics,
            count,
        )?;
        repositories.push(RepositoryObservation {
            name: alias.into(),
            url: url.into(),
            gate: optional(r, "gate")
                .map_err(|_| ImportError::Family)?
                .map(str::to_owned),
            setup: optional(r, "setup")
                .map_err(|_| ImportError::Family)?
                .map(str::to_owned),
            order: optional_int(r, "order").map_err(|_| ImportError::Family)?,
            mapping,
        });
    }
    let get = |k| {
        optional(t, k)
            .map(|s| s.map(str::to_owned))
            .map_err(|_| ImportError::Family)
    };
    let landing = get("landing")?;
    if landing
        .as_deref()
        .is_some_and(|s| !["batch", "shifts"].contains(&s))
    {
        add(
            &mut diagnostics,
            diag(
                "family.toml",
                DiagnosticCode::UnknownLanding,
                Some("landing"),
            ),
            count,
        )?;
    }
    Ok((
        FamilyObservation {
            name: name.into(),
            landing,
            batch_branch: get("batch_branch")?,
            base_branch: get("base_branch")?,
            shift_tz: get("shift_tz")?,
            dayshift_prefix: get("dayshift_prefix")?,
            legacy_bulletshift_prefix: get("bulletshift_prefix")?,
            nightshift_prefix: get("nightshift_prefix")?,
            worker_notes: get("worker_notes")?,
            deny_commands: strings(t, "deny_commands", MAX_REPOS)
                .map_err(|_| ImportError::Family)?
                .into_iter()
                .map(str::to_owned)
                .collect(),
            auto_shift_pr: optional_bool(t, "auto_shift_pr").map_err(|_| ImportError::Family)?,
            repositories,
        },
        diagnostics,
    ))
}
struct Parsed<'a> {
    doc: &'a PinnedDocument<'a>,
    table: std::result::Result<Table, Malformed>,
    body: &'a str,
    body_start: usize,
}
fn markdown<'a>(doc: &'a PinnedDocument<'a>) -> Parsed<'a> {
    let text = std::str::from_utf8(doc.bytes).expect("preflight UTF-8");
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    let bad = |code| Parsed {
        doc,
        table: Err(Malformed {
            code,
            field: "front_matter",
        }),
        body: "",
        body_start: doc.bytes.len(),
    };
    if first.trim_end_matches(['\r', '\n']) != "+++" || !first.ends_with('\n') {
        return bad(DiagnosticCode::MissingFence);
    }
    let mut offset = first.len();
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "+++" {
            let body_start = offset + line.len();
            return Parsed {
                doc,
                table: toml::from_str(&text[first.len()..offset]).map_err(|_| Malformed {
                    code: DiagnosticCode::MalformedToml,
                    field: "front_matter",
                }),
                body: &text[body_start..],
                body_start,
            };
        }
        offset += line.len();
    }
    bad(DiagnosticCode::MissingFence)
}
struct Todo<'a> {
    id: &'a str,
    family: &'a str,
    title: &'a str,
    repos: Vec<&'a str>,
    deps: Vec<&'a str>,
    status: Option<&'a str>,
    mode: Option<&'a str>,
    merged: Option<bool>,
    priority: Option<i64>,
    requested: Option<&'a str>,
    filed: Option<&'a str>,
    claim: Option<&'a str>,
    lease: Option<&'a str>,
}
fn todo(t: &Table) -> std::result::Result<Todo<'_>, Malformed> {
    Ok(Todo {
        id: required(t, "id")?,
        family: required(t, "family")?,
        title: required(t, "title")?,
        repos: strings(t, "repos", MAX_REPOS)?,
        deps: strings(t, "blocked_by", MAX_EDGES)?,
        status: optional(t, "status")?,
        mode: optional(t, "mode")?,
        merged: optional_bool(t, "merged")?,
        priority: optional_int(t, "priority")?,
        requested: optional(t, "requested_by")?,
        filed: optional(t, "filed_by")?,
        claim: optional(t, "claim_by")?,
        lease: optional(t, "lease_until")?,
    })
}

pub fn parse_pinned_snapshot(
    input: &PinnedSnapshot<'_>,
    policy: &ImportPolicy,
) -> Result<SnapshotObservation> {
    policy.validate()?;
    if input.todos.len() > policy.max_records
        || input.mappings.len() > MAX_REPOS
        || input.prior_sources.len() > MAX_RECORDS + 1
    {
        return Err(ImportError::Capacity);
    }
    if !pin_ok(input.queue) || input.family.path != "family.toml" {
        return Err(ImportError::Pin);
    }
    let mut bytes = 0usize;
    let mut paths = BTreeSet::new();
    for doc in std::iter::once(&input.family).chain(input.todos.iter()) {
        bytes = bytes
            .checked_add(doc.bytes.len())
            .ok_or(ImportError::Capacity)?;
        if bytes > policy.max_bytes {
            return Err(ImportError::Capacity);
        }
        if !path_ok(doc.path)
            || !oid(doc.blob_oid)
            || doc.blob_oid.len() != input.queue.commit_oid.len()
            || !is_digest(doc.raw_sha256)
            || std::str::from_utf8(doc.bytes).is_err()
        {
            return Err(ImportError::Pin);
        }
        if !paths.insert(doc.path) {
            return Err(ImportError::SourceCollision);
        }
        if doc.path != "family.toml"
            && !doc
                .path
                .strip_prefix("todos/")
                .is_some_and(|s| s.ends_with(".md") && !s.contains('/'))
        {
            return Err(ImportError::Pin);
        }
    }
    let mut mapping_keys = BTreeSet::new();
    for m in input.mappings {
        if !id(&m.family)
            || !id(&m.repository_alias)
            || !mapping_keys.insert((&m.family, &m.repository_alias))
        {
            return Err(ImportError::Mapping);
        }
    }
    let mut prior = BTreeMap::new();
    for p in input.prior_sources {
        if !is_digest(&p.source_key) || !is_digest(&p.raw_sha256) {
            return Err(ImportError::Pin);
        }
        if prior
            .insert(p.source_key.as_str(), p.raw_sha256.as_str())
            .is_some_and(|old| old != p.raw_sha256)
        {
            return Err(ImportError::SourceCollision);
        }
    }
    for doc in std::iter::once(&input.family).chain(input.todos.iter()) {
        if raw_digest(doc.bytes) != doc.raw_sha256 {
            return Err(ImportError::Digest);
        }
        if prior
            .get(source_key(input.queue, doc)?.as_str())
            .is_some_and(|old| *old != doc.raw_sha256)
        {
            return Err(ImportError::SourceCollision);
        }
    }
    let family_raw = std::str::from_utf8(input.family.bytes).map_err(|_| ImportError::Pin)?;
    let family_table: Table = toml::from_str(family_raw).map_err(|_| ImportError::Family)?;
    let mut nodes = 0;
    tree_bounds(&family_table, policy, &mut nodes)?;
    let mut sorted: Vec<_> = input.todos.iter().collect();
    sorted.sort_by_key(|d| d.path);
    let mut parsed = Vec::with_capacity(sorted.len());
    for doc in sorted {
        let p = markdown(doc);
        if let Ok(t) = &p.table {
            tree_bounds(t, policy, &mut nodes)?;
        }
        // Enforce the shared node budget before parsing the next document;
        // never retain an entire oversized parsed collection before checking.
        parsed.push(p);
    }
    let metas: Vec<_> = parsed
        .iter()
        .map(|p| p.table.as_ref().map_err(|e| *e).and_then(todo))
        .collect();
    let mut ids: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    let mut edges = 0usize;
    for (i, m) in metas.iter().enumerate() {
        if let Ok(m) = m {
            edges = edges
                .checked_add(m.deps.len())
                .ok_or(ImportError::Capacity)?;
            if edges > MAX_EDGES {
                return Err(ImportError::Capacity);
            }
            ids.entry((m.family, m.id)).or_default().push(i);
        }
    }
    let mut incoming = vec![0usize; metas.len()];
    let mut outgoing = vec![vec![]; metas.len()];
    for (i, m) in metas.iter().enumerate() {
        if let Ok(m) = m {
            let mut seen = BTreeSet::new();
            for d in &m.deps {
                if !seen.insert(*d) {
                    continue;
                }
                if let Some(target) = ids.get(&(m.family, *d)).filter(|v| v.len() == 1) {
                    incoming[i] += 1;
                    outgoing[target[0]].push(i);
                }
            }
        }
    }
    let mut queue: VecDeque<_> = incoming
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (*n == 0).then_some(i))
        .collect();
    while let Some(i) = queue.pop_front() {
        for next in &outgoing[i] {
            incoming[*next] -= 1;
            if incoming[*next] == 0 {
                queue.push_back(*next);
            }
        }
    }
    let mut diagnostic_count = 0;
    let (family, family_diagnostics) =
        family(&family_table, input.mappings, &mut diagnostic_count)?;
    let mut sources = vec![source(input.queue, &input.family)?];
    let mut records = vec![];
    let known = [
        "id",
        "family",
        "title",
        "repos",
        "mode",
        "priority",
        "blocked_by",
        "status",
        "attempts",
        "requested_by",
        "filed_by",
        "filed_at",
        "claim_by",
        "lease_until",
        "shift",
        "change_set",
        "commits",
        "merged",
        "note",
        "triaged",
        "worked_by",
        "triage_cost_usd",
    ];
    for (i, (p, m)) in parsed.iter().zip(&metas).enumerate() {
        sources.push(source(input.queue, p.doc)?);
        let mut ds = vec![];
        let observed = match m {
            Err(m) => {
                add(&mut ds, malformed(p.doc.path, *m), &mut diagnostic_count)?;
                None
            }
            Ok(m) => {
                let table = p.table.as_ref().expect("validated table");
                unknown(
                    table,
                    &known,
                    p.doc.path,
                    "",
                    &mut ds,
                    &mut diagnostic_count,
                )?;
                let mut note = |code, field| {
                    add(
                        &mut ds,
                        diag(p.doc.path, code, field),
                        &mut diagnostic_count,
                    )
                };
                note(DiagnosticCode::CriteriaUnresolved, None)?;
                if m.family != family.name {
                    note(DiagnosticCode::FamilyMismatch, Some("family"))?;
                }
                if ids.get(&(m.family, m.id)).is_some_and(|v| v.len() > 1) {
                    note(DiagnosticCode::DuplicateTaskId, Some("id"))?;
                }
                if incoming[i] > 0 {
                    note(DiagnosticCode::CycleOrCycleDependent, Some("blocked_by"))?;
                }
                let mut dep_seen = BTreeSet::new();
                for d in &m.deps {
                    if !dep_seen.insert(*d) {
                        note(DiagnosticCode::DuplicateDependency, Some("blocked_by"))?;
                    }
                    if ids.get(&(m.family, *d)).is_none_or(|v| v.len() != 1) {
                        note(DiagnosticCode::MissingDependency, Some(d))?;
                    }
                }
                let mut projects = BTreeSet::new();
                if m.repos.is_empty() {
                    note(DiagnosticCode::MissingRepository, Some("repos"))?;
                }
                for alias in &m.repos {
                    match family.repositories.iter().find(|r| r.name == *alias) {
                        None => note(DiagnosticCode::MissingRepository, Some(alias))?,
                        Some(r) => match &r.mapping {
                            None => note(DiagnosticCode::UnmappedRepository, Some(alias))?,
                            Some(m) => {
                                projects.insert(m.project_id.clone());
                            }
                        },
                    }
                }
                if m.status.is_some_and(|s| {
                    ![
                        "open", "claimed", "done", "blocked", "handoff", "parked", "closed",
                    ]
                    .contains(&s)
                }) {
                    note(DiagnosticCode::UnknownStatus, Some("status"))?;
                }
                if m.mode.is_some_and(|s| !["now", "night"].contains(&s)) {
                    note(DiagnosticCode::UnknownMode, Some("mode"))?;
                }
                if m.requested.zip(m.filed).is_some_and(|(a, b)| a != b) {
                    note(DiagnosticCode::RequesterAliasConflict, Some("requested_by"))?;
                }
                if table.contains_key("worked_by") || table.contains_key("triage_cost_usd") {
                    note(DiagnosticCode::ImportedHistoryUnverified, Some("worked_by"))?;
                }
                if ["", "todo", "tbd", "..."]
                    .iter()
                    .any(|s| p.body.trim().eq_ignore_ascii_case(s))
                {
                    note(DiagnosticCode::Placeholder, Some("body"))?;
                }
                Some(TodoObservation {
                    id: m.id.into(),
                    family: m.family.into(),
                    title: m.title.into(),
                    repositories: m.repos.iter().map(|s| (*s).into()).collect(),
                    blocked_by: m.deps.iter().map(|s| (*s).into()).collect(),
                    declared_status: m.status.map(str::to_owned),
                    declared_mode: m.mode.map(str::to_owned),
                    declared_merged: m.merged,
                    priority: m.priority,
                    requested_by: m.requested.map(str::to_owned),
                    legacy_filed_by: m.filed.map(str::to_owned),
                    historical_claimant: m.claim.map(str::to_owned),
                    observed_lease_until: m.lease.map(str::to_owned),
                    suggestion: (!p.body.trim().is_empty()).then(|| IntakeSuggestion {
                        format: IntakeFormat::PlainText,
                        text: p.body.into(),
                        source_span: SourceSpan {
                            start: p.body_start as u32,
                            end: p.doc.bytes.len() as u32,
                        },
                        claimed_project_id: if m.family == family.name
                            && projects.len() == 1
                            && m.repos.iter().all(|a| {
                                family
                                    .repositories
                                    .iter()
                                    .any(|r| r.name == *a && r.mapping.is_some())
                            }) {
                            projects.into_iter().next()
                        } else {
                            None
                        },
                    }),
                })
            }
        };
        records.push(RecordObservation {
            source_index: (i + 1) as u16,
            todo: observed,
            diagnostics: ds,
        });
    }
    let mut mappings: Vec<_> = input.mappings.iter().collect();
    mappings.sort_by_key(|m| (&m.family, &m.repository_alias));
    let mut result = SnapshotObservation {
        schema_version: 1,
        parser_revision: PARSER_REVISION.into(),
        queue: input.queue.clone(),
        family,
        sources,
        records,
        family_diagnostics,
        mapping_digest: content_digest(&("admiralty.todoq.mappings/v1", mappings))
            .map_err(|_| ImportError::Capacity)?,
        snapshot_digest: String::new(),
    };
    result.snapshot_digest = result.digest()?;
    bounded_json_size(&result, MAX_OUTPUT).map_err(|_| ImportError::Capacity)?;
    Ok(result)
}
