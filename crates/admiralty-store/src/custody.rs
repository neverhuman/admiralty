//! Durable recorded custody. Never grants destructive filesystem authority.
use admiralty_core::{
    PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ASSETS: i64 = 2000;
pub const MAX_COMMANDS: i64 = 20_000;
pub const MAX_PINS_PER_ASSET: i64 = 128;
pub const MAX_OBSERVATION_AGE: i64 = 86_400;
const MAX_RECEIPT_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CustodyError {
    #[error("custody SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("custody JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid custody input: {0}")]
    Invalid(&'static str),
    #[error("input exceeds supported size")]
    Oversize,
    #[error("custody counter exceeds SQLite's signed integer range")]
    Overflow,
    #[error("asset is not registered")]
    MissingAsset,
    #[error("immutable asset binding does not match")]
    ForeignAsset,
    #[error("asset observation is unknown, stale, foreign, dirty, held or regressing")]
    UnsafeObservation,
    #[error("asset revision compare-and-swap failed")]
    StaleRevision,
    #[error("asset generation does not match")]
    StaleGeneration,
    #[error("asset is already claimed for retirement")]
    RetirementClaimed,
    #[error("asset has active pins")]
    ActivePins,
    #[error("pin is missing, inactive, foreign, or belongs to another generation")]
    InvalidPin,
    #[error("command ID reused with different operation or request bytes")]
    CommandCollision,
    #[error("custody history is full; retain evidence and reconcile explicitly")]
    HistoryFull,
    #[error("custody schema or connection assumptions do not hold")]
    SchemaMismatch,
    #[error("custody receipt or projection closure is inconsistent")]
    CorruptState,
}
pub type CustodyResult<T> = std::result::Result<T, CustodyError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetBinding {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub asset_id: String,
    pub owner_id: PrincipalId,
    pub sha256: String,
    pub generation: String,
}

/// Recorded assertions only. Their exact asset/revision/clock pins prevent
/// accidental reuse; this module cannot authenticate the observer or facts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CustodyObservation {
    pub binding: AssetBinding,
    pub asset_revision: String,
    pub clock_id: String,
    pub observed_at: String,
    pub decision_at: String,
    pub max_age: String,
    pub provenance: String,
    pub registered: Option<bool>,
    pub foreign: Option<bool>,
    pub dirty: Option<bool>,
    pub held: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RegisterAssetRequest {
    pub command_id: String,
    pub binding: AssetBinding,
    pub expected_revision: String,
    pub observation: CustodyObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcquirePinRequest {
    pub command_id: String,
    pub binding: AssetBinding,
    pub pin_id: String,
    pub expected_revision: String,
    pub observation: CustodyObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePinRequest {
    pub command_id: String,
    pub binding: AssetBinding,
    pub pin_id: String,
    pub expected_revision: String,
    pub observation: CustodyObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClaimRetirementRequest {
    pub command_id: String,
    pub binding: AssetBinding,
    pub claim_id: String,
    pub expected_revision: String,
    pub observation: CustodyObservation,
}

/// Output only. Full binding and request digest describe the recorded mutation;
/// private flags cannot be set by a deserialized untrusted receipt.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CustodyReceipt {
    pub command_id: String,
    pub operation: String,
    pub request_digest: String,
    pub binding: AssetBinding,
    pub revision: String,
    pub reference_id: Option<String>,
    pub clock_id: String,
    pub observed_at: String,
    pub decision_at: String,
    pub max_age: String,
    pub provenance: String,
    recorded_only: bool,
    destructive_permission: bool,
}

#[derive(Clone)]
struct Validated {
    observed: i64,
    decision: i64,
    max_age: i64,
}
#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Register,
    Acquire,
    Release,
    Retire,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Register => "register_asset",
            Self::Acquire => "acquire_pin",
            Self::Release => "release_pin",
            Self::Retire => "claim_retirement",
        }
    }
}
trait Request: Serialize {
    fn command(&self) -> &str;
    fn binding(&self) -> &AssetBinding;
    fn expected(&self) -> &str;
    fn observation(&self) -> &CustodyObservation;
    fn reference(&self) -> Option<&str>;
}
macro_rules! request_impl {
    ($type:ty, $this:ident => $reference:expr) => {
        impl Request for $type {
            fn command(&self) -> &str {
                &self.command_id
            }
            fn binding(&self) -> &AssetBinding {
                &self.binding
            }
            fn expected(&self) -> &str {
                &self.expected_revision
            }
            fn observation(&self) -> &CustodyObservation {
                &self.observation
            }
            fn reference(&self) -> Option<&str> {
                let $this = self;
                $reference
            }
        }
    };
}
request_impl!(RegisterAssetRequest, _this => None);
request_impl!(AcquirePinRequest, this => Some(this.pin_id.as_str()));
request_impl!(ReleasePinRequest, this => Some(this.pin_id.as_str()));
request_impl!(ClaimRetirementRequest, this => Some(this.claim_id.as_str()));

// Component-owned inline schema. Existing older/partial schema is rejected,
// never guessed/migrated. No global user_version or other component is changed.
const SCHEMA: &[(&str, &str, &str)] = &[
("table", "custody_assets", "CREATE TABLE IF NOT EXISTS main.custody_assets (
 asset_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, root_id TEXT NOT NULL,
 task_id TEXT NOT NULL, owner_id TEXT NOT NULL, sha256 TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation>0), revision INTEGER NOT NULL CHECK(revision>0),
 clock_id TEXT NOT NULL, max_age INTEGER NOT NULL CHECK(max_age>0 AND max_age<=86400),
 observed_at INTEGER NOT NULL CHECK(observed_at>=0), decision_at INTEGER NOT NULL CHECK(decision_at>=observed_at),
 observation_provenance TEXT NOT NULL, retirement_claim_id TEXT,
 UNIQUE(asset_id,project_id,root_id,task_id,owner_id,sha256,generation)) STRICT"),
("table", "custody_pins", "CREATE TABLE IF NOT EXISTS main.custody_pins (
 asset_id TEXT NOT NULL, pin_id TEXT NOT NULL, project_id TEXT NOT NULL,
 root_id TEXT NOT NULL, task_id TEXT NOT NULL, owner_id TEXT NOT NULL, sha256 TEXT NOT NULL,
 generation INTEGER NOT NULL CHECK(generation>0), active INTEGER NOT NULL CHECK(active IN (0,1)),
 PRIMARY KEY(asset_id,pin_id),
 FOREIGN KEY(asset_id,project_id,root_id,task_id,owner_id,sha256,generation)
 REFERENCES custody_assets(asset_id,project_id,root_id,task_id,owner_id,sha256,generation) ON DELETE RESTRICT) STRICT"),
("index", "custody_pins_active", "CREATE INDEX IF NOT EXISTS main.custody_pins_active ON custody_pins(asset_id,active)"),
("table", "custody_commands", "CREATE TABLE IF NOT EXISTS main.custody_commands (
 command_id TEXT PRIMARY KEY, operation TEXT NOT NULL, request_digest TEXT NOT NULL,
 asset_id TEXT NOT NULL REFERENCES custody_assets(asset_id) ON DELETE RESTRICT,
 revision INTEGER NOT NULL CHECK(revision>0), receipt_json TEXT NOT NULL,
 UNIQUE(asset_id,revision)) STRICT"),
];

