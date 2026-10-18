//! Pure validation and proposal generation for recovery manifests.
//!
//! This module does not restore a database, execute effects, or contact a
//! credential holder. It reports whether supplied evidence is sufficient to
//! propose resuming work.

use admiralty_core::{ProjectId, RootId, TaskId, content_digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_COLLECTION: usize = 1024;
const MAX_STRING: usize = 4096;
const MAX_IDENTIFIER: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct DecimalU64(u64);

impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}

impl TryFrom<String> for DecimalU64 {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("counter must be a canonical decimal u64".into());
        }
        value
            .parse()
            .map(Self)
            .map_err(|_| "counter exceeds u64".into())
    }
}
impl From<DecimalU64> for String {
    fn from(value: DecimalU64) -> Self {
        value.0.to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    /// Stable ID derived from this complete immutable record's content.
    pub closure_id: String,
    pub digest: String,
    pub size: DecimalU64,
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub scope: RecoveryScope,
    pub schema_version: u32,
    /// Exact store snapshot identity captured by the supported online backup.
    pub snapshot_watermark: DecimalU64,
    pub recovery_generation: DecimalU64,
    pub artifact_closure: Vec<ArtifactRecord>,
    pub active_bundle_digests: Vec<String>,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalFence {
    pub old_generation: DecimalU64,
    pub new_generation: DecimalU64,
    pub external_receipt_digest: String,
    pub old_executor_fenced: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reconciliation {
    Clear,
    Unresolved,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionCustody {
    Reconciled,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryObservation {
    pub available_artifacts: Vec<ArtifactRecord>,
    pub compatibility: Compatibility,
    pub unresolved_effects: Reconciliation,
    pub session_custody: SessionCustody,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    InvalidManifest(String),
    MissingArtifact(String),
    MismatchedArtifact(String),
    IncompatibleActiveBundle,
    UnknownCompatibility,
    UnresolvedRemoteMutation,
    UnknownRemoteMutation,
    UnknownSessionCustody,
    MissingExternalFence,
    StaleExternalGeneration,
    OldExecutorNotFenced,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryProposal {
    pub scope: RecoveryScope,
    pub blockers: Vec<Blocker>,
    pub resume_proposed: bool,
    /// Restore authority is owned by a qualified executor, never this proposal.
    pub restore_authorized: bool,
    /// External effects remain disabled pending executor qualification.
    pub effects_authorized: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecoveryError {
    #[error("serialized input exceeds 1 MiB")]
    InputTooLarge,
    #[error("invalid recovery input: {0}")]
    Invalid(String),
}

pub fn artifact_closure_id(record: &ArtifactRecord) -> Result<String, RecoveryError> {
    #[derive(Serialize)]
    struct Content<'a> {
        digest: &'a str,
        size: &'a DecimalU64,
        verified: bool,
    }
    content_digest(&Content {
        digest: &record.digest,
        size: &record.size,
        verified: record.verified,
    })
    .map_err(|e| RecoveryError::Invalid(e.to_string()))
}

pub fn manifest_digest(manifest: &BackupManifest) -> Result<String, RecoveryError> {
    #[derive(Serialize)]
    struct Content<'a> {
        scope: &'a RecoveryScope,
        schema_version: u32,
        snapshot_watermark: &'a DecimalU64,
        recovery_generation: &'a DecimalU64,
        artifact_closure: &'a [ArtifactRecord],
        active_bundle_digests: &'a [String],
    }
    let mut artifacts = manifest.artifact_closure.clone();
    artifacts.sort_by(|a, b| a.closure_id.cmp(&b.closure_id));
    let mut bundles = manifest.active_bundle_digests.clone();
    bundles.sort();
    content_digest(&Content {
        scope: &manifest.scope,
        schema_version: manifest.schema_version,
        snapshot_watermark: &manifest.snapshot_watermark,
        recovery_generation: &manifest.recovery_generation,
        artifact_closure: &artifacts,
        active_bundle_digests: &bundles,
    })
    .map_err(|e| RecoveryError::Invalid(e.to_string()))
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, RecoveryError> {
    let bytes = serde_json::to_vec(value).map_err(|e| RecoveryError::Invalid(e.to_string()))?;
    Ok(bytes.len())
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn validate_artifacts(
    records: &[ArtifactRecord],
) -> Result<BTreeMap<String, ArtifactRecord>, RecoveryError> {
    if records.len() > MAX_COLLECTION {
        return Err(RecoveryError::Invalid(
            "artifact collection exceeds 1024".into(),
        ));
    }
    let mut by_id = BTreeMap::new();
    let mut by_digest = BTreeMap::<&str, &ArtifactRecord>::new();
    for record in records {
        if record.closure_id.len() > MAX_IDENTIFIER
            || record.digest.len() > MAX_STRING
            || !valid_digest(&record.digest)
        {
            return Err(RecoveryError::Invalid(
                "invalid artifact ID or SHA-256 digest".into(),
            ));
        }
        let expected = artifact_closure_id(record)?;
        if record.closure_id != expected {
            return Err(RecoveryError::Invalid(
                "artifact closure ID does not match immutable content".into(),
            ));
        }
        if let Some(previous) = by_id.get(&record.closure_id) {
            if previous != record {
                return Err(RecoveryError::Invalid("artifact ID collision".into()));
            }
            return Err(RecoveryError::Invalid(
                "duplicate artifact closure ID".into(),
            ));
        }
        if let Some(previous) = by_digest.insert(&record.digest, record) {
            if previous != record {
                return Err(RecoveryError::Invalid(
                    "same digest has conflicting size or verification".into(),
                ));
            }
            return Err(RecoveryError::Invalid("duplicate artifact digest".into()));
        }
        by_id.insert(record.closure_id.clone(), record.clone());
    }
    Ok(by_id)
}

pub fn plan_recovery(
    manifest: &BackupManifest,
    observation: &RecoveryObservation,
    fence: Option<&ExternalFence>,
) -> Result<RecoveryProposal, RecoveryError> {
    let mut input_size = serialized_size(manifest)?
        .checked_add(serialized_size(observation)?)
        .ok_or(RecoveryError::InputTooLarge)?;
    if let Some(fence) = fence {
        input_size = input_size
            .checked_add(serialized_size(fence)?)
            .ok_or(RecoveryError::InputTooLarge)?;
    }
    if input_size > MAX_JSON_BYTES {
        return Err(RecoveryError::InputTooLarge);
    }
    let mut blockers = Vec::new();
    if manifest.schema_version != 1 {
        blockers.push(Blocker::InvalidManifest(
            "unsupported schema version".into(),
        ));
    }
    if manifest.artifact_closure.len() > MAX_COLLECTION
        || manifest.active_bundle_digests.len() > MAX_COLLECTION
    {
        return Err(RecoveryError::Invalid(
            "manifest collection exceeds 1024".into(),
        ));
    }
    if manifest
        .active_bundle_digests
        .iter()
        .any(|d| !valid_digest(d))
    {
        return Err(RecoveryError::Invalid(
            "invalid active bundle digest".into(),
        ));
    }
    if manifest
        .active_bundle_digests
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != manifest.active_bundle_digests.len()
    {
        return Err(RecoveryError::Invalid(
            "duplicate active bundle digest".into(),
        ));
    }
    let expected_digest = manifest_digest(manifest)?;
    if manifest.manifest_digest != expected_digest {
        blockers.push(Blocker::InvalidManifest("manifest digest mismatch".into()));
    }
    let required = validate_artifacts(&manifest.artifact_closure)?;
    let available = validate_artifacts(&observation.available_artifacts)?;
    for (id, expected) in &required {
        match available.get(id) {
            None => blockers.push(Blocker::MissingArtifact(id.clone())),
            Some(found) if found != expected => {
                blockers.push(Blocker::MismatchedArtifact(id.clone()))
            }
            Some(_) if !expected.verified => blockers.push(Blocker::MismatchedArtifact(id.clone())),
            Some(_) => {}
        }
    }
    let required_digests: BTreeSet<_> = required.values().map(|r| r.digest.as_str()).collect();
    if available
        .values()
        .any(|r| !required_digests.contains(r.digest.as_str()))
    {
        blockers.push(Blocker::MismatchedArtifact(
            "unexpected artifact in observed closure".into(),
        ));
    }
    match observation.compatibility {
        Compatibility::Compatible => {}
        Compatibility::Incompatible => blockers.push(Blocker::IncompatibleActiveBundle),
        Compatibility::Unknown => blockers.push(Blocker::UnknownCompatibility),
    }
    match observation.unresolved_effects {
        Reconciliation::Clear => {}
        Reconciliation::Unresolved => blockers.push(Blocker::UnresolvedRemoteMutation),
        Reconciliation::Unknown => blockers.push(Blocker::UnknownRemoteMutation),
    }
    if observation.session_custody == SessionCustody::Unknown {
        blockers.push(Blocker::UnknownSessionCustody);
    }
    match fence {
        None => blockers.push(Blocker::MissingExternalFence),
        Some(fence) => {
            if !valid_digest(&fence.external_receipt_digest) {
                return Err(RecoveryError::Invalid(
                    "invalid external receipt digest".into(),
                ));
            }
            if fence.new_generation.get() <= manifest.recovery_generation.get()
                || fence.new_generation.get() <= fence.old_generation.get()
            {
                blockers.push(Blocker::StaleExternalGeneration);
            }
            if fence.old_executor_fenced != Some(true) {
                blockers.push(Blocker::OldExecutorNotFenced);
            }
        }
    }
    let resume_proposed = blockers.is_empty();
    Ok(RecoveryProposal {
        scope: manifest.scope.clone(),
        blockers,
        resume_proposed,
        restore_authorized: false,
        effects_authorized: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(n: u8) -> String {
        format!("sha256:{}{n:02x}", "0".repeat(62))
    }
    fn record(d: String, size: u64) -> ArtifactRecord {
        let mut r = ArtifactRecord {
            closure_id: String::new(),
            digest: d,
            size: DecimalU64::new(size),
            verified: true,
        };
        r.closure_id = artifact_closure_id(&r).expect("fixture closure ID hashes");
        r
    }
    fn fixture() -> (
        BackupManifest,
        RecoveryObservation,
        ExternalFence,
        RecoveryScope,
    ) {
        let artifacts = vec![record(digest(1), 7)];
        let scope = RecoveryScope {
            project_id: ProjectId::new("p").expect("fixture project ID is valid"),
            root_id: RootId::new("r").expect("fixture root ID is valid"),
            task_id: TaskId::new("t").expect("fixture task ID is valid"),
        };
        let mut m = BackupManifest {
            scope,
            schema_version: 1,
            snapshot_watermark: DecimalU64::new(9_007_199_254_740_993),
            recovery_generation: DecimalU64::new(4),
            artifact_closure: artifacts.clone(),
            active_bundle_digests: vec![digest(2)],
            manifest_digest: String::new(),
        };
        m.manifest_digest = manifest_digest(&m).expect("fixture manifest hashes");
        let o = RecoveryObservation {
            available_artifacts: artifacts,
            compatibility: Compatibility::Compatible,
            unresolved_effects: Reconciliation::Clear,
            session_custody: SessionCustody::Reconciled,
        };
        let f = ExternalFence {
            old_generation: DecimalU64::new(5),
            new_generation: DecimalU64::new(6),
            external_receipt_digest: digest(3),
            old_executor_fenced: Some(true),
        };
        let scope = m.scope.clone();
        (m, o, f, scope)
    }
    #[test]
    fn valid_closure_proposes_resume_without_authority() {
        let (m, o, f, s) = fixture();
        let p = plan_recovery(&m, &o, Some(&f)).expect("valid fixture plans");
        assert_eq!(p.scope, s);
        assert!(p.resume_proposed);
        assert!(!p.restore_authorized && !p.effects_authorized);
    }
    #[test]
    fn missing_and_changed_closure_block() {
        let (m, mut o, f, _s) = fixture();
        o.available_artifacts.clear();
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("missing evidence is a proposal")
                .blockers
                .iter()
                .any(|b| matches!(b, Blocker::MissingArtifact(_)))
        );
        o.available_artifacts.push(record(digest(1), 8));
        assert!(
            !plan_recovery(&m, &o, Some(&f))
                .expect("changed evidence is a proposal")
                .resume_proposed
        );
    }
    #[test]
    fn duplicate_and_collision_artifacts_fail() {
        let (m, mut o, f, _s) = fixture();
        o.available_artifacts.push(o.available_artifacts[0].clone());
        assert!(plan_recovery(&m, &o, Some(&f)).is_err());
        let mut r = record(digest(1), 8);
        r.closure_id = String::from("collision");
        assert!(validate_artifacts(&[r]).is_err());
    }
    #[test]
    fn input_order_is_irrelevant_but_manifest_replay_is_bound() {
        let (mut m, mut o, f, _s) = fixture();
        let extra = record(digest(4), 8);
        m.artifact_closure.push(extra.clone());
        m.manifest_digest = manifest_digest(&m).expect("order independent fixture hash");
        o.available_artifacts.push(extra);
        o.available_artifacts.reverse();
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("ordered observations reconcile")
                .resume_proposed
        );
    }
    #[test]
    fn changed_manifest_digest_blocks() {
        let (mut m, o, f, _s) = fixture();
        m.manifest_digest = digest(9);
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("tampering yields blocker")
                .blockers
                .iter()
                .any(|b| matches!(b, Blocker::InvalidManifest(_)))
        );
    }
    #[test]
    fn stale_missing_unknown_or_false_fence_blocks() {
        let (m, o, mut f, _s) = fixture();
        f.new_generation = DecimalU64::new(5);
        assert!(
            !plan_recovery(&m, &o, Some(&f))
                .expect("stale fence reported")
                .resume_proposed
        );
        f.new_generation = DecimalU64::new(6);
        f.old_executor_fenced = None;
        assert!(
            !plan_recovery(&m, &o, Some(&f))
                .expect("unknown fence reported")
                .resume_proposed
        );
        f.old_executor_fenced = Some(false);
        assert!(
            !plan_recovery(&m, &o, Some(&f))
                .expect("false fence reported")
                .resume_proposed
        );
        assert!(
            !plan_recovery(&m, &o, None)
                .expect("missing fence reported")
                .resume_proposed
        );
    }
    #[test]
    fn compatibility_and_remote_uncertainty_block() {
        let (m, mut o, f, _s) = fixture();
        o.compatibility = Compatibility::Incompatible;
        o.unresolved_effects = Reconciliation::Unknown;
        let p = plan_recovery(&m, &o, Some(&f)).expect("uncertainty is reported");
        assert!(p.blockers.contains(&Blocker::IncompatibleActiveBundle));
        assert!(p.blockers.contains(&Blocker::UnknownRemoteMutation));
    }
    #[test]
    fn decimal_counters_preserve_precision_and_reject_overflow() {
        assert_eq!(
            serde_json::from_str::<DecimalU64>("\"9007199254740993\"")
                .expect("large canonical value")
                .get(),
            9_007_199_254_740_993
        );
        assert!(serde_json::from_str::<DecimalU64>("\"18446744073709551616\"").is_err());
        assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
    }
    #[test]
    fn watermark_is_actual_input_and_never_authority() {
        let (m, o, f, _s) = fixture();
        let p = plan_recovery(&m, &o, Some(&f)).expect("fixture plans");
        assert_eq!(m.snapshot_watermark.get(), 9_007_199_254_740_993);
        assert!(!p.restore_authorized);
        assert!(!p.effects_authorized);
    }
}
