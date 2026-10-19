//! Scoped local incidents on Store's sole writer. No execution/publication effects.
//! Commands and receipt digests are replayed before reads, retries and writes.
use admiralty_core::{
    content_digest,
    excursions::{
        ExcursionScope, ExcursionState, LocalProposal, ReproductionContract, ScopedIncident,
        build_local_proposal, reduce_excursions,
    },
    limits::bounded_json_size,
};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};

pub const MAX_COMMAND_BYTES: usize = 1024 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_JOURNAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COMMANDS: usize = 1024;
pub const MAX_PROPOSALS: usize = 32;
const MAX_SCOPES: i64 = 128;
const MAX_TOTAL_BYTES: i64 = 32 * 1024 * 1024;
pub type ExcursionProof = Arc<dyn Any + Send + Sync>;
type Result<T> = std::result::Result<T, ExcursionStoreError>;

#[derive(Debug, thiserror::Error)]
pub enum ExcursionStoreError {
    #[error("excursion SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("excursion JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Reduction(#[from] admiralty_core::excursions::ExcursionError),
    #[error("excursion access denied")]
    Denied,
    #[error("invalid excursion request")]
    Invalid,
    #[error("excursion command identity collision")]
    Collision,
    #[error("excursion revision is stale")]
    Stale,
    #[error("excursion capacity exhausted; history was retained")]
    Capacity,
    #[error("excursion journal integrity failure")]
    Integrity,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExcursionAction {
    Read,
    Observe,
    Propose,
}

/// Independently installed, bounded, nonblocking policy. It sees the complete
/// command, including evidence references; request labels never grant authority.
pub trait ExcursionAuthority: Send + Sync {
    fn authorize(
        &self,
        scope: &ExcursionScope,
        action: ExcursionAction,
        command: Option<&ExcursionCommand>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExcursionOperation {
    Observe {
        incidents: Vec<ScopedIncident>,
    },
    Propose {
        episode_id: String,
        reproduction: ReproductionContract,
    },
}
impl ExcursionOperation {
    pub fn action(&self) -> ExcursionAction {
        match self {
            Self::Observe { .. } => ExcursionAction::Observe,
            Self::Propose { .. } => ExcursionAction::Propose,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExcursionCommand {
    pub scope: ExcursionScope,
    pub command_id: String,
    pub expected_revision: u32,
    pub operation: ExcursionOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoredProposal {
    pub command_id: String,
    pub proposal: LocalProposal,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExcursionSnapshot {
    pub scope: ExcursionScope,
    pub revision: u32,
    pub state: Option<ExcursionState>,
    pub proposals: Vec<StoredProposal>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExcursionReceipt {
    pub command_id: String,
    pub revision: u32,
    pub snapshot_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExcursionResult {
    pub receipt: ExcursionReceipt,
    pub replayed: bool,
}

pub(crate) fn initialize(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS excursion_commands (
        scope_key TEXT NOT NULL, sequence INTEGER NOT NULL CHECK(sequence>0),
        command_id TEXT NOT NULL, command_json TEXT NOT NULL, receipt_json TEXT NOT NULL,
        PRIMARY KEY(scope_key, sequence), UNIQUE(scope_key, command_id)) STRICT;",
    )?;
    Ok(())
}
pub fn validate_command(c: &ExcursionCommand) -> Result<()> {
    bounded_json_size(c, MAX_COMMAND_BYTES).map_err(|_| ExcursionStoreError::Invalid)?;
    if c.command_id.is_empty()
        || c.command_id.len() > 128
        || !c
            .command_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(ExcursionStoreError::Invalid);
    }
    validate_scope(&c.scope)?;
    Ok(())
}
pub(crate) fn validate_scope(scope: &ExcursionScope) -> Result<()> {
    reduce_excursions(&None, scope, &[])?;
    Ok(())
}
fn empty(scope: &ExcursionScope) -> ExcursionSnapshot {
    ExcursionSnapshot {
        scope: scope.clone(),
        revision: 0,
        state: None,
        proposals: vec![],
    }
}
fn apply(snapshot: &mut ExcursionSnapshot, c: &ExcursionCommand) -> Result<ExcursionReceipt> {
    validate_command(c)?;
    if c.scope != snapshot.scope || c.expected_revision != snapshot.revision {
        return Err(ExcursionStoreError::Stale);
    }
    match &c.operation {
        ExcursionOperation::Observe { incidents } => {
            if incidents.is_empty() {
                return Err(ExcursionStoreError::Invalid);
            }
            snapshot.state = Some(reduce_excursions(&snapshot.state, &c.scope, incidents)?);
        }
        ExcursionOperation::Propose {
            episode_id,
            reproduction,
        } => {
            if snapshot.proposals.len() >= MAX_PROPOSALS {
                return Err(ExcursionStoreError::Capacity);
            }
            let episode = snapshot
                .state
                .as_ref()
                .and_then(|s| s.episodes.iter().find(|e| &e.episode_id == episode_id))
                .ok_or(ExcursionStoreError::Invalid)?;
            let proposal = build_local_proposal(episode, reproduction, &[])?;
            snapshot.proposals.push(StoredProposal {
                command_id: c.command_id.clone(),
                proposal,
            });
        }
    }
    snapshot.revision = snapshot
        .revision
        .checked_add(1)
        .ok_or(ExcursionStoreError::Capacity)?;
    bounded_json_size(snapshot, MAX_SNAPSHOT_BYTES).map_err(|_| ExcursionStoreError::Capacity)?;
    Ok(ExcursionReceipt {
        command_id: c.command_id.clone(),
        revision: snapshot.revision,
        snapshot_digest: content_digest(snapshot)?,
    })
}
fn stored_text(row: &rusqlite::Row<'_>, i: usize, max: usize) -> rusqlite::Result<String> {
    let bytes = row.get_ref(i)?.as_str()?;
    if bytes.len() > max {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(bytes.to_owned())
}
struct History {
    snapshot: ExcursionSnapshot,
    rows: Vec<(ExcursionCommand, ExcursionReceipt)>,
    bytes: usize,
}
fn history(c: &Connection, scope: &ExcursionScope) -> Result<History> {
    let key = content_digest(scope)?;
    let mut h = History {
        snapshot: empty(scope),
        rows: vec![],
        bytes: 0,
    };
    let mut stmt = c.prepare("SELECT sequence,command_json,receipt_json,command_id FROM excursion_commands WHERE scope_key=?1 ORDER BY sequence LIMIT 1025")?;
    let mut rows = stmt.query([&key])?;
    while let Some(row) = rows.next()? {
        if h.rows.len() == MAX_COMMANDS {
            return Err(ExcursionStoreError::Integrity);
        }
        let sequence: u32 = row.get(0)?;
        let cj = stored_text(row, 1, MAX_COMMAND_BYTES)?;
        let rj = stored_text(row, 2, 1024)?;
        h.bytes += cj.len() + rj.len();
        if h.bytes > MAX_JOURNAL_BYTES {
            return Err(ExcursionStoreError::Integrity);
        }
        let command: ExcursionCommand = serde_json::from_str(&cj)?;
        let receipt: ExcursionReceipt = serde_json::from_str(&rj)?;
        let command_id = stored_text(row, 3, 128)?;
        if command_id != command.command_id
            || sequence != h.snapshot.revision + 1
            || h.rows
                .iter()
                .any(|(c, _)| c.command_id == command.command_id)
            || apply(&mut h.snapshot, &command)? != receipt
        {
            return Err(ExcursionStoreError::Integrity);
        }
        h.rows.push((command, receipt));
    }
    Ok(h)
}
fn authorize(
    authority: Option<&dyn ExcursionAuthority>,
    scope: &ExcursionScope,
    action: ExcursionAction,
    command: Option<&ExcursionCommand>,
    proof: &(dyn Any + Send + Sync),
) -> Result<()> {
    if !authority.is_some_and(|a| a.authorize(scope, action, command, proof)) {
        return Err(ExcursionStoreError::Denied);
    }
    Ok(())
}
pub(crate) fn read(
    c: &mut Connection,
    scope: &ExcursionScope,
    authority: Option<&dyn ExcursionAuthority>,
    proof: &(dyn Any + Send + Sync),
) -> Result<ExcursionSnapshot> {
    validate_scope(scope)?;
    authorize(authority, scope, ExcursionAction::Read, None, proof)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let snapshot = history(&tx, scope)?.snapshot;
    authorize(authority, scope, ExcursionAction::Read, None, proof)?;
    tx.commit()?;
    Ok(snapshot)
}
pub(crate) fn record(
    c: &mut Connection,
    command: &ExcursionCommand,
    authority: Option<&dyn ExcursionAuthority>,
    proof: &(dyn Any + Send + Sync),
) -> Result<ExcursionResult> {
    validate_command(command)?;
    let check = || {
        authorize(
            authority,
            &command.scope,
            command.operation.action(),
            Some(command),
            proof,
        )
    };
    check()?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut h = history(&tx, &command.scope)?;
    if let Some((prior, receipt)) = h
        .rows
        .iter()
        .find(|(c, _)| c.command_id == command.command_id)
    {
        if prior != command {
            return Err(ExcursionStoreError::Collision);
        }
        check()?;
        return Ok(ExcursionResult {
            receipt: receipt.clone(),
            replayed: true,
        });
    }
    if h.rows.len() == MAX_COMMANDS {
        return Err(ExcursionStoreError::Capacity);
    }
    let receipt = apply(&mut h.snapshot, command)?;
    let cj = serde_json::to_string(command)?;
    let rj = serde_json::to_string(&receipt)?;
    let extra = cj.len() + rj.len();
    let (scopes, total): (i64, i64) = tx.query_row("SELECT count(DISTINCT scope_key),coalesce(sum(length(CAST(command_json AS BLOB))+length(CAST(receipt_json AS BLOB))),0) FROM excursion_commands", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if h.bytes + extra > MAX_JOURNAL_BYTES
        || total > MAX_TOTAL_BYTES - extra as i64
        || (h.rows.is_empty() && scopes >= MAX_SCOPES)
    {
        return Err(ExcursionStoreError::Capacity);
    }
    tx.execute(
        "INSERT INTO excursion_commands VALUES(?1,?2,?3,?4,?5)",
        params![
            content_digest(&command.scope)?,
            receipt.revision,
            command.command_id,
            cj,
            rj
        ],
    )?;
    check()?;
    tx.commit()?;
    Ok(ExcursionResult {
        receipt,
        replayed: false,
    })
}

#[cfg(test)]
mod tests;