/// Explicit component initialization. Enables foreign keys before its own
/// transaction; refuses nested transactions and incompatible preexisting schema.
pub fn initialize(connection: &Connection) -> CustodyResult<()> {
    if !connection.is_autocommit() {
        return Err(CustodyError::SchemaMismatch);
    }
    connection.pragma_update(None, "foreign_keys", true)?;
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    verify_schema(&tx)?;
    for table in ["custody_pins", "custody_commands"] {
        if tx
            .prepare(&format!("PRAGMA main.foreign_key_check({table})"))?
            .exists([])?
        {
            return Err(CustodyError::CorruptState);
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
fn verify_schema(tx: &Transaction<'_>) -> CustodyResult<()> {
    let foreign_keys: i64 = tx.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(CustodyError::SchemaMismatch);
    }
    for (kind, name, expected) in SCHEMA {
        let actual: Option<String> = tx.query_row(
            "SELECT CASE WHEN length(CAST(sql AS BLOB))<=16384 THEN sql END FROM main.sqlite_schema WHERE type=?1 AND name=?2",
            params![kind, name], |row| row.get(0)).optional()?.flatten();
        if actual.as_deref().map(normalized_sql) != Some(normalized_sql(expected)) {
            return Err(CustodyError::SchemaMismatch);
        }
    }
    let extra_triggers: i64 = tx.query_row("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name IN ('custody_assets','custody_pins','custody_commands')", [], |row| row.get(0))?;
    if extra_triggers != 0 {
        return Err(CustodyError::SchemaMismatch);
    }
    Ok(())
}

pub fn register_asset(
    c: &mut Connection,
    request: RegisterAssetRequest,
) -> CustodyResult<CustodyReceipt> {
    apply(c, &request, Operation::Register)
}
pub fn acquire_pin(
    c: &mut Connection,
    request: AcquirePinRequest,
) -> CustodyResult<CustodyReceipt> {
    apply(c, &request, Operation::Acquire)
}
pub fn release_pin(
    c: &mut Connection,
    request: ReleasePinRequest,
) -> CustodyResult<CustodyReceipt> {
    apply(c, &request, Operation::Release)
}
pub fn claim_retirement(
    c: &mut Connection,
    request: ClaimRetirementRequest,
) -> CustodyResult<CustodyReceipt> {
    apply(c, &request, Operation::Retire)
}

fn apply<T: Request>(
    connection: &mut Connection,
    request: &T,
    operation: Operation,
) -> CustodyResult<CustodyReceipt> {
    // Streaming admission happens before serialization/hash/clone or DB access.
    bounded_json_size(request, MAX_JSON_BYTES).map_err(|_| CustodyError::Oversize)?;
    let evidence = validate_request(request, operation)?;
    if !connection.is_autocommit() {
        return Err(CustodyError::SchemaMismatch);
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_schema(&tx)?;
    let receipt = make_receipt(request, operation, &evidence)?;
    if replay(&tx, &receipt)? {
        return Ok(receipt);
    }
    let count: i64 = tx.query_row("SELECT count(*) FROM main.custody_commands", [], |row| {
        row.get(0)
    })?;
    if count >= MAX_COMMANDS {
        return Err(CustodyError::HistoryFull);
    }
    let b = request.binding();
    if operation == Operation::Register {
        let count: i64 = tx.query_row("SELECT count(*) FROM main.custody_assets", [], |row| {
            row.get(0)
        })?;
        if count >= MAX_ASSETS {
            return Err(CustodyError::HistoryFull);
        }
        let changed = tx.execute("INSERT OR IGNORE INTO main.custody_assets(asset_id,project_id,root_id,task_id,owner_id,sha256,generation,revision,clock_id,max_age,observed_at,decision_at,observation_provenance) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9,?10,?11,?12)", params![b.asset_id,b.project_id.as_str(),b.root_id.as_str(),b.task_id.as_str(),b.owner_id.as_str(),b.sha256,positive_counter(&b.generation)?,request.observation().clock_id,evidence.max_age,evidence.observed,evidence.decision,request.observation().provenance])?;
        if changed != 1 {
            return Err(CustodyError::ForeignAsset);
        }
    } else {
        let current = load_asset(&tx, b)?;
        if current.revision != positive_counter(request.expected())? {
            return Err(CustodyError::StaleRevision);
        }
        if current.claim.is_some() {
            return Err(CustodyError::RetirementClaimed);
        }
        if current.clock != request.observation().clock_id
            || current.max_age != evidence.max_age
            || evidence.observed < current.decision
            || evidence.decision < current.decision
        {
            return Err(CustodyError::UnsafeObservation);
        }
        let reference = request
            .reference()
            .ok_or(CustodyError::Invalid("reference missing"))?;
        match operation {
            Operation::Acquire => {
                if current.pin_count >= MAX_PINS_PER_ASSET {
                    return Err(CustodyError::HistoryFull);
                }
                let changed = tx.execute("INSERT OR IGNORE INTO main.custody_pins(asset_id,pin_id,project_id,root_id,task_id,owner_id,sha256,generation,active) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1)", params![b.asset_id,reference,b.project_id.as_str(),b.root_id.as_str(),b.task_id.as_str(),b.owner_id.as_str(),b.sha256,positive_counter(&b.generation)?])?;
                if changed != 1 {
                    return Err(CustodyError::InvalidPin);
                }
            }
            Operation::Release => {
                let changed = tx.execute("UPDATE main.custody_pins SET active=0 WHERE asset_id=?1 AND pin_id=?2 AND project_id=?3 AND root_id=?4 AND task_id=?5 AND owner_id=?6 AND sha256=?7 AND generation=?8 AND active=1", params![b.asset_id,reference,b.project_id.as_str(),b.root_id.as_str(),b.task_id.as_str(),b.owner_id.as_str(),b.sha256,positive_counter(&b.generation)?])?;
                if changed != 1 {
                    return Err(CustodyError::InvalidPin);
                }
            }
            Operation::Retire if current.active_pins != 0 => return Err(CustodyError::ActivePins),
            _ => {}
        }
        let claim = (operation == Operation::Retire).then_some(reference);
        let changed = tx.execute("UPDATE main.custody_assets SET revision=?1,observed_at=?2,decision_at=?3,observation_provenance=?4,retirement_claim_id=?5 WHERE asset_id=?6 AND revision=?7 AND retirement_claim_id IS NULL", params![positive_counter(&receipt.revision)?,evidence.observed,evidence.decision,request.observation().provenance,claim,b.asset_id,current.revision])?;
        if changed != 1 {
            return Err(CustodyError::StaleRevision);
        }
    }
    let encoded = serde_json::to_string(&receipt)?;
    tx.execute("INSERT INTO main.custody_commands(command_id,operation,request_digest,asset_id,revision,receipt_json) VALUES(?1,?2,?3,?4,?5,?6)", params![receipt.command_id,receipt.operation,receipt.request_digest,b.asset_id,positive_counter(&receipt.revision)?,encoded])?;
    // Closure is checked before commit too; any inconsistency rolls everything back.
    load_asset(&tx, b)?;
    tx.commit()?;
    Ok(receipt)
}
fn bounded_id(value: &str) -> CustodyResult<()> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
    {
        return Err(CustodyError::Invalid("identifier bounds"));
    }
    Ok(())
}
fn validate_binding(b: &AssetBinding) -> CustodyResult<()> {
    bounded_id(&b.asset_id)?;
    if b.sha256.len() != 71
        || !b.sha256.starts_with("sha256:")
        || !b.sha256[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CustodyError::Invalid("full lowercase SHA-256 required"));
    }
    positive_counter(&b.generation)?;
    Ok(())
}
fn counter(value: &str) -> CustodyResult<i64> {
    if value.len() > 20 {
        return Err(CustodyError::Overflow);
    }
    let number = value
        .parse::<u64>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or(CustodyError::Invalid("canonical decimal counter"))?;
    i64::try_from(number).map_err(|_| CustodyError::Overflow)
}
fn positive_counter(value: &str) -> CustodyResult<i64> {
    let number = counter(value)?;
    if number == 0 {
        return Err(CustodyError::Invalid("positive counter required"));
    }
    Ok(number)
}
fn validate_request<T: Request>(request: &T, operation: Operation) -> CustodyResult<Validated> {
    bounded_id(request.command())?;
    if let Some(reference) = request.reference() {
        bounded_id(reference)?;
    }
    validate_binding(request.binding())?;
    let o = request.observation();
    validate_binding(&o.binding)?;
    if o.binding.generation != request.binding().generation {
        return Err(CustodyError::StaleGeneration);
    }
    if o.binding != *request.binding() {
        return Err(CustodyError::ForeignAsset);
    }
    if o.asset_revision != request.expected() {
        return Err(CustodyError::StaleRevision);
    }
    let expected = counter(request.expected())?;
    if (operation == Operation::Register) != (expected == 0) {
        return Err(CustodyError::StaleRevision);
    }
    bounded_id(&o.clock_id)?;
    bounded_id(&o.provenance)?;
    if o.registered != Some(operation != Operation::Register)
        || o.foreign != Some(false)
        || o.dirty != Some(false)
        || o.held != Some(false)
    {
        return Err(CustodyError::UnsafeObservation);
    }
    let observed = counter(&o.observed_at)?;
    let decision = counter(&o.decision_at)?;
    let max_age = positive_counter(&o.max_age)?;
    if max_age > MAX_OBSERVATION_AGE || observed > decision || decision - observed > max_age {
        return Err(CustodyError::UnsafeObservation);
    }
    Ok(Validated {
        observed,
        decision,
        max_age,
    })
}
struct Current {
    revision: i64,
    observed: i64,
    decision: i64,
    clock: String,
    max_age: i64,
    claim: Option<String>,
    pin_count: i64,
    active_pins: i64,
}
struct AssetRow {
    project: String,
    root: String,
    task: String,
    owner: String,
    digest: String,
    generation: i64,
    revision: i64,
    clock: String,
    max_age: i64,
    observed: i64,
    decision: i64,
    claim: Option<String>,
    has_claim: bool,
    provenance: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredReceipt {
    command_id: String,
    operation: String,
    request_digest: String,
    binding: AssetBinding,
    revision: String,
    reference_id: Option<String>,
    clock_id: String,
    observed_at: String,
    decision_at: String,
    max_age: String,
    provenance: String,
    recorded_only: bool,
    destructive_permission: bool,
}
fn check_stored_receipt(encoded: &str) -> CustodyResult<CustodyReceipt> {
    if encoded.len() > MAX_RECEIPT_BYTES {
        return Err(CustodyError::Oversize);
    }
    let record: StoredReceipt = serde_json::from_str(encoded)?;
    if !record.recorded_only || record.destructive_permission {
        return Err(CustodyError::CorruptState);
    }
    let revision = positive_counter(&record.revision)?;
    let expected = (revision - 1).to_string();
    let observation = CustodyObservation {
        binding: record.binding.clone(),
        asset_revision: expected.clone(),
        clock_id: record.clock_id.clone(),
        observed_at: record.observed_at.clone(),
        decision_at: record.decision_at.clone(),
        max_age: record.max_age.clone(),
        provenance: record.provenance.clone(),
        registered: Some(revision != 1),
        foreign: Some(false),
        dirty: Some(false),
        held: Some(false),
    };
    macro_rules! reconstruct {
        ($request:expr,$operation:expr) => {{
            let request = $request;
            let evidence = validate_request(&request, $operation)?;
            make_receipt(&request, $operation, &evidence)?
        }};
    }
    let receipt = match record.operation.as_str() {
        "register_asset" if record.reference_id.is_none() => reconstruct!(
            RegisterAssetRequest {
                command_id: record.command_id,
                binding: record.binding,
                expected_revision: expected,
                observation,
            },
            Operation::Register
        ),
        "acquire_pin" => reconstruct!(
            AcquirePinRequest {
                command_id: record.command_id,
                binding: record.binding,
                pin_id: record.reference_id.ok_or(CustodyError::CorruptState)?,
                expected_revision: expected,
                observation,
            },
            Operation::Acquire
        ),
        "release_pin" => reconstruct!(
            ReleasePinRequest {
                command_id: record.command_id,
                binding: record.binding,
                pin_id: record.reference_id.ok_or(CustodyError::CorruptState)?,
                expected_revision: expected,
                observation,
            },
            Operation::Release
        ),
        "claim_retirement" => reconstruct!(
            ClaimRetirementRequest {
                command_id: record.command_id,
                binding: record.binding,
                claim_id: record.reference_id.ok_or(CustodyError::CorruptState)?,
                expected_revision: expected,
                observation,
            },
            Operation::Retire
        ),
        _ => return Err(CustodyError::CorruptState),
    };
    if receipt.request_digest != record.request_digest
        || serde_json::to_string(&receipt)? != encoded
    {
        return Err(CustodyError::CorruptState);
    }
    Ok(receipt)
}
fn load_asset(tx: &Transaction<'_>, b: &AssetBinding) -> CustodyResult<Current> {
    let row:Option<AssetRow>=tx.query_row(
        "SELECT CASE WHEN length(CAST(project_id AS BLOB))<=128 THEN project_id END,CASE WHEN length(CAST(root_id AS BLOB))<=128 THEN root_id END,CASE WHEN length(CAST(task_id AS BLOB))<=128 THEN task_id END,CASE WHEN length(CAST(owner_id AS BLOB))<=128 THEN owner_id END,CASE WHEN length(CAST(sha256 AS BLOB))=71 THEN sha256 END,generation,revision,CASE WHEN length(CAST(clock_id AS BLOB))<=128 THEN clock_id END,max_age,observed_at,decision_at,CASE WHEN length(CAST(retirement_claim_id AS BLOB))<=128 THEN retirement_claim_id END,retirement_claim_id IS NOT NULL,CASE WHEN length(CAST(observation_provenance AS BLOB))<=128 THEN observation_provenance END FROM main.custody_assets WHERE asset_id=?1",
        [&b.asset_id],|r|Ok(AssetRow{project:r.get(0)?,root:r.get(1)?,task:r.get(2)?,owner:r.get(3)?,digest:r.get(4)?,generation:r.get(5)?,revision:r.get(6)?,clock:r.get(7)?,max_age:r.get(8)?,observed:r.get(9)?,decision:r.get(10)?,claim:r.get(11)?,has_claim:r.get(12)?,provenance:r.get(13)?})).optional()?;
    let Some(row) = row else {
        return Err(CustodyError::MissingAsset);
    };
    if row.project != b.project_id.as_str()
        || row.root != b.root_id.as_str()
        || row.task != b.task_id.as_str()
        || row.owner != b.owner_id.as_str()
        || row.digest != b.sha256
    {
        return Err(CustodyError::ForeignAsset);
    }
    if row.generation != positive_counter(&b.generation)? {
        return Err(CustodyError::StaleGeneration);
    }
    if row.has_claim != row.claim.is_some()
        || row.revision <= 0
        || row.revision > 2 * MAX_PINS_PER_ASSET + 2
    {
        return Err(CustodyError::CorruptState);
    }
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM main.custody_commands WHERE asset_id=?1",
        [&b.asset_id],
        |r| r.get(0),
    )?;
    if count != row.revision {
        return Err(CustodyError::CorruptState);
    }
    // Pin IDs are lifetime identities; at most 128 acquire/release pairs plus
    // registration and one retirement. Full replay is therefore bounded to 258
    // small receipts, not the unbounded transcript or global command history.
    let mut pins = std::collections::BTreeMap::<String, bool>::new();
    let mut claim = None;
    let mut last: Option<CustodyReceipt> = None;
    let mut query=tx.prepare("SELECT command_id,operation,request_digest,revision,CASE WHEN length(CAST(receipt_json AS BLOB))<=16384 THEN receipt_json END FROM main.custody_commands WHERE asset_id=?1 ORDER BY revision")?;
    let mut rows = query.query([&b.asset_id])?;
    let mut index = 0_i64;
    while let Some(r) = rows.next()? {
        index += 1;
        // Bound all loaded text columns in SQL, before allocating Rust strings.
        let encoded: Option<String> = r.get(4)?;
        let encoded = encoded.ok_or(CustodyError::CorruptState)?;
        let receipt = check_stored_receipt(&encoded)?;
        // Compare metadata as borrowed SQLite text, avoiding separate unbounded allocations.
        if r.get_ref(0)?
            .as_str()
            .map_err(|_| CustodyError::CorruptState)?
            != receipt.command_id
            || r.get_ref(1)?
                .as_str()
                .map_err(|_| CustodyError::CorruptState)?
                != receipt.operation
            || r.get_ref(2)?
                .as_str()
                .map_err(|_| CustodyError::CorruptState)?
                != receipt.request_digest
            || r.get::<_, i64>(3)? != index
            || receipt.revision != index.to_string()
            || receipt.binding != *b
            || claim.is_some()
        {
            return Err(CustodyError::CorruptState);
        }
        if let Some(previous) = &last
            && (receipt.clock_id != previous.clock_id
                || receipt.max_age != previous.max_age
                || counter(&receipt.observed_at)? < counter(&previous.decision_at)?)
        {
            return Err(CustodyError::CorruptState);
        }
        match receipt.operation.as_str() {
            "register_asset" if index == 1 => {}
            "acquire_pin" if index > 1 => {
                let pin = receipt
                    .reference_id
                    .clone()
                    .ok_or(CustodyError::CorruptState)?;
                if pins.len() >= MAX_PINS_PER_ASSET as usize || pins.insert(pin, true).is_some() {
                    return Err(CustodyError::CorruptState);
                }
            }
            "release_pin" if index > 1 => {
                let pin = receipt
                    .reference_id
                    .as_ref()
                    .ok_or(CustodyError::CorruptState)?;
                if pins.get(pin) != Some(&true) {
                    return Err(CustodyError::CorruptState);
                }
                pins.insert(pin.clone(), false);
            }
            "claim_retirement" if index > 1 && !pins.values().any(|active| *active) => {
                claim = receipt.reference_id.clone();
                if claim.is_none() {
                    return Err(CustodyError::CorruptState);
                }
            }
            _ => return Err(CustodyError::CorruptState),
        }
        last = Some(receipt);
    }
    let last = last.ok_or(CustodyError::CorruptState)?;
    if last.clock_id != row.clock
        || counter(&last.max_age)? != row.max_age
        || counter(&last.observed_at)? != row.observed
        || counter(&last.decision_at)? != row.decision
        || last.provenance != row.provenance
        || claim != row.claim
    {
        return Err(CustodyError::CorruptState);
    }
    let foreign:i64=tx.query_row("SELECT count(*) FROM main.custody_pins WHERE asset_id=?1 AND (project_id<>?2 OR root_id<>?3 OR task_id<>?4 OR owner_id<>?5 OR sha256<>?6 OR generation<>?7)",params![b.asset_id,b.project_id.as_str(),b.root_id.as_str(),b.task_id.as_str(),b.owner_id.as_str(),b.sha256,row.generation],|r|r.get(0))?;
    if foreign != 0 {
        return Err(CustodyError::CorruptState);
    }
    let pin_count: i64 = tx.query_row(
        "SELECT count(*) FROM main.custody_pins WHERE asset_id=?1",
        [&b.asset_id],
        |r| r.get(0),
    )?;
    if pin_count != pins.len() as i64 || pin_count > MAX_PINS_PER_ASSET {
        return Err(CustodyError::CorruptState);
    }
    let mut query=tx.prepare("SELECT CASE WHEN length(CAST(pin_id AS BLOB))<=128 THEN pin_id END,active FROM main.custody_pins WHERE asset_id=?1")?;
    let mut rows = query.query([&b.asset_id])?;
    while let Some(r) = rows.next()? {
        let pin: String = r.get(0)?;
        let active: i64 = r.get(1)?;
        if !matches!(active, 0 | 1) || pins.get(&pin) != Some(&(active == 1)) {
            return Err(CustodyError::CorruptState);
        }
    }
    Ok(Current {
        revision: row.revision,
        observed: row.observed,
        decision: row.decision,
        clock: row.clock,
        max_age: row.max_age,
        claim: row.claim,
        pin_count,
        active_pins: pins.values().filter(|active| **active).count() as i64,
    })
}
fn make_receipt<T: Request>(
    request: &T,
    operation: Operation,
    e: &Validated,
) -> CustodyResult<CustodyReceipt> {
    let revision = counter(request.expected())?
        .checked_add(1)
        .ok_or(CustodyError::Overflow)?;
    let o = request.observation();
    Ok(CustodyReceipt {
        command_id: request.command().to_owned(),
        operation: operation.name().to_owned(),
        request_digest: content_digest(request)?,
        binding: request.binding().clone(),
        revision: revision.to_string(),
        reference_id: request.reference().map(str::to_owned),
        clock_id: o.clock_id.clone(),
        observed_at: e.observed.to_string(),
        decision_at: e.decision.to_string(),
        max_age: e.max_age.to_string(),
        provenance: o.provenance.clone(),
        recorded_only: true,
        destructive_permission: false,
    })
}
fn replay(tx: &Transaction<'_>, expected: &CustodyReceipt) -> CustodyResult<bool> {
    let row:Option<(String,String,String,i64,Option<String>)>=tx.query_row(
        "SELECT CASE WHEN length(CAST(operation AS BLOB))<=32 THEN operation END,CASE WHEN length(CAST(request_digest AS BLOB))=71 THEN request_digest END,CASE WHEN length(CAST(asset_id AS BLOB))<=128 THEN asset_id END,revision,CASE WHEN length(CAST(receipt_json AS BLOB))<=16384 THEN receipt_json END FROM main.custody_commands WHERE command_id=?1",
        [&expected.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((operation, digest, asset, revision, encoded)) = row else {
        return Ok(false);
    };
    if operation != expected.operation || digest != expected.request_digest {
        return Err(CustodyError::CommandCollision);
    }
    bounded_json_size(expected, MAX_RECEIPT_BYTES).map_err(|_| CustodyError::Oversize)?;
    if asset != expected.binding.asset_id
        || revision != positive_counter(&expected.revision)?
        || encoded.as_deref() != Some(serde_json::to_string(expected)?.as_str())
    {
        return Err(CustodyError::CorruptState);
    }
    let current = load_asset(tx, &expected.binding)?;
    if current.revision < revision
        || current.observed < counter(&expected.observed_at)?
        || current.decision < counter(&expected.decision_at)?
        || current.clock != expected.clock_id
        || current.max_age != counter(&expected.max_age)?
    {
        return Err(CustodyError::CorruptState);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };
    use tempfile::TempDir;

    fn db() -> (TempDir, Connection) {
        let dir = tempfile::tempdir().expect("create SQLite fixture directory");
        let path = dir.path().join("custody.sqlite");
        let connection = Connection::open(path).expect("open SQLite fixture");
        connection
            .busy_timeout(Duration::from_secs(5))
            .expect("set bounded fixture busy timeout");
        initialize(&connection).expect("initialize custody fixture schema");
        (dir, connection)
    }
    fn binding() -> AssetBinding {
        AssetBinding {
            project_id: ProjectId::new("project-1").expect("valid fixture project ID"),
            root_id: RootId::new("root-1").expect("valid fixture root ID"),
            task_id: TaskId::new("task-1").expect("valid fixture task ID"),
            asset_id: "asset-1".into(),
            owner_id: PrincipalId::new("owner-1").expect("valid fixture owner ID"),
            sha256: format!("sha256:{}", "a".repeat(64)),
            generation: "1".into(),
        }
    }
    fn observation(registered: bool, at: u64) -> CustodyObservation {
        CustodyObservation {
            binding: binding(),
            asset_revision: (at - 10).to_string(),
            clock_id: "fixture-clock".into(),
            observed_at: at.to_string(),
            decision_at: (at + 1).to_string(),
            max_age: "10".into(),
            provenance: "inventory:fixture-1".into(),
            registered: Some(registered),
            foreign: Some(false),
            dirty: Some(false),
            held: Some(false),
        }
    }
    fn register(connection: &mut Connection) -> CustodyReceipt {
        register_asset(
            connection,
            RegisterAssetRequest {
                command_id: "register-1".into(),
                binding: binding(),
                expected_revision: "0".into(),
                observation: observation(false, 10),
            },
        )
        .expect("register clean synthetic asset")
    }
    fn acquire_request(cmd: &str, rev: &str) -> AcquirePinRequest {
        AcquirePinRequest {
            command_id: cmd.into(),
            binding: binding(),
            pin_id: format!("pin-{cmd}"),
            expected_revision: rev.into(),
            observation: observation(true, 10 + rev.parse::<u64>().expect("fixture revision")),
        }
    }
    fn claim_request(cmd: &str, rev: &str) -> ClaimRetirementRequest {
        ClaimRetirementRequest {
            command_id: cmd.into(),
            binding: binding(),
            claim_id: format!("claim-{cmd}"),
            expected_revision: rev.into(),
            observation: observation(true, 10 + rev.parse::<u64>().expect("fixture revision")),
        }
    }
    fn release_request(cmd: &str, rev: &str, pin: &str) -> ReleasePinRequest {
        ReleasePinRequest {
            command_id: cmd.into(),
            binding: binding(),
            pin_id: pin.into(),
            expected_revision: rev.into(),
            observation: observation(true, 10 + rev.parse::<u64>().expect("fixture revision")),
        }
    }

    #[test]
    fn registration_reopen_exact_replay_and_command_collision() {
        let (dir, mut connection) = db();
        let first = register(&mut connection);
        drop(connection);
        let path = dir.path().join("custody.sqlite");
        let mut reopened = Connection::open(path).expect("reopen durable fixture database");
        reopened
            .busy_timeout(Duration::from_secs(5))
            .expect("set reopen timeout");
        initialize(&reopened).expect("reinitialize idempotent schema");
        let replay = register_asset(
            &mut reopened,
            RegisterAssetRequest {
                command_id: "register-1".into(),
                binding: binding(),
                expected_revision: "0".into(),
                observation: observation(false, 10),
            },
        )
        .expect("exact replay returns stored receipt");
        assert_eq!(first, replay);
        let mut changed = RegisterAssetRequest {
            command_id: "register-1".into(),
            binding: AssetBinding {
                sha256: format!("sha256:{}", "b".repeat(64)),
                ..binding()
            },
            expected_revision: "0".into(),
            observation: observation(false, 10),
        };
        changed.observation.binding = changed.binding.clone();
        assert!(matches!(
            register_asset(&mut reopened, changed),
            Err(CustodyError::CommandCollision)
        ));
    }

    #[test]
    fn pin_blocks_claim_then_release_allows_claim_and_claim_blocks_pin() {
        let (_dir, mut connection) = db();
        register(&mut connection);
        let pin = acquire_pin(&mut connection, acquire_request("acquire", "1"))
            .expect("acquire active pin");
        assert!(matches!(
            claim_retirement(&mut connection, claim_request("blocked", "2")),
            Err(CustodyError::ActivePins)
        ));
        let pin_id = pin.reference_id.as_deref().expect("pin receipt ID");
        let mut foreign_release = release_request("foreign-release", "2", pin_id);
        foreign_release.binding.owner_id =
            PrincipalId::new("other-owner").expect("valid foreign owner fixture");
        assert!(matches!(
            release_pin(&mut connection, foreign_release),
            Err(CustodyError::ForeignAsset)
        ));
        let mut wrong_generation = release_request("wrong-generation", "2", pin_id);
        wrong_generation.binding.generation = "2".into();
        assert!(matches!(
            release_pin(&mut connection, wrong_generation),
            Err(CustodyError::StaleGeneration)
        ));
        let released = release_pin(&mut connection, release_request("release", "2", pin_id))
            .expect("release own generation pin");
        assert_eq!(released.revision, "3");
        claim_retirement(&mut connection, claim_request("claim", "3"))
            .expect("claim after final pin release");
        assert!(matches!(
            acquire_pin(&mut connection, acquire_request("late-pin", "4")),
            Err(CustodyError::RetirementClaimed)
        ));
    }

    #[test]
    fn unknown_stale_foreign_dirty_hold_and_cas_denials_do_not_mutate() {
        let (_dir, mut connection) = db();
        register(&mut connection);
        let mut dirty = acquire_request("dirty", "1");
        dirty.observation.dirty = Some(true);
        assert!(matches!(
            acquire_pin(&mut connection, dirty),
            Err(CustodyError::UnsafeObservation)
        ));
        let mut unknown = acquire_request("unknown", "1");
        unknown.observation.held = None;
        assert!(matches!(
            acquire_pin(&mut connection, unknown),
            Err(CustodyError::UnsafeObservation)
        ));
        let mut stale = acquire_request("stale", "1");
        stale.observation.observed_at = "0".into();
        stale.observation.decision_at = "100".into();
        assert!(matches!(
            acquire_pin(&mut connection, stale),
            Err(CustodyError::UnsafeObservation)
        ));
        let mut foreign = acquire_request("foreign", "1");
        foreign.binding.project_id = ProjectId::new("other").expect("valid foreign fixture ID");
        assert!(matches!(
            acquire_pin(&mut connection, foreign),
            Err(CustodyError::ForeignAsset)
        ));
        assert!(matches!(
            acquire_pin(&mut connection, acquire_request("wrong-cas", "2")),
            Err(CustodyError::StaleRevision)
        ));
        let success = acquire_pin(&mut connection, acquire_request("after-denials", "1"))
            .expect("denials left revision and pins unchanged");
        assert_eq!(success.revision, "2");
    }

    #[test]
    fn counter_overflow_and_identifier_bounds_fail_before_mutation() {
        let (_dir, mut connection) = db();
        register(&mut connection);
        let mut overflow = acquire_request("overflow", "1");
        overflow.binding.generation = "9223372036854775808".into();
        assert!(matches!(
            acquire_pin(&mut connection, overflow),
            Err(CustodyError::Overflow)
        ));
        let mut oversized = acquire_request("oversized", "1");
        oversized.pin_id = "p".repeat(MAX_ID_BYTES + 1);
        assert!(matches!(
            acquire_pin(&mut connection, oversized),
            Err(CustodyError::Invalid("identifier bounds"))
        ));
        let receipt = acquire_pin(&mut connection, acquire_request("after-invalid", "1"))
            .expect("invalid requests made no mutation");
        assert_eq!(receipt.revision, "2");
    }

    #[test]
    fn observations_pin_full_scope_revision_clock_policy_and_retained_time_floor() {
        let (directory, mut connection) = db();
        register(&mut connection);
        for field in ["asset", "project", "root", "task", "owner", "content"] {
            let mut request = acquire_request("foreign-observation", "1");
            match field {
                "asset" => request.observation.binding.asset_id = "other".into(),
                "project" => {
                    request.observation.binding.project_id =
                        ProjectId::new("other").expect("fixture project")
                }
                "root" => {
                    request.observation.binding.root_id =
                        RootId::new("other").expect("fixture root")
                }
                "task" => {
                    request.observation.binding.task_id =
                        TaskId::new("other").expect("fixture task")
                }
                "owner" => {
                    request.observation.binding.owner_id =
                        PrincipalId::new("other").expect("fixture owner")
                }
                _ => request.observation.binding.sha256 = format!("sha256:{}", "b".repeat(64)),
            }
            assert!(matches!(
                acquire_pin(&mut connection, request),
                Err(CustodyError::ForeignAsset)
            ));
        }
        let pin = acquire_pin(&mut connection, acquire_request("floor-pin", "1"))
            .expect("clean pin fixture");
        let pin_id = pin.reference_id.as_deref().expect("fixture pin reference");
        for issue in ["time", "clock", "policy", "revision"] {
            let mut request = release_request("rejected-floor", "2", pin_id);
            match issue {
                "time" => {
                    request.observation.observed_at = "10".into();
                    request.observation.decision_at = "11".into();
                }
                "clock" => request.observation.clock_id = "new-clock".into(),
                "policy" => request.observation.max_age = "11".into(),
                _ => request.observation.asset_revision = "1".into(),
            }
            assert!(matches!(
                release_pin(&mut connection, request),
                Err(CustodyError::UnsafeObservation | CustodyError::StaleRevision)
            ));
        }
        drop(connection);
        let mut reopened = Connection::open(directory.path().join("custody.sqlite"))
            .expect("reopen retained floor");
        initialize(&reopened).expect("explicit reopen initialization");
        let mut old = release_request("old-after-reopen", "2", pin_id);
        old.observation.observed_at = "10".into();
        old.observation.decision_at = "11".into();
        assert!(matches!(
            release_pin(&mut reopened, old),
            Err(CustodyError::UnsafeObservation)
        ));
        let receipt = release_pin(
            &mut reopened,
            release_request("release-after-reopen", "2", pin_id),
        )
        .expect("fresh release at retained floor");
        assert_eq!(receipt.revision, "3");
        assert_eq!(receipt.binding, binding());
        let json = serde_json::to_value(receipt).expect("receipt JSON");
        assert_eq!(json["recorded_only"], true);
        assert_eq!(json["destructive_permission"], false);
    }

    #[test]
    fn historical_replays_never_rewind_current_pin_claim_or_clock_projection() {
        let (_directory, mut connection) = db();
        let registered = register(&mut connection);
        let request = acquire_request("historical-pin", "1");
        let pinned = acquire_pin(&mut connection, request.clone()).expect("acquire fixture pin");
        let release = release_request(
            "historical-release",
            "2",
            pinned.reference_id.as_deref().expect("fixture pin"),
        );
        let released = release_pin(&mut connection, release.clone()).expect("release fixture pin");
        let retired = claim_retirement(&mut connection, claim_request("historical-claim", "3"))
            .expect("record fixture retirement");
        let projection = |connection: &Connection| -> (i64, i64, i64, String, i64) {
            connection.query_row("SELECT revision,observed_at,decision_at,retirement_claim_id,(SELECT active FROM main.custody_pins WHERE asset_id='asset-1') FROM main.custody_assets WHERE asset_id='asset-1'",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).expect("current projection fixture")
        };
        let before = projection(&connection);
        assert_eq!(
            acquire_pin(&mut connection, request).expect("historical acquire replay"),
            pinned
        );
        assert_eq!(
            release_pin(&mut connection, release).expect("historical release replay"),
            released
        );
        assert_eq!(register(&mut connection), registered);
        assert_eq!(
            claim_retirement(&mut connection, claim_request("historical-claim", "3"))
                .expect("claim replay"),
            retired
        );
        assert_eq!(projection(&connection), before);
        assert_eq!(before, (4, 13, 14, "claim-historical-claim".into(), 0));
    }

    #[test]
    fn corrupted_receipts_missing_pins_and_rewound_projections_fail_closed() {
        for issue in [
            "receipt-flags",
            "receipt-binding",
            "oversized",
            "pin",
            "rewind",
            "clock",
            "claim",
        ] {
            let (_directory, mut connection) = db();
            register(&mut connection);
            let request = acquire_request("corruption-pin", "1");
            acquire_pin(&mut connection, request.clone()).expect("pin before injected corruption");
            match issue {
                "receipt-flags" => {
                    connection.execute("UPDATE main.custody_commands SET receipt_json=json_set(receipt_json,'$.destructive_permission',json('true')) WHERE command_id='corruption-pin'",[]).expect("inject forged receipt flag");
                }
                "receipt-binding" => {
                    connection.execute("UPDATE main.custody_commands SET receipt_json=json_set(receipt_json,'$.binding.owner_id','other') WHERE command_id='register-1'",[]).expect("inject unrelated historical receipt corruption");
                }
                "oversized" => {
                    connection.execute("UPDATE main.custody_commands SET receipt_json=?1 WHERE command_id='corruption-pin'",["x".repeat(MAX_RECEIPT_BYTES+1)]).expect("inject bounded oversized stored row");
                }
                "pin" => {
                    connection
                        .execute("DELETE FROM main.custody_pins WHERE asset_id='asset-1'", [])
                        .expect("inject missing pin projection");
                }
                "rewind" => {
                    connection.execute("UPDATE main.custody_assets SET revision=1,observed_at=10,decision_at=11 WHERE asset_id='asset-1'",[]).expect("inject projection rewind");
                }
                "clock" => {
                    connection.execute("UPDATE main.custody_assets SET clock_id='other-clock' WHERE asset_id='asset-1'",[]).expect("inject clock substitution");
                }
                _ => {
                    connection.execute("UPDATE main.custody_assets SET retirement_claim_id=?1 WHERE asset_id='asset-1'",["c".repeat(129)]).expect("inject oversized non-null claim");
                }
            }
            assert!(
                acquire_pin(&mut connection, request).is_err(),
                "historical replay must reject {issue}"
            );
            assert!(
                release_pin(
                    &mut connection,
                    release_request("after-corruption", "2", "pin-corruption-pin")
                )
                .is_err(),
                "new mutation must reject {issue}"
            );
            let count: i64 = connection
                .query_row("SELECT count(*) FROM main.custody_commands", [], |row| {
                    row.get(0)
                })
                .expect("corrupted fixture command count");
            assert_eq!(count, 2, "corruption denials cannot append a receipt");
        }
    }

    #[test]
    fn schema_fk_transaction_and_temp_shadow_assumptions_are_explicit() {
        let connection = Connection::open_in_memory().expect("schema fixture");
        connection
            .execute_batch(
                "PRAGMA user_version=77;CREATE TABLE custody_assets(asset_id TEXT PRIMARY KEY)",
            )
            .expect("legacy schema fixture");
        assert!(matches!(
            initialize(&connection),
            Err(CustodyError::SchemaMismatch)
        ));
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("global version");
        assert_eq!(version, 77);
        let (_directory, mut connection) = db();
        connection.execute_batch("CREATE TEMP TABLE custody_assets(marker TEXT);INSERT INTO temp.custody_assets VALUES('preserve');PRAGMA foreign_keys=OFF").expect("foreign key and shadow fixtures");
        let request = RegisterAssetRequest {
            command_id: "new-registration".into(),
            binding: binding(),
            expected_revision: "0".into(),
            observation: observation(false, 10),
        };
        assert!(matches!(
            register_asset(&mut connection, request.clone()),
            Err(CustodyError::SchemaMismatch)
        ));
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("restore FK");
        connection
            .execute_batch("BEGIN")
            .expect("begin caller transaction");
        assert!(matches!(
            register_asset(&mut connection, request.clone()),
            Err(CustodyError::SchemaMismatch)
        ));
        assert!(matches!(
            initialize(&connection),
            Err(CustodyError::SchemaMismatch)
        ));
        connection.execute_batch("ROLLBACK;CREATE TABLE extra_effects(marker TEXT);CREATE TRIGGER custody_hidden_effect AFTER INSERT ON main.custody_assets BEGIN INSERT INTO extra_effects VALUES('bad'); END").expect("unexpected trigger fixture");
        assert!(matches!(
            register_asset(&mut connection, request.clone()),
            Err(CustodyError::SchemaMismatch)
        ));
        connection
            .execute_batch("DROP TRIGGER custody_hidden_effect")
            .expect("remove test-owned trigger");
        register_asset(&mut connection, request)
            .expect("qualified component schema ignores temp shadow");
        let marker: String = connection
            .query_row("SELECT marker FROM temp.custody_assets", [], |row| {
                row.get(0)
            })
            .expect("temp table preserved");
        assert_eq!(marker, "preserve");
        let count: i64 = connection
            .query_row("SELECT count(*) FROM extra_effects", [], |row| row.get(0))
            .expect("hidden effect count");
        assert_eq!(count, 0);
    }

    #[test]
    fn streaming_admission_and_lifetime_pin_history_are_bounded() {
        let (_directory, mut connection) = db();
        register(&mut connection);
        let mut oversized = acquire_request("too-large", "1");
        oversized.observation.provenance = "x".repeat(MAX_JSON_BYTES + 1);
        assert!(matches!(
            acquire_pin(&mut connection, oversized),
            Err(CustodyError::Oversize)
        ));
        for index in 0..MAX_PINS_PER_ASSET {
            acquire_pin(
                &mut connection,
                acquire_request(&format!("bounded-{index}"), &(index + 1).to_string()),
            )
            .expect("bounded active pin fixture");
        }
        let revision = (MAX_PINS_PER_ASSET + 1).to_string();
        assert!(matches!(
            acquire_pin(&mut connection, acquire_request("full", &revision)),
            Err(CustodyError::HistoryFull)
        ));
        assert!(matches!(
            claim_retirement(&mut connection, claim_request("held", &revision)),
            Err(CustodyError::ActivePins)
        ));
        let receipt = release_pin(
            &mut connection,
            release_request("release-full", &revision, "pin-bounded-0"),
        )
        .expect("full pin history still permits release");
        assert!(matches!(
            acquire_pin(
                &mut connection,
                acquire_request("still-full", &receipt.revision)
            ),
            Err(CustodyError::HistoryFull)
        ));
        let count: i64 = connection
            .query_row("SELECT count(*) FROM main.custody_pins", [], |row| {
                row.get(0)
            })
            .expect("bounded pin count");
        assert_eq!(count, MAX_PINS_PER_ASSET);
    }

    #[test]
    fn pin_and_claim_race_serializes_on_two_real_connections() {
        let dir = tempfile::tempdir().expect("create race fixture directory");
        for round in 0..2 {
            // Each independent pair races from revision 1 on two real connections.
            let race_path = dir.path().join(format!("race-{round}.sqlite"));
            let mut init = Connection::open(&race_path).expect("open race database");
            init.busy_timeout(Duration::from_secs(5))
                .expect("set bounded init timeout");
            initialize(&init).expect("initialize concurrent race schema");
            register(&mut init);
            drop(init);
            let barrier = Arc::new(Barrier::new(3));
            let path_a = race_path.clone();
            let barrier_a = Arc::clone(&barrier);
            let pin_thread = thread::spawn(move || {
                let mut c = Connection::open(path_a).expect("open pin-racer connection");
                c.pragma_update(None, "foreign_keys", true)
                    .expect("enable pin-racer FK");
                c.busy_timeout(Duration::from_secs(5))
                    .expect("set bounded pin-racer timeout");
                barrier_a.wait();
                acquire_pin(&mut c, acquire_request("racing-pin", "1"))
            });
            let path_b = race_path;
            let barrier_b = Arc::clone(&barrier);
            let claim_thread = thread::spawn(move || {
                let mut c = Connection::open(path_b).expect("open claim-racer connection");
                c.pragma_update(None, "foreign_keys", true)
                    .expect("enable claim-racer FK");
                c.busy_timeout(Duration::from_secs(5))
                    .expect("set bounded claim-racer timeout");
                barrier_b.wait();
                claim_retirement(&mut c, claim_request("racing-claim", "1"))
            });
            barrier.wait();
            let pin = pin_thread.join().expect("pin racer thread completes");
            let claim = claim_thread.join().expect("claim racer thread completes");
            assert_ne!(
                pin.is_ok(),
                claim.is_ok(),
                "exactly one competing operation commits"
            );
            if pin.is_ok() {
                assert!(matches!(
                    claim,
                    Err(CustodyError::ActivePins) | Err(CustodyError::StaleRevision)
                ));
            } else {
                assert!(matches!(
                    pin,
                    Err(CustodyError::RetirementClaimed) | Err(CustodyError::StaleRevision)
                ));
            }
        }
    }

    #[test]
    fn two_connection_orderings_enforce_pin_claim_interlock() {
        let dir = tempfile::tempdir().expect("create ordering fixture directory");
        let make_pair = |name: &str| {
            let path = dir.path().join(name);
            let mut setup = Connection::open(&path).expect("open fixture setup connection");
            setup
                .busy_timeout(Duration::from_secs(5))
                .expect("set bounded setup timeout");
            initialize(&setup).expect("initialize fixture schema");
            register(&mut setup);
            drop(setup);
            let a = Connection::open(&path).expect("open first real SQLite connection");
            let b = Connection::open(&path).expect("open second real SQLite connection");
            a.pragma_update(None, "foreign_keys", true)
                .expect("enable first FK");
            b.pragma_update(None, "foreign_keys", true)
                .expect("enable second FK");
            a.busy_timeout(Duration::from_secs(5))
                .expect("set bounded first timeout");
            b.busy_timeout(Duration::from_secs(5))
                .expect("set bounded second timeout");
            (a, b)
        };
        let (mut pin_first, mut claim_second) = make_pair("pin-first.sqlite");
        acquire_pin(&mut pin_first, acquire_request("pin-first", "1")).expect("pin commits first");
        assert!(matches!(
            claim_retirement(&mut claim_second, claim_request("claim-second", "2")),
            Err(CustodyError::ActivePins)
        ));

        let (mut claim_first, mut pin_second) = make_pair("claim-first.sqlite");
        claim_retirement(&mut claim_first, claim_request("claim-first", "1"))
            .expect("claim commits first");
        assert!(matches!(
            acquire_pin(&mut pin_second, acquire_request("pin-second", "2")),
            Err(CustodyError::RetirementClaimed)
        ));
    }
}
