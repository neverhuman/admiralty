//! Durable asset custody, pin, and retirement-claim records.
//!
//! All mutations run on a caller-owned SQLite connection in one IMMEDIATE
//! transaction. This module records observations and claims; it never scans,
//! deletes, or otherwise touches an asset on disk.
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ID_BYTES: usize = 128;

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
    #[error("asset observation is unknown, stale, foreign, dirty, or held")]
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
}

pub type CustodyResult<T> = std::result::Result<T, CustodyError>;

/// Full immutable scope, SHA-256 identity, and custody generation.
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

/// Facts must be freshly observed by the caller. `None` means unknown and fails closed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CustodyObservation {
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
    /// Registration is insert-only: zero means the caller expects no prior row.
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CustodyReceipt {
    pub command_id: String,
    pub operation: String,
    pub request_digest: String,
    pub asset_id: String,
    pub generation: String,
    pub revision: String,
    pub reference_id: Option<String>,
    pub observed_at: String,
    pub decision_at: String,
}

#[derive(Clone)]
struct Validated {
    observed_at: i64,
    decision_at: i64,
    provenance: String,
}

/// Create custody tables transactionally. Safe to call repeatedly.
pub fn initialize(connection: &Connection) -> CustodyResult<()> {
    let tx = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS custody_assets (
            asset_id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            root_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            sha256 TEXT NOT NULL CHECK(length(sha256)=71 AND substr(sha256,1,7)='sha256:'),
            generation INTEGER NOT NULL CHECK(generation>0),
            revision INTEGER NOT NULL CHECK(revision>0),
            observed_at INTEGER NOT NULL CHECK(observed_at>=0),
            decision_at INTEGER NOT NULL CHECK(decision_at>=0),
            observation_provenance TEXT NOT NULL,
            retirement_claim_id TEXT,
            UNIQUE(asset_id,generation)
        );
        CREATE TABLE IF NOT EXISTS custody_pins (
            asset_id TEXT NOT NULL REFERENCES custody_assets(asset_id),
            pin_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            root_id TEXT NOT NULL,
            task_id TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK(generation>0),
            active INTEGER NOT NULL CHECK(active IN (0,1)),
            PRIMARY KEY(asset_id,pin_id)
        );
        CREATE INDEX IF NOT EXISTS custody_pins_active ON custody_pins(asset_id,active);
        CREATE TABLE IF NOT EXISTS custody_commands (
            command_id TEXT PRIMARY KEY,
            operation TEXT NOT NULL,
            request_digest TEXT NOT NULL,
            receipt_json TEXT NOT NULL
        );",
    )?;
    tx.commit()?;
    Ok(())
}

pub fn register_asset(
    connection: &mut Connection,
    request: RegisterAssetRequest,
) -> CustodyResult<CustodyReceipt> {
    bounded_request(&request)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(receipt) =
        replay::<RegisterAssetRequest>(&tx, &request.command_id, "register_asset", &request)?
    {
        return Ok(receipt);
    }
    let evidence = validate_observation(&request.observation)?;
    if request.expected_revision != "0" || request.observation.registered != Some(false) {
        return Err(CustodyError::StaleRevision);
    }
    validate_binding(&request.binding)?;
    let generation = positive_counter(&request.binding.generation)?;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO custody_assets(asset_id,project_id,root_id,task_id,owner_id,sha256,generation,revision,observed_at,decision_at,observation_provenance) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9,?10)",
        params![request.binding.asset_id, request.binding.project_id.as_str(), request.binding.root_id.as_str(), request.binding.task_id.as_str(), request.binding.owner_id.as_str(), request.binding.sha256, generation, evidence.observed_at, evidence.decision_at, evidence.provenance],
    )?;
    if changed != 1 {
        return Err(CustodyError::ForeignAsset);
    }
    let receipt = make_receipt(
        &request.command_id,
        "register_asset",
        &request,
        &request.binding,
        1,
        None,
        &evidence,
    )?;
    record_command(&tx, &receipt)?;
    tx.commit()?;
    Ok(receipt)
}

