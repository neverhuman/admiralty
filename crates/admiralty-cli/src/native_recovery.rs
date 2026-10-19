//! Create-only native Store backup and isolated recovery rehearsal primitives.
//!
//! This module performs local filesystem and SQLite work only. A successful
//! rehearsal proves database replay and explicitly supplied artifact bytes;
//! it does not prove recovery of external sessions, credentials, or effects.

use admiralty_core::{Projection, content_digest};
use admiralty_runtime::recovery::{
    BackupManifest, Blocker, ExternalFence, RecoveryObservation, RecoveryProposal, plan_recovery,
};
use admiralty_store::{Cursor, Store, artifacts::ArtifactStore};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const MAX_DATABASE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CLOSURE_OBJECTS: usize = 1024;
const MAX_CLOSURE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_COMMAND_JSON_BYTES: u64 = 1024 * 1024;

pub type RecoveryResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Args)]
pub struct RecoveryArgs {
    #[command(subcommand)]
    command: RecoveryCommand,
}

#[derive(Subcommand)]
enum RecoveryCommand {
    /// Back up an exclusively owned database and explicitly enumerated artifacts.
    Backup {
        #[arg(long, value_parser = command_path)]
        database: PathBuf,
        /// New database filename in an existing private directory.
        #[arg(long, value_parser = command_path)]
        output: PathBuf,
        /// New owner-only JSON receipt; an existing path is never overwritten.
        #[arg(long, value_parser = command_path)]
        receipt_output: PathBuf,
        /// Existing private content-addressed object directory.
        #[arg(long, value_parser = command_path)]
        objects: PathBuf,
        /// Owner-only JSON array of {sha256,size_bytes}; use [] explicitly for no additional objects.
        #[arg(long, value_parser = command_path)]
        artifacts: PathBuf,
    },
    /// Verify backup bytes, replay and the receipt's explicit artifact closure.
    Verify {
        #[arg(long, value_parser = command_path)]
        backup: PathBuf,
        #[arg(long, value_parser = command_path)]
        receipt: PathBuf,
        #[arg(long, value_parser = command_path)]
        objects: PathBuf,
    },
    /// Restore only into an existing private empty scratch directory; never activate it.
    Rehearse {
        #[arg(long, value_parser = command_path)]
        backup: PathBuf,
        #[arg(long, value_parser = command_path)]
        receipt: PathBuf,
        #[arg(long, value_parser = command_path)]
        objects: PathBuf,
        #[arg(long, value_parser = command_path)]
        scratch: PathBuf,
    },
    /// Bind a recovery manifest to verified backup and artifact bytes; never activate it.
    Preflight {
        #[arg(long, value_parser = command_path)]
        backup: PathBuf,
        #[arg(long, value_parser = command_path)]
        receipt: PathBuf,
        #[arg(long, value_parser = command_path)]
        objects: PathBuf,
        #[arg(long, value_parser = command_path)]
        manifest: PathBuf,
        #[arg(long, value_parser = command_path)]
        observation: PathBuf,
        #[arg(long, value_parser = command_path)]
        fence: PathBuf,
    },
    /// Evaluate recorded recovery evidence; external authority stays unverified and sessions held.
    RestartAdmission {
        #[arg(long, value_parser = command_path)]
        manifest: PathBuf,
        #[arg(long, value_parser = command_path)]
        observation: PathBuf,
        #[arg(long, value_parser = command_path)]
        fence: PathBuf,
    },
}

fn command_path(value: &str) -> Result<PathBuf, String> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("recovery paths must fit 4096 bytes without control characters".into());
    }
    let path = PathBuf::from(value);
    validate_absolute_normalized(&path)
        .map_err(|_| "recovery paths must be absolute and normalized".to_owned())?;
    Ok(path)
}

fn read_command_json<T: for<'de> Deserialize<'de>>(path: &Path) -> RecoveryResult<T> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("input has no parent"))?;
    validate_private_directory(parent)?;
    Ok(serde_json::from_slice(&read_regular_bounded(
        path,
        MAX_COMMAND_JSON_BYTES,
    )?)?)
}

/// A CLI JSON document cannot authenticate a credential holder or fence a live
/// executor. Keep restart admission unavailable until a real trusted adapter is
/// installed by the service; there is intentionally no override flag.
struct UnverifiedExternalAuthority;
impl ExternalAuthorityVerifier for UnverifiedExternalAuthority {
    fn verify_external_authority(&self, _: &ExternalFence) -> RecoveryResult<bool> {
        Ok(false)
    }
}

