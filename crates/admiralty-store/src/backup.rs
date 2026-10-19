//! Online SQLite backup primitive and artifact-closure metadata interface (`P03` / `MASTER_PLAN` §6.5).
//!
//! Uses `Store::backup` (SQLite online backup API + `PRAGMA quick_check` + `fsync` +
//! atomic hard-link publication, never raw file copy of the live database) and binds
//! the snapshot watermark (`Cursor`), schema `user_version`, canonical `projection_digest`,
//! backup file SHA-256/size, pinned `artifact_closure`, and `active_bundle_refs`.
//! Full restore authority belongs to `P25` (`full_restore_authority_granted` is always `false`).

use crate::{Cursor, Result, Store, StoreError};
use admiralty_core::content_digest;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
};

fn validate_private_file(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(StoreError::Invalid(
                    "backup files must be regular files, never symlinks".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
                    return Err(StoreError::Invalid(
                        "backup files require private 0600 permissions and one link".into(),
                    ));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub const BACKUP_CLOSURE_SCHEMA_VERSION: u32 = 1;
pub const MAX_BACKUP_CLOSURE_ARTIFACTS: usize = 1024;
pub const MAX_BACKUP_ACTIVE_BUNDLES: usize = 256;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactClosureEntry {
    pub artifact_id: String,
    pub content_digest: String,
    pub size_bytes: u64,
    pub pin_id: String,
    pub verified_available: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActiveBundleRef {
    pub target_id: String,
    pub bundle_id: String,
    pub version: String,
    pub artifact_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupClosureManifest {
    pub schema_version: u32,
    pub snapshot_cursor: Cursor,
    pub schema_user_version: i64,
    pub projection_digest: String,
    pub backup_file_sha256: String,
    pub backup_file_size_bytes: u64,
    pub artifact_closure: Vec<ArtifactClosureEntry>,
    pub active_bundle_refs: Vec<ActiveBundleRef>,
    /// Always `false` in `P03`: full restore authority requires `P25` executor fencing.
    pub full_restore_authority_granted: bool,
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn valid_identifier(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_len && !value.chars().any(char::is_control)
}

pub fn validate_closure_inputs(
    artifacts: &[ArtifactClosureEntry],
    bundles: &[ActiveBundleRef],
) -> Result<()> {
    if artifacts.len() > MAX_BACKUP_CLOSURE_ARTIFACTS || bundles.len() > MAX_BACKUP_ACTIVE_BUNDLES {
        return Err(StoreError::Invalid(
            "backup closure exceeds maximum artifact or bundle count".into(),
        ));
    }
    let mut seen_artifacts = BTreeSet::new();
    for entry in artifacts {
        if !valid_identifier(&entry.artifact_id, 256)
            || !valid_identifier(&entry.pin_id, 256)
            || !valid_sha256(&entry.content_digest)
            || !entry.verified_available
            || !seen_artifacts.insert(entry.artifact_id.as_str())
        {
            return Err(StoreError::Invalid(
                "backup artifact closure requires unique, pinned, verified_available artifacts with valid sha256 digests".into(),
            ));
        }
    }
    let mut seen_targets = BTreeSet::new();
    for bundle in bundles {
        if !valid_identifier(&bundle.target_id, 256)
            || !valid_identifier(&bundle.bundle_id, 256)
            || !valid_identifier(&bundle.version, 128)
            || !valid_sha256(&bundle.artifact_digest)
            || !seen_targets.insert(bundle.target_id.as_str())
        {
            return Err(StoreError::Invalid(
                "backup active_bundle_refs contains invalid or duplicate target bundle reference"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn hash_file_sha256(path: &Path) -> Result<(String, u64)> {
    validate_private_file(path)?;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }
    Ok((format!("sha256:{:x}", hasher.finalize()), total))
}

/// Create an online SQLite backup via `Store::backup` and atomically persist its
/// verified `BackupClosureManifest` at `manifest_destination`.
pub fn create_online_backup_with_closure(
    store: &Store,
    backup_destination: &Path,
    manifest_destination: &Path,
    artifact_closure: Vec<ArtifactClosureEntry>,
    active_bundle_refs: Vec<ActiveBundleRef>,
) -> Result<BackupClosureManifest> {
    validate_closure_inputs(&artifact_closure, &active_bundle_refs)?;
    if backup_destination == manifest_destination {
        return Err(StoreError::Invalid(
            "backup_destination and manifest_destination must be distinct".into(),
        ));
    }
    if manifest_destination.symlink_metadata().is_ok() {
        return Err(StoreError::Invalid(
            "backup manifest destination already exists".into(),
        ));
    }

    let snapshot = store.backup(backup_destination)?;
    let projection_digest = content_digest(&snapshot.projection)
        .map_err(|e| StoreError::Invalid(format!("cannot digest backup projection: {e}")))?;
    let (backup_file_sha256, backup_file_size_bytes) = hash_file_sha256(backup_destination)?;

    let readonly = Connection::open_with_flags(
        backup_destination,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let schema_user_version: i64 =
        readonly.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_user_version <= 0 {
        return Err(StoreError::Invalid(
            "backup schema user_version must be positive".into(),
        ));
    }

    let manifest = BackupClosureManifest {
        schema_version: BACKUP_CLOSURE_SCHEMA_VERSION,
        snapshot_cursor: snapshot.cursor,
        schema_user_version,
        projection_digest,
        backup_file_sha256,
        backup_file_size_bytes,
        artifact_closure,
        active_bundle_refs,
        full_restore_authority_granted: false,
    };

    let encoded = serde_json::to_vec_pretty(&manifest)?;
    if encoded.len() > MAX_MANIFEST_BYTES {
        return Err(StoreError::Invalid(
            "backup closure manifest exceeds 1 MiB".into(),
        ));
    }

    let parent = manifest_destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stage = parent.join(format!(
        ".admiralty-backup-closure-{}.json",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_res = (|| -> Result<()> {
        use std::io::Write;
        let mut file = options.open(&stage)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&stage, manifest_destination)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&stage);
    write_res?;

    Ok(manifest)
}

/// Independently verify a backup file and its `BackupClosureManifest`, confirming
/// private file permissions, exact SHA-256/size match, SQLite replay projection
/// digest equality, pinned artifact closure validity, and `full_restore_authority_granted == false`.
pub fn verify_backup_closure(
    backup_path: &Path,
    manifest_path: &Path,
) -> Result<BackupClosureManifest> {
    validate_private_file(backup_path)?;
    validate_private_file(manifest_path)?;
    let bytes = fs::read(manifest_path)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(StoreError::Invalid(
            "backup closure manifest exceeds 1 MiB".into(),
        ));
    }
    let manifest: BackupClosureManifest = serde_json::from_slice(&bytes)?;
    if manifest.schema_version != BACKUP_CLOSURE_SCHEMA_VERSION
        || manifest.full_restore_authority_granted
        || manifest.schema_user_version <= 0
    {
        return Err(StoreError::Invalid(
            "invalid backup closure manifest version or unauthorized restore grant".into(),
        ));
    }
    validate_closure_inputs(&manifest.artifact_closure, &manifest.active_bundle_refs)?;

    let (actual_sha256, actual_size) = hash_file_sha256(backup_path)?;
    if actual_sha256 != manifest.backup_file_sha256
        || actual_size != manifest.backup_file_size_bytes
    {
        return Err(StoreError::Invalid(
            "backup file bytes or size do not match closure manifest".into(),
        ));
    }

    let reopened = Store::open(backup_path)?;
    let snapshot = reopened.snapshot()?;
    let actual_projection_digest = content_digest(&snapshot.projection)
        .map_err(|e| StoreError::Invalid(format!("cannot digest reopened backup: {e}")))?;
    if snapshot.cursor != manifest.snapshot_cursor
        || actual_projection_digest != manifest.projection_digest
    {
        return Err(StoreError::Invalid(
            "reopened backup cursor or projection digest differs from closure manifest".into(),
        ));
    }
    Ok(manifest)
}