pub fn acquire_pin(
    connection: &mut Connection,
    request: AcquirePinRequest,
) -> CustodyResult<CustodyReceipt> {
    bounded_request(&request)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(receipt) =
        replay::<AcquirePinRequest>(&tx, &request.command_id, "acquire_pin", &request)?
    {
        return Ok(receipt);
    }
    let evidence = validate_observation(&request.observation)?;
    if request.observation.registered != Some(true) {
        return Err(CustodyError::UnsafeObservation);
    }
    validate_binding(&request.binding)?;
    bounded_id(&request.pin_id)?;
    let revision = validate_asset(&tx, &request.binding, &request.expected_revision)?;
    if active_claim(&tx, &request.binding.asset_id)?.is_some() {
        return Err(CustodyError::RetirementClaimed);
    }
    let changed = tx.execute("INSERT OR IGNORE INTO custody_pins(asset_id,pin_id,project_id,root_id,task_id,owner_id,generation,active) VALUES(?1,?2,?3,?4,?5,?6,?7,1)", params![request.binding.asset_id, request.pin_id, request.binding.project_id.as_str(), request.binding.root_id.as_str(), request.binding.task_id.as_str(), request.binding.owner_id.as_str(), positive_counter(&request.binding.generation)?])?;
    if changed != 1 {
        return Err(CustodyError::InvalidPin);
    }
    let next = next_revision(revision)?;
    update_observation(&tx, &request.binding.asset_id, revision, next, &evidence)?;
    let receipt = make_receipt(
        &request.command_id,
        "acquire_pin",
        &request,
        &request.binding,
        next,
        Some(request.pin_id.clone()),
        &evidence,
    )?;
    record_command(&tx, &receipt)?;
    tx.commit()?;
    Ok(receipt)
}

pub fn release_pin(
    connection: &mut Connection,
    request: ReleasePinRequest,
) -> CustodyResult<CustodyReceipt> {
    bounded_request(&request)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(receipt) =
        replay::<ReleasePinRequest>(&tx, &request.command_id, "release_pin", &request)?
    {
        return Ok(receipt);
    }
    let evidence = validate_observation(&request.observation)?;
    if request.observation.registered != Some(true) {
        return Err(CustodyError::UnsafeObservation);
    }
    validate_binding(&request.binding)?;
    bounded_id(&request.pin_id)?;
    let revision = validate_asset(&tx, &request.binding, &request.expected_revision)?;
    let changed = tx.execute("UPDATE custody_pins SET active=0 WHERE asset_id=?1 AND pin_id=?2 AND project_id=?3 AND root_id=?4 AND task_id=?5 AND owner_id=?6 AND generation=?7 AND active=1", params![request.binding.asset_id, request.pin_id, request.binding.project_id.as_str(), request.binding.root_id.as_str(), request.binding.task_id.as_str(), request.binding.owner_id.as_str(), positive_counter(&request.binding.generation)?])?;
    if changed != 1 {
        return Err(CustodyError::InvalidPin);
    }
    let next = next_revision(revision)?;
    update_observation(&tx, &request.binding.asset_id, revision, next, &evidence)?;
    let receipt = make_receipt(
        &request.command_id,
        "release_pin",
        &request,
        &request.binding,
        next,
        Some(request.pin_id.clone()),
        &evidence,
    )?;
    record_command(&tx, &receipt)?;
    tx.commit()?;
    Ok(receipt)
}

pub fn claim_retirement(
    connection: &mut Connection,
    request: ClaimRetirementRequest,
) -> CustodyResult<CustodyReceipt> {
    bounded_request(&request)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(receipt) =
        replay::<ClaimRetirementRequest>(&tx, &request.command_id, "claim_retirement", &request)?
    {
        return Ok(receipt);
    }
    let evidence = validate_observation(&request.observation)?;
    if request.observation.registered != Some(true) {
        return Err(CustodyError::UnsafeObservation);
    }
    validate_binding(&request.binding)?;
    bounded_id(&request.claim_id)?;
    let revision = validate_asset(&tx, &request.binding, &request.expected_revision)?;
    let pins: i64 = tx.query_row(
        "SELECT count(*) FROM custody_pins WHERE asset_id=?1 AND active=1",
        [&request.binding.asset_id],
        |row| row.get(0),
    )?;
    if pins != 0 {
        return Err(CustodyError::ActivePins);
    }
    let next = next_revision(revision)?;
    let changed = tx.execute("UPDATE custody_assets SET retirement_claim_id=?1,revision=?2,observed_at=?3,decision_at=?4,observation_provenance=?5 WHERE asset_id=?6 AND revision=?7 AND retirement_claim_id IS NULL", params![request.claim_id, next, evidence.observed_at, evidence.decision_at, evidence.provenance, request.binding.asset_id, revision])?;
    if changed != 1 {
        return Err(CustodyError::StaleRevision);
    }
    let receipt = make_receipt(
        &request.command_id,
        "claim_retirement",
        &request,
        &request.binding,
        next,
        Some(request.claim_id.clone()),
        &evidence,
    )?;
    record_command(&tx, &receipt)?;
    tx.commit()?;
    Ok(receipt)
}