pub fn run(args: RecoveryArgs) -> RecoveryResult<Value> {
    match args.command {
        RecoveryCommand::Backup {
            database,
            output,
            receipt_output,
            objects,
            artifacts,
        } => {
            validate_new_file_path(&output)?;
            validate_new_file_path(&receipt_output)?;
            if output == receipt_output {
                return Err(invalid("database and receipt outputs must be distinct"));
            }
            let entries: Vec<HashedFile> = read_command_json(&artifacts)?;
            if entries.len() > MAX_CLOSURE_OBJECTS {
                return Err(invalid("artifact closure exceeds the object limit"));
            }
            let references: Vec<_> = entries.iter().map(artifact_ref).collect();
            validate_private_directory(&objects)?;
            let object_store = ArtifactStore::open(&objects, MAX_CLOSURE_BYTES)?;
            // Refuse missing databases before Store::open could create one.
            validate_private_directory(
                database
                    .parent()
                    .ok_or_else(|| invalid("database has no parent"))?,
            )?;
            let metadata = fs::symlink_metadata(&database)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > MAX_DATABASE_BYTES
            {
                return Err(invalid(
                    "source must be an existing bounded regular database",
                ));
            }
            let store = Store::open(&database)?;
            let receipt = create_online_backup(&store, &output, &object_store, &references)?;
            write_new_private_file(&receipt_output, &serde_json::to_vec_pretty(&receipt)?)?;
            Ok(
                json!({"operation":"backup","backup":output,"receipt_file":receipt_output,
                "receipt":receipt,"native_execution_qualified":false,
                "restore_authorized":false,"effects_authorized":false}),
            )
        }
        RecoveryCommand::Verify {
            backup,
            receipt,
            objects,
        } => {
            let receipt: NativeBackupReceipt = read_command_json(&receipt)?;
            validate_private_directory(&objects)?;
            let object_store = ArtifactStore::open(objects, MAX_CLOSURE_BYTES)?;
            verify_backup(&backup, &receipt, &object_store)?;
            Ok(
                json!({"operation":"verify","verified":true,"source_cursor":receipt.source_cursor,
                "projection_digest":receipt.projection_digest,"artifact_count":receipt.artifact_closure.len(),
                "native_execution_qualified":false,"restore_authorized":false,"effects_authorized":false}),
            )
        }
        RecoveryCommand::Rehearse {
            backup,
            receipt,
            objects,
            scratch,
        } => {
            let receipt: NativeBackupReceipt = read_command_json(&receipt)?;
            validate_private_directory(&objects)?;
            let object_store = ArtifactStore::open(objects, MAX_CLOSURE_BYTES)?;
            let restored = restore_rehearsal(&backup, &receipt, &object_store, &scratch)?;
            // Full task/message contents remain in the scratch database. Keep
            // routine CLI output focused on exact recovery identity and proof.
            Ok(
                json!({"operation":"rehearse","scratch_database":scratch.join("restored.sqlite"),
                "source_cursor":restored.source_cursor,"restored_cursor":restored.restored_cursor,
                "database":restored.database,"projection_digest":restored.projection_digest,
                "restored_artifacts":restored.restored_artifacts,
                "task_count":restored.semantic_projection.tasks.len(),
                "message_count":restored.semantic_projection.messages.len(),
                "unresolved_sessions":restored.unresolved_sessions,"session_disposition":restored.session_disposition,
                "native_execution_qualified":false,"restore_authorized":restored.restore_authorized,
                "effects_authorized":restored.effects_authorized}),
            )
        }
        RecoveryCommand::Preflight {
            backup,
            receipt,
            objects,
            manifest,
            observation,
            fence,
        } => {
            let receipt: NativeBackupReceipt = read_command_json(&receipt)?;
            let manifest: BackupManifest = read_command_json(&manifest)?;
            let observation = read_command_json(&observation)?;
            let fence = read_command_json(&fence)?;
            validate_private_directory(&objects)?;
            let objects = ArtifactStore::open(objects, MAX_CLOSURE_BYTES)?;
            verify_recovery_manifest(&backup, &receipt, &objects, &manifest)?;
            let (proposal, admission) = propose_native_restart_admission(
                &manifest,
                &observation,
                &fence,
                &UnverifiedExternalAuthority,
                true,
            )?;
            Ok(
                json!({"operation":"recovery_preflight","recorded_only":true,
                "backup_verified":true,"source_cursor":receipt.source_cursor,
                "manifest_digest":manifest.manifest_digest,"proposal":proposal,"admission":admission,
                "native_execution_qualified":false,"restore_authorized":false,"effects_authorized":false}),
            )
        }
        RecoveryCommand::RestartAdmission {
            manifest,
            observation,
            fence,
        } => {
            let manifest = read_command_json(&manifest)?;
            let observation = read_command_json(&observation)?;
            let fence = read_command_json(&fence)?;
            let (proposal, admission) = propose_native_restart_admission(
                &manifest,
                &observation,
                &fence,
                &UnverifiedExternalAuthority,
                true,
            )?;
            Ok(
                json!({"operation":"restart_admission_proposal","recorded_only":true,
                "proposal":proposal,"admission":admission,"native_execution_qualified":false,
                "restore_authorized":false,"effects_authorized":false}),
            )
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HashedFile {
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionDisposition {
    UnknownHeld,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeBackupReceipt {
    pub schema_version: u32,
    pub source_cursor: Cursor,
    pub database: HashedFile,
    pub projection_digest: String,
    /// Only the caller-enumerated immutable objects are represented here.
    pub artifact_closure: Vec<HashedFile>,
    /// This Store has no authoritative live-session inventory; unknown stays held.
    pub unresolved_sessions: Option<Vec<String>>,
    pub session_disposition: SessionDisposition,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeRestoreReceipt {
    pub source_cursor: Cursor,
    pub restored_cursor: Cursor,
    pub database: HashedFile,
    pub projection_digest: String,
    pub semantic_projection: Projection,
    pub restored_artifacts: Vec<HashedFile>,
    /// `None` means no session source was available; it is not an empty inventory.
    pub unresolved_sessions: Option<Vec<String>>,
    pub session_disposition: SessionDisposition,
    pub restore_authorized: bool,
    pub effects_authorized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeRestartAdmissionProposal {
    pub proposed: bool,
    pub externally_verified: bool,
    pub unknown_live_legacy_jobs_held: bool,
    pub blockers: Vec<Blocker>,
}

/// An executor implements this only after consulting its external fence and
/// authority source. The Store's own recovery generation is not such a fence.
pub trait ExternalAuthorityVerifier {
    fn verify_external_authority(&self, fence: &ExternalFence) -> RecoveryResult<bool>;
}

/// Make a SQLite online backup without rebuilding or otherwise mutating the
/// source generation. The database itself and every additional closure member
/// are hashed through ArtifactStore's SHA-256 verifier.
pub fn create_online_backup(
    store: &Store,
    output: &Path,
    object_store: &ArtifactStore,
    enumerated_artifacts: &[admiralty_store::artifacts::ArtifactRef],
) -> RecoveryResult<NativeBackupReceipt> {
    validate_new_file_path(output)?;
    let closure = verify_enumerated_closure(object_store, enumerated_artifacts)?;
    let backup_snapshot = store.backup(output)?;
    let source_after = store.snapshot()?;
    if source_after.cursor.recovery_generation != backup_snapshot.cursor.recovery_generation
        || canonical_counter(&source_after.cursor.event_seq)?
            < canonical_counter(&backup_snapshot.cursor.event_seq)?
    {
        return Err(invalid(
            "online backup changed or outran the source generation",
        ));
    }

    let database_bytes = read_regular_bounded(output, MAX_DATABASE_BYTES)?;
    let database_ref = object_store.publish_bytes(&database_bytes)?;
    let database = HashedFile {
        sha256: database_ref.sha256,
        size_bytes: database_ref.size_bytes,
    };
    let receipt = NativeBackupReceipt {
        schema_version: backup_snapshot.schema_version,
        source_cursor: backup_snapshot.cursor,
        database,
        projection_digest: content_digest(&backup_snapshot.projection)?,
        artifact_closure: closure,
        unresolved_sessions: None,
        session_disposition: SessionDisposition::UnknownHeld,
    };
    verify_backup(output, &receipt, object_store)?;
    Ok(receipt)
}

/// Verify exact database bytes, schema, cursor, and semantic projection by
/// reopening the backup as a real Store. It does not mutate the source Store.
pub fn verify_backup(
    backup_path: &Path,
    receipt: &NativeBackupReceipt,
    object_store: &ArtifactStore,
) -> RecoveryResult<()> {
    validate_receipt(receipt)?;
    let bytes = read_regular_bounded(backup_path, MAX_DATABASE_BYTES)?;
    let actual = object_store.publish_bytes(&bytes)?;
    if actual.sha256 != receipt.database.sha256 || actual.size_bytes != receipt.database.size_bytes
    {
        return Err(invalid(
            "backup bytes do not match the recorded SHA-256 and size",
        ));
    }
    verify_enumerated_closure_from_receipt(object_store, &receipt.artifact_closure)?;
    // Store::open may update SQLite header/journal metadata. Open an exact
    // private copy so verification never mutates the operator's backup bytes.
    let parent = backup_path
        .parent()
        .ok_or_else(|| invalid("backup path has no parent directory"))?;
    validate_private_directory(parent)?;
    let verification_dir = tempfile::Builder::new()
        .prefix(".admiralty-backup-verify-")
        .tempdir_in(parent)?;
    fs::set_permissions(verification_dir.path(), fs::Permissions::from_mode(0o700))?;
    let verification_database = verification_dir.path().join("verify.sqlite");
    write_new_private_file(&verification_database, &bytes)?;
    let reopened = Store::open(&verification_database).map_err(|error| {
        io::Error::other(format!(
            "could not reopen isolated verification copy in {}: {error}",
            verification_dir.path().display()
        ))
    })?;
    let snapshot = reopened.snapshot()?;
    if snapshot.schema_version != receipt.schema_version
        || snapshot.cursor != receipt.source_cursor
        || content_digest(&snapshot.projection)? != receipt.projection_digest
    {
        return Err(invalid(
            "backup cursor or semantic projection differs from receipt",
        ));
    }
    drop(reopened);
    Ok(())
}

/// Bind the planner's metadata to an actual SQLite backup and its complete
/// enumerated byte closure. External executor generations remain independent
/// of the Store's opaque reader generation; neither grants restart authority.
pub fn verify_recovery_manifest(
    backup_path: &Path,
    receipt: &NativeBackupReceipt,
    objects: &ArtifactStore,
    manifest: &BackupManifest,
) -> RecoveryResult<()> {
    // Check serialized/collection bounds before copying or sorting supplied data.
    if admiralty_runtime::recovery::manifest_digest(manifest)? != manifest.manifest_digest {
        return Err(invalid("recovery manifest digest mismatch"));
    }
    verify_backup(backup_path, receipt, objects)?;
    if manifest.schema_version != receipt.schema_version
        || manifest.snapshot_digest != receipt.database.sha256
        || manifest.snapshot_size.get() != receipt.database.size_bytes
        || manifest.snapshot_watermark.get() != canonical_counter(&receipt.source_cursor.event_seq)?
    {
        return Err(invalid(
            "recovery manifest does not identify the verified backup",
        ));
    }
    let expected: BTreeSet<_> = receipt
        .artifact_closure
        .iter()
        .chain(std::iter::once(&receipt.database))
        .map(|entry| (entry.sha256.as_str(), entry.size_bytes))
        .collect();
    let declared: BTreeSet<_> = manifest
        .artifact_closure
        .iter()
        .map(|entry| (entry.digest.as_str(), entry.size.get()))
        .collect();
    if expected != declared || declared.len() != manifest.artifact_closure.len() {
        return Err(invalid(
            "recovery manifest differs from the verified artifact closure",
        ));
    }
    // A manifest's `verified` booleans are metadata. Independently read every
    // required member, including the snapshot's content-addressed copy.
    for entry in &manifest.artifact_closure {
        if !entry.verified
            || admiralty_runtime::recovery::artifact_closure_id(entry)? != entry.closure_id
        {
            return Err(invalid(
                "recovery manifest has an invalid artifact identity",
            ));
        }
        objects.read_verified(&admiralty_store::artifacts::ArtifactRef {
            sha256: entry.digest.clone(),
            size_bytes: entry.size.get(),
        })?;
    }
    Ok(())
}

/// Reopen, replay/rebuild and restore only within a new, private, empty scratch
/// directory. No live path is replaced and no process is signalled.
pub fn restore_rehearsal(
    backup_path: &Path,
    receipt: &NativeBackupReceipt,
    source_objects: &ArtifactStore,
    scratch_root: &Path,
) -> RecoveryResult<NativeRestoreReceipt> {
    validate_empty_owned_directory(scratch_root)?;
    verify_backup(backup_path, receipt, source_objects)?;

    // Preflight the complete explicit closure before creating anything in the
    // destination, so a missing object cannot leave a partial rehearsal tree.
    for entry in &receipt.artifact_closure {
        source_objects.read_verified(&artifact_ref(entry))?;
    }

    let artifact_root = scratch_root.join("artifacts");
    fs::create_dir(&artifact_root)?;
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))?;
    let restored_objects = ArtifactStore::open(&artifact_root, MAX_CLOSURE_BYTES)?;
    let mut restored_artifacts = Vec::with_capacity(receipt.artifact_closure.len());
    for entry in &receipt.artifact_closure {
        let bytes = source_objects.read_verified(&artifact_ref(entry))?;
        let copied = restored_objects.publish_bytes(&bytes)?;
        if copied.sha256 != entry.sha256 || copied.size_bytes != entry.size_bytes {
            return Err(invalid(
                "restored artifact differs from its enumerated closure",
            ));
        }
        restored_artifacts.push(entry.clone());
    }

    let stage = scratch_root.join(".recovery-stage.sqlite");
    let final_database = scratch_root.join("restored.sqlite");
    let bytes = read_regular_bounded(backup_path, MAX_DATABASE_BYTES)?;
    let staged_ref = source_objects.publish_bytes(&bytes)?;
    if staged_ref.sha256 != receipt.database.sha256
        || staged_ref.size_bytes != receipt.database.size_bytes
    {
        return Err(invalid("backup changed after verification"));
    }
    write_new_private_file(&stage, &bytes)?;

    let rebuilt_snapshot = {
        let restored = Store::open(&stage)?;
        let before = restored.snapshot()?;
        if before.cursor != receipt.source_cursor
            || content_digest(&before.projection)? != receipt.projection_digest
        {
            return Err(invalid(
                "staged restore does not match the source projection",
            ));
        }
        let rebuilt = restored.rebuild()?;
        if rebuilt.cursor.event_seq != receipt.source_cursor.event_seq
            || rebuilt.projection != before.projection
            || rebuilt.cursor.recovery_generation == receipt.source_cursor.recovery_generation
        {
            return Err(invalid(
                "projection rebuild failed to preserve semantics and rotate generation",
            ));
        }
        if restored.events_since(&receipt.source_cursor, 1).is_ok() {
            return Err(invalid(
                "pre-restore reader cursor remained valid after generation rotation",
            ));
        }
        rebuilt
    };

    // The staging filename owns its lock file. Close Store first, then move the
    // rebuilt database atomically to its final name inside the same empty root.
    for suffix in [".writer.lock", "-wal", "-shm", "-journal"] {
        remove_owned_file(&append_suffix(&stage, suffix))?;
    }
    fs::rename(&stage, &final_database)?;
    File::open(scratch_root)?.sync_all()?;

    let reopened = Store::open(&final_database)?;
    let final_snapshot = reopened.snapshot()?;
    if final_snapshot.cursor != rebuilt_snapshot.cursor
        || final_snapshot.projection != rebuilt_snapshot.projection
    {
        return Err(invalid(
            "published scratch restore failed final reopen verification",
        ));
    }
    drop(reopened);

    Ok(NativeRestoreReceipt {
        source_cursor: receipt.source_cursor.clone(),
        restored_cursor: final_snapshot.cursor,
        database: receipt.database.clone(),
        projection_digest: content_digest(&final_snapshot.projection)?,
        semantic_projection: final_snapshot.projection,
        restored_artifacts,
        unresolved_sessions: None,
        session_disposition: SessionDisposition::UnknownHeld,
        restore_authorized: false,
        effects_authorized: false,
    })
}

/// Validate runtime recovery evidence and return a proposal only after the
/// caller's external verifier confirms the fence. This never grants authority.
pub fn propose_native_restart_admission(
    manifest: &BackupManifest,
    observation: &RecoveryObservation,
    fence: &ExternalFence,
    verifier: &dyn ExternalAuthorityVerifier,
    unknown_live_legacy_jobs: bool,
) -> RecoveryResult<(RecoveryProposal, NativeRestartAdmissionProposal)> {
    let proposal = plan_recovery(manifest, observation, Some(fence))?;
    let externally_verified = verifier.verify_external_authority(fence)?;
    let mut blockers = proposal.blockers.clone();
    if !externally_verified {
        blockers.push(Blocker::MissingExternalFence);
    }
    if unknown_live_legacy_jobs {
        blockers.push(Blocker::UnknownSessionCustody);
    }
    let proposed = proposal.resume_proposed
        && externally_verified
        && !unknown_live_legacy_jobs
        && blockers.is_empty();
    Ok((
        proposal,
        NativeRestartAdmissionProposal {
            proposed,
            externally_verified,
            unknown_live_legacy_jobs_held: unknown_live_legacy_jobs,
            blockers,
        },
    ))
}

fn verify_enumerated_closure(
    store: &ArtifactStore,
    refs: &[admiralty_store::artifacts::ArtifactRef],
) -> RecoveryResult<Vec<HashedFile>> {
    if refs.len() > MAX_CLOSURE_OBJECTS {
        return Err(invalid("artifact closure exceeds the object limit"));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut entries = Vec::with_capacity(refs.len());
    for reference in refs {
        if !seen.insert(reference.sha256.clone()) {
            return Err(invalid("artifact closure contains a duplicate digest"));
        }
        total = total
            .checked_add(reference.size_bytes)
            .ok_or_else(|| invalid("artifact closure size overflow"))?;
        if total > MAX_CLOSURE_BYTES {
            return Err(invalid("artifact closure exceeds the byte limit"));
        }
        let bytes = store.read_verified(reference)?;
        entries.push(HashedFile {
            sha256: reference.sha256.clone(),
            size_bytes: u64::try_from(bytes.len())?,
        });
    }
    entries.sort_by(|left, right| left.sha256.cmp(&right.sha256));
    Ok(entries)
}

fn verify_enumerated_closure_from_receipt(
    store: &ArtifactStore,
    entries: &[HashedFile],
) -> RecoveryResult<()> {
    if entries.len() > MAX_CLOSURE_OBJECTS {
        return Err(invalid("artifact closure exceeds the object limit"));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for entry in entries {
        if !seen.insert(entry.sha256.clone()) {
            return Err(invalid("artifact closure contains a duplicate digest"));
        }
        total = total
            .checked_add(entry.size_bytes)
            .ok_or_else(|| invalid("artifact closure size overflow"))?;
        if total > MAX_CLOSURE_BYTES {
            return Err(invalid("artifact closure exceeds the byte limit"));
        }
        let bytes = store.read_verified(&artifact_ref(entry))?;
        if u64::try_from(bytes.len())? != entry.size_bytes {
            return Err(invalid("artifact closure size differs from its receipt"));
        }
    }
    Ok(())
}

fn validate_receipt(receipt: &NativeBackupReceipt) -> RecoveryResult<()> {
    if !matches!(receipt.schema_version, 1 | 2)
        || receipt.database.sha256.len() != 71
        || !receipt.database.sha256.starts_with("sha256:")
        || receipt.database.size_bytes == 0
        || receipt.database.size_bytes > MAX_DATABASE_BYTES
        || receipt.projection_digest.len() != 71
        || !receipt.projection_digest.starts_with("sha256:")
        || receipt.session_disposition != SessionDisposition::UnknownHeld
        || receipt.unresolved_sessions.is_some()
    {
        return Err(invalid("backup receipt is malformed or over bounds"));
    }
    canonical_counter(&receipt.source_cursor.event_seq)?;
    Ok(())
}

fn artifact_ref(file: &HashedFile) -> admiralty_store::artifacts::ArtifactRef {
    admiralty_store::artifacts::ArtifactRef {
        sha256: file.sha256.clone(),
        size_bytes: file.size_bytes,
    }
}

fn canonical_counter(value: &str) -> RecoveryResult<u64> {
    let parsed = value
        .parse::<u64>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or_else(|| invalid("Store cursor is not a canonical u64"))?;
    Ok(parsed)
}

fn validate_new_file_path(path: &Path) -> RecoveryResult<()> {
    validate_absolute_normalized(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("output path has no parent directory"))?;
    validate_private_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => Err(invalid("create-only output already exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_empty_owned_directory(path: &Path) -> RecoveryResult<()> {
    validate_private_directory(path)?;
    if fs::read_dir(path)?.next().is_some() {
        return Err(invalid(
            "restore destination must be a new empty scratch directory",
        ));
    }
    Ok(())
}

fn validate_private_directory(path: &Path) -> RecoveryResult<()> {
    validate_absolute_normalized(path)?;
    let mut current = PathBuf::from("/");
    for component in path.components() {
        if let Component::Normal(name) = component {
            current.push(name);
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(invalid(
                    "directory path contains a symlink or non-directory",
                ));
            }
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("directory must be owned by this user and private"));
    }
    Ok(())
}

fn validate_absolute_normalized(path: &Path) -> RecoveryResult<()> {
    if !path.is_absolute()
        || path
            .to_str()
            .is_some_and(|value| value.split('/').any(|part| matches!(part, "." | "..")))
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid("path must be absolute and normalized"));
    }
    Ok(())
}

fn read_regular_bounded(path: &Path, max_bytes: u64) -> RecoveryResult<Vec<u8>> {
    validate_absolute_normalized(path)?;
    let before = fs::symlink_metadata(path)?;
    if before.file_type().is_symlink()
        || !before.is_file()
        || before.len() > max_bytes
        || before.uid() != rustix::process::geteuid().as_raw()
        || before.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("file is not regular or exceeds its size bound"));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    let mut file = options.open(path)?;
    if file.metadata()?.len() != before.len() {
        return Err(invalid("file changed before bounded read"));
    }
    let capacity = usize::try_from(before.len())?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if bytes.len() as u64 != before.len()
        || bytes.len() as u64 > max_bytes
        || after.len() != before.len()
        || named.file_type().is_symlink()
        || named.len() != before.len()
        || named.uid() != before.uid()
        || named.permissions().mode() & 0o077 != 0
        || after.ino() != before.ino()
        || after.dev() != before.dev()
    {
        return Err(invalid("file changed during bounded read"));
    }
    Ok(bytes)
}

fn write_new_private_file(path: &Path, bytes: &[u8]) -> RecoveryResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_owned_file(path: &Path) -> RecoveryResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(invalid(
            "owned recovery staging sidecar is not a regular file",
        )),
        Ok(_) => {
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn invalid(message: &str) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(
        io::ErrorKind::InvalidInput,
        message.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{
        AttemptId, CoordinationMessage, DomainEvent, MessageId, MessagePayload, PrincipalId,
        ProjectId, RootId, SourceIdentity, TaskContract, TaskId,
    };
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    const NOW: &str = "2026-10-18T20:30:00Z";

    fn private_tempdir() -> TempDir {
        let directory = tempfile::tempdir().expect("temporary directory is created");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("temporary directory is private");
        directory
    }

    fn source(sequence: u64) -> SourceIdentity {
        SourceIdentity {
            producer_id: "native-recovery-test".into(),
            incarnation: "fixture-v1".into(),
            sequence: sequence.to_string(),
        }
    }

    fn populate_store(database: &Path) -> Store {
        let store = Store::open(database).expect("real temporary Store opens");
        let contract: TaskContract = serde_json::from_value(json!({
            "schema_version":1,"project_id":"recovery-project","root_id":"recovery-root",
            "task_id":"recovery-task","revision":1,"title":"Restore rehearsal task",
            "objective":"Preserve durable task and message facts","non_goals":["No live effects"],
            "owned_paths":["src/recovery.rs"],"forbidden_paths":["credentials"],"read_first":["README.md"],
            "dependencies":[],"criteria":[{"id":"C1","text":"Projection replays","check_ids":["offline-check"]}],
            "checks":[{"id":"offline-check","program":"true","args":[],"cwd":".","timeout_secs":5}],
            "authority_ref":"fixture:no-live-authority","max_attempts":1,"simulated":true
        })).expect("fixture TaskContract is valid");
        store
            .append(
                source(1),
                DomainEvent::TaskSubmitted { contract },
                NOW.into(),
            )
            .expect("task event is durably appended");
        let message = CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("recovery-question").expect("message id is valid"),
            project_id: ProjectId::new("recovery-project").expect("project id is valid"),
            sender: PrincipalId::new("recovery-owner").expect("sender is valid"),
            recipient: PrincipalId::new("recovery-worker").expect("recipient is valid"),
            causation_id: None,
            context_epoch: "fixture-epoch-1".into(),
            authority_ref: "fixture:no-live-authority".into(),
            payload: MessagePayload::Question {
                task_id: "recovery-task"
                    .to_owned()
                    .try_into()
                    .expect("task id is valid"),
                requirement: "Keep the task projection".into(),
                question: "Did replay preserve this message?".into(),
                options: vec!["yes".into(), "no".into()],
            },
        };
        store
            .append(
                source(2),
                DomainEvent::MessageQueued { message },
                NOW.into(),
            )
            .expect("message event is durably appended");
        store
    }

    fn private_artifact_store(path: &Path) -> ArtifactStore {
        fs::create_dir(path).expect("artifact root is created");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("artifact root is private");
        ArtifactStore::open(path, MAX_CLOSURE_BYTES).expect("artifact store opens")
    }

    fn make_backup(root: &Path) -> (Store, ArtifactStore, NativeBackupReceipt, PathBuf) {
        let database = root.join("live.sqlite");
        let store = populate_store(&database);
        let object_store = private_artifact_store(&root.join("objects"));
        let artifact = object_store
            .publish_bytes(b"explicitly enumerated release artifact")
            .expect("fixture artifact is published");
        let backup_path = root.join("backup.sqlite");
        let receipt = create_online_backup(&store, &backup_path, &object_store, &[artifact])
            .expect("online backup produces a receipt");
        (store, object_store, receipt, backup_path)
    }

    fn manifest_for_receipt(receipt: &NativeBackupReceipt) -> RecoveryResult<BackupManifest> {
        use admiralty_runtime::recovery::{ArtifactRecord, DecimalU64, artifact_closure_id};
        let mut artifacts = Vec::new();
        for entry in receipt
            .artifact_closure
            .iter()
            .chain(std::iter::once(&receipt.database))
        {
            let mut record = ArtifactRecord {
                closure_id: String::new(),
                digest: entry.sha256.clone(),
                size: DecimalU64::new(entry.size_bytes),
                verified: true,
            };
            record.closure_id = artifact_closure_id(&record)?;
            artifacts.push(record);
        }
        let mut manifest: BackupManifest = serde_json::from_value(json!({
            "scope":{"project_id":"recovery-project","root_id":"recovery-root",
                "task_id":"recovery-task","store_id":"recovery-store","attempt_id":"attempt-one",
                "owner":"recovery-owner","revision":1,"contract_digest":format!("sha256:{}", "a".repeat(64)),
                "executor_incarnation":"backup-executor"},
            "schema_version":receipt.schema_version,"snapshot_watermark":receipt.source_cursor.event_seq,
            "snapshot_digest":receipt.database.sha256,"snapshot_size":receipt.database.size_bytes.to_string(),
            "recovery_generation":"1","artifact_closure":artifacts,
            "active_bundle_digests":[receipt.artifact_closure[0].sha256],"manifest_digest":""
        }))?;
        manifest.manifest_digest = admiralty_runtime::recovery::manifest_digest(&manifest)?;
        Ok(manifest)
    }

    #[test]
    fn recovery_preflight_binds_real_backup_watermark_schema_digest_and_complete_closure()
    -> RecoveryResult<()> {
        let root = private_tempdir();
        let (store, objects, receipt, backup) = make_backup(root.path());
        let original = store.snapshot()?;
        let manifest = manifest_for_receipt(&receipt)?;
        verify_recovery_manifest(&backup, &receipt, &objects, &manifest)?;
        for mutation in 0..6 {
            let mut changed = manifest.clone();
            match mutation {
                0 => changed.snapshot_watermark = admiralty_runtime::recovery::DecimalU64::new(99),
                1 => changed.schema_version += 1,
                2 => changed.snapshot_digest = format!("sha256:{}", "f".repeat(64)),
                3 => changed.snapshot_size = admiralty_runtime::recovery::DecimalU64::new(1),
                4 => {
                    changed.artifact_closure.pop();
                }
                _ => changed
                    .artifact_closure
                    .push(changed.artifact_closure[0].clone()),
            }
            changed.manifest_digest = admiralty_runtime::recovery::manifest_digest(&changed)?;
            assert!(
                verify_recovery_manifest(&backup, &receipt, &objects, &changed).is_err(),
                "mutation {mutation}"
            );
        }
        assert_eq!(store.snapshot()?, original);
        assert_eq!(fs::read(&backup)?.len() as u64, receipt.database.size_bytes);
        Ok(())
    }

    #[test]
    fn recovery_preflight_refuses_missing_artifact_despite_verified_metadata() -> RecoveryResult<()>
    {
        let root = private_tempdir();
        let (_, objects, receipt, backup) = make_backup(root.path());
        let manifest = manifest_for_receipt(&receipt)?;
        // An independently located object store has the additional artifact,
        // but no snapshot object. Metadata booleans alone cannot fill that gap.
        let incomplete = private_artifact_store(&root.path().join("incomplete"));
        for entry in &receipt.artifact_closure {
            incomplete.publish_bytes(&objects.read_verified(&artifact_ref(entry))?)?;
        }
        // Backup verification republishes the snapshot from verified backup
        // bytes. Missing additional bytes still fail before admission planning.
        verify_recovery_manifest(&backup, &receipt, &incomplete, &manifest)?;
        let empty = private_artifact_store(&root.path().join("empty"));
        assert!(verify_recovery_manifest(&backup, &receipt, &empty, &manifest).is_err());
        assert!(
            empty
                .read_verified(&artifact_ref(&receipt.artifact_closure[0]))
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cli_backup_verify_and_rehearse_use_real_store_without_activation() -> RecoveryResult<()>
    {
        use clap::Parser;
        let root = private_tempdir();
        let database = root.path().join("cli-source.sqlite");
        let source_store = populate_store(&database);
        let original = source_store.snapshot()?;
        drop(source_store);
        let object_path = root.path().join("cli-objects");
        let objects = private_artifact_store(&object_path);
        let artifact = objects.publish_bytes(b"CLI explicitly enumerated immutable evidence")?;
        let closure = root.path().join("closure.json");
        write_new_private_file(
            &closure,
            &serde_json::to_vec(&vec![HashedFile {
                sha256: artifact.sha256,
                size_bytes: artifact.size_bytes,
            }])?,
        )?;
        let output = root.path().join("cli-backup.sqlite");
        let receipt_file = root.path().join("cli-receipt.json");
        let scratch = root.path().join("cli-rehearsal");
        fs::create_dir(&scratch)?;
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))?;
        let text = |path: &Path| -> RecoveryResult<String> {
            Ok(path
                .to_str()
                .ok_or_else(|| invalid("fixture path is UTF-8"))?
                .to_owned())
        };
        let backup_args = vec![
            "admiralty".into(),
            "recover".into(),
            "backup".into(),
            "--database".into(),
            text(&database)?,
            "--output".into(),
            text(&output)?,
            "--receipt-output".into(),
            text(&receipt_file)?,
            "--objects".into(),
            text(&object_path)?,
            "--artifacts".into(),
            text(&closure)?,
        ];
        crate::run(crate::Cli::try_parse_from(&backup_args)?).await?;
        let receipt: NativeBackupReceipt = read_command_json(&receipt_file)?;
        assert_eq!(receipt.source_cursor, original.cursor);
        assert_eq!(
            receipt.projection_digest,
            content_digest(&original.projection)?
        );
        assert_eq!(receipt_file.metadata()?.permissions().mode() & 0o777, 0o600);
        let manifest = manifest_for_receipt(&receipt)?;
        let manifest_file = root.path().join("recovery-manifest.json");
        let observation_file = root.path().join("recovery-observation.json");
        let fence_file = root.path().join("recovery-fence.json");
        let observation = json!({
            "scope":manifest.scope,"manifest_digest":manifest.manifest_digest,
            "snapshot_digest":manifest.snapshot_digest,"snapshot_size":manifest.snapshot_size,
            "snapshot_watermark":manifest.snapshot_watermark,"backup_generation":"1","current_generation":"1",
            "old_executor_incarnation":"backup-executor","observed_bundle_digests":manifest.active_bundle_digests,
            "available_artifacts":manifest.artifact_closure,"compatibility":"compatible",
            "unresolved_effects":"clear","session_custody":"reconciled"
        });
        let fence = json!({
            "scope":manifest.scope,"manifest_digest":manifest.manifest_digest,
            "old_executor_incarnation":"backup-executor","new_executor_incarnation":"new-executor",
            "old_generation":"1","new_generation":"2",
            "external_receipt_digest":format!("sha256:{}", "c".repeat(64)),"old_executor_fenced":true
        });
        for (path, value) in [
            (&manifest_file, serde_json::to_value(&manifest)?),
            (&observation_file, observation),
            (&fence_file, fence),
        ] {
            write_new_private_file(path, &serde_json::to_vec(&value)?)?;
        }
        let preflight = run(RecoveryArgs {
            command: RecoveryCommand::Preflight {
                backup: output.clone(),
                receipt: receipt_file.clone(),
                objects: object_path.clone(),
                manifest: manifest_file,
                observation: observation_file,
                fence: fence_file,
            },
        })?;
        assert_eq!(preflight["backup_verified"], true);
        assert_eq!(preflight["admission"]["proposed"], false);
        assert_eq!(preflight["admission"]["externally_verified"], false);
        assert_eq!(
            preflight["admission"]["unknown_live_legacy_jobs_held"],
            true
        );
        assert_eq!(preflight["effects_authorized"], false);
        for (operation, extra) in [
            ("verify", Vec::new()),
            ("rehearse", vec!["--scratch".into(), text(&scratch)?]),
        ] {
            let mut args = vec![
                "admiralty".into(),
                "recover".into(),
                operation.into(),
                "--backup".into(),
                text(&output)?,
                "--receipt".into(),
                text(&receipt_file)?,
                "--objects".into(),
                text(&object_path)?,
            ];
            args.extend(extra);
            crate::run(crate::Cli::try_parse_from(args)?).await?;
        }
        let restored = Store::open(scratch.join("restored.sqlite"))?.snapshot()?;
        assert_eq!(restored.projection, original.projection);
        assert_ne!(
            restored.cursor.recovery_generation,
            original.cursor.recovery_generation
        );
        assert_eq!(Store::open(&database)?.snapshot()?, original);
        let bytes = fs::read(&output)?;
        assert!(
            crate::run(crate::Cli::try_parse_from(backup_args)?)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(&output)?,
            bytes,
            "existing backup is never replaced"
        );
        Ok(())
    }

    #[test]
    fn lifecycle_v2_backup_and_rehearsal_preserve_terminal_metadata() -> RecoveryResult<()> {
        use admiralty_core::{LifecycleAction, LifecycleCommand, SchedulingState, TaskView};
        struct FixtureAuthority;
        impl admiralty_store::LifecycleAuthority for FixtureAuthority {
            fn authorize(
                &self,
                _: &SourceIdentity,
                command: &LifecycleCommand,
                _: &TaskView,
                effect: Option<&admiralty_store::LifecycleEffect<'_>>,
            ) -> bool {
                effect.is_none() && matches!(command.action, LifecycleAction::CancelPending { .. })
            }
        }
        let root = private_tempdir();
        let database = root.path().join("native-v2.sqlite");
        drop(populate_store(&database));
        let store =
            Store::open_with_lifecycle_authority(&database, std::sync::Arc::new(FixtureAuthority))?;
        let snapshot = store.snapshot()?;
        let task = snapshot
            .projection
            .tasks
            .values()
            .next()
            .ok_or_else(|| invalid("fixture task"))?;
        store.execute_lifecycle(
            source(3),
            LifecycleCommand {
                protocol_version: 2,
                task_id: task.contract.task_id.clone(),
                expected_task_digest: content_digest(task)?,
                actor: PrincipalId::new("fixture-owner")?,
                action: LifecycleAction::CancelPending {
                    reason: "offline lifecycle recovery fixture".into(),
                },
            },
            NOW.into(),
            None,
        )?;
        let expected = store.snapshot()?;
        assert_eq!(expected.schema_version, 2);
        let objects = private_artifact_store(&root.path().join("v2-objects"));
        let backup = root.path().join("v2-backup.sqlite");
        let receipt = create_online_backup(&store, &backup, &objects, &[])?;
        assert_eq!(receipt.schema_version, 2);
        let scratch = root.path().join("v2-rehearsal");
        fs::create_dir(&scratch)?;
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))?;
        let restored = restore_rehearsal(&backup, &receipt, &objects, &scratch)?;
        assert_eq!(restored.semantic_projection, expected.projection);
        assert_eq!(
            restored
                .semantic_projection
                .tasks
                .values()
                .next()
                .ok_or_else(|| invalid("restored task"))?
                .state,
            SchedulingState::Cancelled
        );
        assert!(!restored.restore_authorized && !restored.effects_authorized);
        Ok(())
    }

    #[test]
    fn real_task_message_backup_reopen_rebuild_and_restore_rehearsal() {
        let root = private_tempdir();
        let (source_store, object_store, receipt, backup) = make_backup(root.path());
        let source_after_backup = source_store.snapshot().expect("source remains readable");
        assert_eq!(source_after_backup.cursor, receipt.source_cursor);
        assert_eq!(source_after_backup.projection.tasks.len(), 1);
        assert_eq!(source_after_backup.projection.messages.len(), 1);
        verify_backup(&backup, &receipt, &object_store).expect("backup verifies after reopen");

        let scratch = root.path().join("restore-scratch");
        fs::create_dir(&scratch).expect("new scratch root is created");
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))
            .expect("scratch root is private");
        let restored = restore_rehearsal(&backup, &receipt, &object_store, &scratch)
            .expect("isolated restore rehearsal succeeds");
        assert_eq!(restored.source_cursor, receipt.source_cursor);
        assert_eq!(restored.semantic_projection, source_after_backup.projection);
        assert_ne!(
            restored.restored_cursor.recovery_generation,
            receipt.source_cursor.recovery_generation
        );
        assert_eq!(
            restored.restored_cursor.event_seq,
            receipt.source_cursor.event_seq
        );
        assert_eq!(restored.restored_artifacts, receipt.artifact_closure);
        assert_eq!(restored.unresolved_sessions, None);
        assert_eq!(
            restored.session_disposition,
            SessionDisposition::UnknownHeld
        );
        assert!(!restored.restore_authorized);
        assert!(!restored.effects_authorized);
        drop(source_store);
    }

    #[test]
    fn partial_backup_missing_artifact_and_destination_collision_fail_closed() {
        let root = private_tempdir();
        let (_source, object_store, receipt, backup) = make_backup(root.path());

        let partial = root.path().join("partial.sqlite");
        let bytes = read_regular_bounded(&backup, MAX_DATABASE_BYTES).expect("read backup bytes");
        write_new_private_file(&partial, &bytes[..bytes.len() / 2]).expect("write truncated copy");
        assert!(verify_backup(&partial, &receipt, &object_store).is_err());

        let missing_objects = private_artifact_store(&root.path().join("missing-objects"));
        let missing_destination = root.path().join("missing-destination");
        fs::create_dir(&missing_destination).expect("new scratch root is created");
        fs::set_permissions(&missing_destination, fs::Permissions::from_mode(0o700))
            .expect("scratch root is private");
        assert!(
            restore_rehearsal(&backup, &receipt, &missing_objects, &missing_destination).is_err()
        );
        assert_eq!(
            fs::read_dir(&missing_destination)
                .expect("destination remains readable")
                .count(),
            0
        );

        let collision = root.path().join("collision-destination");
        fs::create_dir(&collision).expect("collision root is created");
        fs::set_permissions(&collision, fs::Permissions::from_mode(0o700))
            .expect("collision root is private");
        fs::write(collision.join("operator-data"), b"preserve me").expect("sentinel is written");
        assert!(restore_rehearsal(&backup, &receipt, &object_store, &collision).is_err());
        assert_eq!(
            fs::read(collision.join("operator-data")).expect("sentinel survives"),
            b"preserve me"
        );
    }

    #[test]
    fn create_only_backup_refuses_collision_without_changing_existing_bytes() {
        let root = private_tempdir();
        let store = populate_store(&root.path().join("live.sqlite"));
        let objects = private_artifact_store(&root.path().join("objects"));
        let output = root.path().join("existing.sqlite");
        fs::write(&output, b"preserve existing bytes").expect("sentinel is created");
        let before = fs::read(&output).expect("sentinel bytes are readable");
        assert!(create_online_backup(&store, &output, &objects, &[]).is_err());
        assert_eq!(fs::read(output).expect("sentinel remains readable"), before);
    }

    struct TestAuthority(bool);
    impl ExternalAuthorityVerifier for TestAuthority {
        fn verify_external_authority(&self, _fence: &ExternalFence) -> RecoveryResult<bool> {
            Ok(self.0)
        }
    }

    // This test pins the fail-closed gate independently of the Store restore
    // exercise: unknown legacy jobs hold admission even when a fence verifies.
    #[test]
    fn admission_proposal_requires_external_fence_and_known_session_custody() {
        let scope = admiralty_runtime::recovery::RecoveryScope {
            project_id: ProjectId::new("recovery-project").expect("project id is valid"),
            root_id: RootId::new("recovery-root").expect("root id is valid"),
            task_id: TaskId::new("recovery-task").expect("task id is valid"),
            store_id: "recovery-store".into(),
            attempt_id: AttemptId::new("recovery-attempt").expect("attempt id is valid"),
            owner: PrincipalId::new("recovery-owner").expect("owner is valid"),
            revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            executor_incarnation: "old-executor".into(),
        };
        let snapshot_digest = format!("sha256:{}", "b".repeat(64));
        let mut snapshot_artifact = admiralty_runtime::recovery::ArtifactRecord {
            closure_id: String::new(),
            digest: snapshot_digest.clone(),
            size: admiralty_runtime::recovery::DecimalU64::new(100),
            verified: true,
        };
        snapshot_artifact.closure_id =
            admiralty_runtime::recovery::artifact_closure_id(&snapshot_artifact)
                .expect("artifact closure identity validates");
        let mut manifest = BackupManifest {
            scope: scope.clone(),
            schema_version: 1,
            snapshot_watermark: admiralty_runtime::recovery::DecimalU64::new(1),
            snapshot_digest: snapshot_digest.clone(),
            snapshot_size: admiralty_runtime::recovery::DecimalU64::new(100),
            recovery_generation: admiralty_runtime::recovery::DecimalU64::new(1),
            artifact_closure: vec![snapshot_artifact.clone()],
            active_bundle_digests: Vec::new(),
            manifest_digest: String::new(),
        };
        manifest.manifest_digest = admiralty_runtime::recovery::manifest_digest(&manifest)
            .expect("manifest identity validates");
        let observation = RecoveryObservation {
            scope: scope.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
            snapshot_digest,
            snapshot_size: admiralty_runtime::recovery::DecimalU64::new(100),
            snapshot_watermark: admiralty_runtime::recovery::DecimalU64::new(1),
            backup_generation: admiralty_runtime::recovery::DecimalU64::new(1),
            current_generation: admiralty_runtime::recovery::DecimalU64::new(1),
            old_executor_incarnation: "old-executor".into(),
            observed_bundle_digests: Vec::new(),
            available_artifacts: vec![snapshot_artifact],
            compatibility: admiralty_runtime::recovery::Compatibility::Compatible,
            unresolved_effects: admiralty_runtime::recovery::Reconciliation::Clear,
            session_custody: admiralty_runtime::recovery::SessionCustody::Reconciled,
        };
        let fence = ExternalFence {
            scope,
            manifest_digest: manifest.manifest_digest.clone(),
            old_executor_incarnation: "old-executor".into(),
            new_executor_incarnation: "new-executor".into(),
            old_generation: admiralty_runtime::recovery::DecimalU64::new(1),
            new_generation: admiralty_runtime::recovery::DecimalU64::new(2),
            external_receipt_digest: format!("sha256:{}", "c".repeat(64)),
            old_executor_fenced: Some(true),
        };

        let (_, allowed) = propose_native_restart_admission(
            &manifest,
            &observation,
            &fence,
            &TestAuthority(true),
            false,
        )
        .expect("valid runtime recovery evidence is evaluated");
        assert!(allowed.proposed);
        assert!(allowed.externally_verified);
        assert!(allowed.blockers.is_empty());

        let (_, unknown_jobs) = propose_native_restart_admission(
            &manifest,
            &observation,
            &fence,
            &TestAuthority(true),
            true,
        )
        .expect("legacy custody uncertainty is surfaced as a blocker");
        assert!(!unknown_jobs.proposed);
        assert!(unknown_jobs.unknown_live_legacy_jobs_held);
        assert!(
            unknown_jobs
                .blockers
                .contains(&Blocker::UnknownSessionCustody)
        );

        let (_, unverified) = propose_native_restart_admission(
            &manifest,
            &observation,
            &fence,
            &TestAuthority(false),
            false,
        )
        .expect("unverified external fence is a non-authorizing proposal");
        assert!(!unverified.proposed);
        assert!(unverified.blockers.contains(&Blocker::MissingExternalFence));

        // Even complete-looking recorded evidence is not an external authority
        // source. The actual CLI path has no boolean flag that can grant it.
        let root = private_tempdir();
        let manifest_file = root.path().join("manifest.json");
        let observation_file = root.path().join("observation.json");
        let fence_file = root.path().join("fence.json");
        for (path, bytes) in [
            (
                &manifest_file,
                serde_json::to_vec(&manifest).expect("manifest JSON"),
            ),
            (
                &observation_file,
                serde_json::to_vec(&observation).expect("observation JSON"),
            ),
            (&fence_file, serde_json::to_vec(&fence).expect("fence JSON")),
        ] {
            write_new_private_file(path, &bytes).expect("private CLI input");
        }
        let output = run(RecoveryArgs {
            command: RecoveryCommand::RestartAdmission {
                manifest: manifest_file,
                observation: observation_file,
                fence: fence_file,
            },
        })
        .expect("recorded recovery proposal");
        assert_eq!(output["admission"]["proposed"], false);
        assert_eq!(output["admission"]["externally_verified"], false);
        assert_eq!(output["admission"]["unknown_live_legacy_jobs_held"], true);
        assert_eq!(output["restore_authorized"], false);
        assert_eq!(output["effects_authorized"], false);
        assert_eq!(output["native_execution_qualified"], false);
    }
}
