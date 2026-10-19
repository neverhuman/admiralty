//! Resumable inbound artifacts held under an explicit node/scope custody grant.
use super::{decode, digest, enrollment::Controller, invalid, json};
use admiralty_core::nodes::{DecimalU64, NodeIdentity, NodeScope};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::io;

const MAX_ARTIFACT: u64 = 8 * 1024 * 1024;
const MAX_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CHUNK: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub transfer_id: String,
    pub identity: NodeIdentity,
    pub scope: NodeScope,
    pub digest: String,
    pub size: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub manifest: Manifest,
    pub offset: DecimalU64,
    pub bytes: Vec<u8>,
    pub finish: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferAck {
    pub offset: DecimalU64,
    pub complete: bool,
}

impl Controller {
    /// A trusted local operator grants custody before any remote bytes are read.
    /// The transport cannot mint this grant from a client manifest.
    pub fn grant_transfer(&mut self, certificate: &[u8], manifest: &Manifest) -> io::Result<()> {
        let size = manifest.size.parse().map_err(invalid)?;
        if size > MAX_ARTIFACT
            || uuid::Uuid::parse_str(&manifest.transfer_id).is_err()
            || manifest.digest.len() != 71
            || !manifest.digest.starts_with("sha256:")
            || !manifest.digest[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("invalid artifact manifest"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        Self::authorize(&tx, certificate, &manifest.identity, &manifest.scope)?;
        let prior: Option<Vec<u8>> = tx
            .query_row(
                "SELECT manifest FROM transfers WHERE id=?1",
                [&manifest.transfer_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(invalid)?;
        if let Some(prior) = prior {
            if decode::<Manifest>(&prior)? != *manifest {
                return Err(invalid("transfer identity collision"));
            }
            return Ok(());
        }
        let reserved: u64 = {
            let mut statement = tx
                .prepare("SELECT manifest FROM transfers")
                .map_err(invalid)?;
            let rows = statement
                .query_map([], |r| r.get::<_, Vec<u8>>(0))
                .map_err(invalid)?;
            let mut sum = 0u64;
            for row in rows {
                sum = sum
                    .checked_add(
                        decode::<Manifest>(&row.map_err(invalid)?)?
                            .size
                            .parse()
                            .map_err(invalid)?,
                    )
                    .ok_or_else(|| invalid("transfer reservation overflow"))?;
            }
            sum
        };
        let count: u64 = tx
            .query_row("SELECT COUNT(*) FROM transfers", [], |r| {
                r.get::<_, u32>(0).map(u64::from)
            })
            .map_err(invalid)?;
        if count >= 128 || reserved + size > MAX_TRANSFER_BYTES {
            return Err(invalid("artifact custody spool full"));
        }
        tx.execute(
            "INSERT INTO transfers VALUES(?1,?2,?3,0)",
            params![manifest.transfer_id, json(manifest)?, Vec::<u8>::new()],
        )
        .map_err(invalid)?;
        tx.commit().map_err(invalid)
    }

    pub fn transfer(&mut self, certificate: &[u8], chunk: &Chunk) -> io::Result<TransferAck> {
        if chunk.bytes.len() > MAX_CHUNK {
            return Err(invalid("transfer chunk too large"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        Self::authorize(
            &tx,
            certificate,
            &chunk.manifest.identity,
            &chunk.manifest.scope,
        )?;
        let (manifest, mut data, complete): (Vec<u8>, Vec<u8>, bool) = tx
            .query_row(
                "SELECT manifest,data,complete FROM transfers WHERE id=?1",
                [&chunk.manifest.transfer_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(invalid)?;
        if decode::<Manifest>(&manifest)? != chunk.manifest {
            return Err(invalid("artifact custody grant mismatch"));
        }
        let offset = chunk.offset.parse().map_err(invalid)?;
        let end = offset
            .checked_add(chunk.bytes.len() as u64)
            .ok_or_else(|| invalid("offset overflow"))?;
        let size = chunk.manifest.size.parse().map_err(invalid)?;
        if offset > data.len() as u64 || end > size {
            return Err(invalid("artifact range gap or overflow"));
        }
        if offset < data.len() as u64 || complete {
            if end > data.len() as u64
                || data.get(offset as usize..end as usize) != Some(chunk.bytes.as_slice())
            {
                return Err(invalid("artifact range collision"));
            }
        } else {
            data.extend_from_slice(&chunk.bytes);
        }
        let finished = complete || chunk.finish;
        if finished && (data.len() as u64 != size || digest(&data) != chunk.manifest.digest) {
            return Err(invalid("artifact final digest/size mismatch"));
        }
        tx.execute(
            "UPDATE transfers SET data=?2,complete=?3 WHERE id=?1",
            params![chunk.manifest.transfer_id, data, finished],
        )
        .map_err(invalid)?;
        tx.commit().map_err(invalid)?;
        Ok(TransferAck {
            offset: DecimalU64::new(data.len() as u64),
            complete: finished,
        })
    }

    /// Only verified complete bytes are visible to the local artifact consumer.
    pub fn artifact(&self, manifest: &Manifest) -> io::Result<Vec<u8>> {
        let (stored, data, complete): (Vec<u8>, Vec<u8>, bool) = self
            .db
            .query_row(
                "SELECT manifest,data,complete FROM transfers WHERE id=?1",
                [&manifest.transfer_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(invalid)?;
        if !complete
            || decode::<Manifest>(&stored)? != *manifest
            || digest(&data) != manifest.digest
            || data.len() as u64 != manifest.size.parse().map_err(invalid)?
        {
            return Err(invalid("artifact not verified"));
        }
        Ok(data)
    }
}