trait BoundedRequest: Serialize {
    fn validate_bounds(&self) -> CustodyResult<()>;
}
fn bounded_request<T: BoundedRequest>(request: &T) -> CustodyResult<()> {
    request.validate_bounds()?;
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(CustodyError::Oversize);
    }
    Ok(())
}
fn check_binding_bounds(b: &AssetBinding) -> CustodyResult<()> {
    bounded_id(&b.asset_id)?;
    if b.sha256.len() > MAX_TEXT_BYTES || b.generation.len() > 20 {
        return Err(CustodyError::Oversize);
    }
    Ok(())
}
fn check_observation_bounds(o: &CustodyObservation) -> CustodyResult<()> {
    if o.provenance.len() > MAX_TEXT_BYTES
        || o.observed_at.len() > 20
        || o.decision_at.len() > 20
        || o.max_age.len() > 20
    {
        return Err(CustodyError::Oversize);
    }
    Ok(())
}
impl BoundedRequest for RegisterAssetRequest {
    fn validate_bounds(&self) -> CustodyResult<()> {
        bounded_id(&self.command_id)?;
        check_binding_bounds(&self.binding)?;
        check_observation_bounds(&self.observation)?;
        if self.expected_revision.len() > 20 {
            return Err(CustodyError::Oversize);
        }
        Ok(())
    }
}
impl BoundedRequest for AcquirePinRequest {
    fn validate_bounds(&self) -> CustodyResult<()> {
        bounded_id(&self.command_id)?;
        bounded_id(&self.pin_id)?;
        check_binding_bounds(&self.binding)?;
        check_observation_bounds(&self.observation)?;
        if self.expected_revision.len() > 20 {
            return Err(CustodyError::Oversize);
        }
        Ok(())
    }
}
impl BoundedRequest for ReleasePinRequest {
    fn validate_bounds(&self) -> CustodyResult<()> {
        bounded_id(&self.command_id)?;
        bounded_id(&self.pin_id)?;
        check_binding_bounds(&self.binding)?;
        check_observation_bounds(&self.observation)?;
        if self.expected_revision.len() > 20 {
            return Err(CustodyError::Oversize);
        }
        Ok(())
    }
}
impl BoundedRequest for ClaimRetirementRequest {
    fn validate_bounds(&self) -> CustodyResult<()> {
        bounded_id(&self.command_id)?;
        bounded_id(&self.claim_id)?;
        check_binding_bounds(&self.binding)?;
        check_observation_bounds(&self.observation)?;
        if self.expected_revision.len() > 20 {
            return Err(CustodyError::Oversize);
        }
        Ok(())
    }
}
fn bounded_id(value: &str) -> CustodyResult<()> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(char::is_control) {
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
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(CustodyError::Invalid("full lowercase SHA-256 required"));
    }
    positive_counter(&b.generation)?;
    Ok(())
}
fn positive_counter(value: &str) -> CustodyResult<i64> {
    let number = value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value && *n > 0)
        .ok_or(CustodyError::Invalid("canonical positive decimal counter"))?;
    i64::try_from(number).map_err(|_| CustodyError::Overflow)
}
fn counter(value: &str) -> CustodyResult<i64> {
    let number = value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or(CustodyError::Invalid("canonical decimal counter"))?;
    i64::try_from(number).map_err(|_| CustodyError::Overflow)
}
fn validate_observation(o: &CustodyObservation) -> CustodyResult<Validated> {
    if o.provenance.is_empty()
        || o.provenance.len() > MAX_TEXT_BYTES
        || o.provenance.chars().any(char::is_control)
    {
        return Err(CustodyError::UnsafeObservation);
    }
    if o.registered.is_none()
        || o.foreign.is_none()
        || o.dirty.is_none()
        || o.held.is_none()
        || o.foreign != Some(false)
        || o.dirty != Some(false)
        || o.held != Some(false)
    {
        return Err(CustodyError::UnsafeObservation);
    }
    let observed = counter(&o.observed_at)?;
    let decision = counter(&o.decision_at)?;
    let max_age = counter(&o.max_age)?;
    if observed > decision
        || decision
            .checked_sub(observed)
            .ok_or(CustodyError::Overflow)?
            > max_age
    {
        return Err(CustodyError::UnsafeObservation);
    }
    Ok(Validated {
        observed_at: observed,
        decision_at: decision,
        provenance: o.provenance.clone(),
    })
}
fn validate_asset(
    tx: &Transaction<'_>,
    b: &AssetBinding,
    expected_revision: &str,
) -> CustodyResult<i64> {
    let expected = positive_counter(expected_revision)?;
    let row: Option<(String,String,String,String,String,i64,i64)> = tx.query_row("SELECT project_id,root_id,task_id,owner_id,sha256,generation,revision FROM custody_assets WHERE asset_id=?1", [&b.asset_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    let Some((project, root, task, owner, digest, generation, revision)) = row else {
        return Err(CustodyError::MissingAsset);
    };
    if project != b.project_id.as_str()
        || root != b.root_id.as_str()
        || task != b.task_id.as_str()
        || owner != b.owner_id.as_str()
        || digest != b.sha256
    {
        return Err(CustodyError::ForeignAsset);
    }
    if generation != positive_counter(&b.generation)? {
        return Err(CustodyError::StaleGeneration);
    }
    if revision != expected {
        return Err(CustodyError::StaleRevision);
    }
    Ok(revision)
}
fn active_claim(tx: &Transaction<'_>, asset: &str) -> CustodyResult<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT retirement_claim_id FROM custody_assets WHERE asset_id=?1",
            [asset],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}
fn next_revision(revision: i64) -> CustodyResult<i64> {
    revision.checked_add(1).ok_or(CustodyError::Overflow)
}
fn update_observation(
    tx: &Transaction<'_>,
    asset: &str,
    revision: i64,
    next: i64,
    e: &Validated,
) -> CustodyResult<()> {
    let n = tx.execute("UPDATE custody_assets SET revision=?1,observed_at=?2,decision_at=?3,observation_provenance=?4 WHERE asset_id=?5 AND revision=?6", params![next,e.observed_at,e.decision_at,e.provenance,asset,revision])?;
    if n != 1 {
        return Err(CustodyError::StaleRevision);
    }
    Ok(())
}
fn make_receipt<T: Serialize>(
    command: &str,
    operation: &str,
    request: &T,
    binding: &AssetBinding,
    revision: i64,
    reference_id: Option<String>,
    evidence: &Validated,
) -> CustodyResult<CustodyReceipt> {
    let canonical = serde_json::to_vec(request)?;
    let request_digest = format!("sha256:{:x}", Sha256::digest(canonical));
    Ok(CustodyReceipt {
        command_id: command.to_owned(),
        operation: operation.to_owned(),
        request_digest,
        asset_id: binding.asset_id.clone(),
        generation: binding.generation.clone(),
        revision: revision.to_string(),
        reference_id,
        observed_at: evidence.observed_at.to_string(),
        decision_at: evidence.decision_at.to_string(),
    })
}
fn record_command(tx: &Transaction<'_>, receipt: &CustodyReceipt) -> CustodyResult<()> {
    tx.execute("INSERT INTO custody_commands(command_id,operation,request_digest,receipt_json) VALUES(?1,?2,?3,?4)", params![receipt.command_id,receipt.operation,receipt.request_digest,serde_json::to_string(receipt)?])?;
    Ok(())
}
fn replay<T: Serialize>(
    tx: &Transaction<'_>,
    command_id: &str,
    operation: &str,
    request: &T,
) -> CustodyResult<Option<CustodyReceipt>> {
    bounded_id(command_id)?;
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(CustodyError::Oversize);
    }
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let row: Option<(String,String,String)> = tx.query_row("SELECT operation,request_digest,receipt_json FROM custody_commands WHERE command_id=?1", [command_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let Some((prior_operation, prior_digest, receipt)) = row else {
        return Ok(None);
    };
    if prior_operation != operation || prior_digest != digest {
        return Err(CustodyError::CommandCollision);
    }
    Ok(Some(serde_json::from_str(&receipt)?))
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
            observation: observation(true, 11),
        }
    }
    fn claim_request(cmd: &str, rev: &str) -> ClaimRetirementRequest {
        ClaimRetirementRequest {
            command_id: cmd.into(),
            binding: binding(),
            claim_id: format!("claim-{cmd}"),
            expected_revision: rev.into(),
            observation: observation(true, 11),
        }
    }
    fn release_request(cmd: &str, rev: &str, pin: &str) -> ReleasePinRequest {
        ReleasePinRequest {
            command_id: cmd.into(),
            binding: binding(),
            pin_id: pin.into(),
            expected_revision: rev.into(),
            observation: observation(true, 12),
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
        let changed = RegisterAssetRequest {
            command_id: "register-1".into(),
            binding: AssetBinding {
                sha256: format!("sha256:{}", "b".repeat(64)),
                ..binding()
            },
            expected_revision: "0".into(),
            observation: observation(false, 10),
        };
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
                c.busy_timeout(Duration::from_secs(5))
                    .expect("set bounded pin-racer timeout");
                barrier_a.wait();
                acquire_pin(&mut c, acquire_request("racing-pin", "1"))
            });
            let path_b = race_path;
            let barrier_b = Arc::clone(&barrier);
            let claim_thread = thread::spawn(move || {
                let mut c = Connection::open(path_b).expect("open claim-racer connection");
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
