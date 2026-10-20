//! Pure validation and proposal generation for recovery manifests.
//!
//! This module does not restore a database, execute effects, or contact a
//! credential holder. It reports whether supplied evidence is sufficient to
//! propose resuming work.

use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size,
};
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
    pub store_id: String,
    pub attempt_id: AttemptId,
    pub owner: PrincipalId,
    pub revision: u32,
    pub contract_digest: String,
    pub executor_incarnation: String,
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
            || value.len() > 20
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
    /// Store snapshot/event schema, bound to the backup receipt (supported: 1 and 2).
    pub schema_version: u32,
    /// Exact store snapshot identity captured by the supported online backup.
    pub snapshot_watermark: DecimalU64,
    pub snapshot_digest: String,
    pub snapshot_size: DecimalU64,
    pub recovery_generation: DecimalU64,
    pub artifact_closure: Vec<ArtifactRecord>,
    pub active_bundle_digests: Vec<String>,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalFence {
    pub scope: RecoveryScope,
    pub manifest_digest: String,
    pub old_executor_incarnation: String,
    pub new_executor_incarnation: String,
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
    pub scope: RecoveryScope,
    pub manifest_digest: String,
    pub snapshot_digest: String,
    pub snapshot_size: DecimalU64,
    pub snapshot_watermark: DecimalU64,
    pub backup_generation: DecimalU64,
    pub current_generation: DecimalU64,
    pub old_executor_incarnation: String,
    pub observed_bundle_digests: Vec<String>,
    pub available_artifacts: Vec<ArtifactRecord>,
    pub compatibility: Compatibility,
    pub unresolved_effects: Reconciliation,
    pub session_custody: SessionCustody,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Blocker {
    ObservationMismatch,
    FenceMismatch,
    MissingBundleClosure,
    SnapshotMismatch,
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RecoveryProposal {
    pub scope: RecoveryScope,
    pub manifest_digest: String,
    pub snapshot_watermark: DecimalU64,
    pub proposed_generation: Option<DecimalU64>,
    pub fence_receipt_digest: Option<String>,
    pub blockers: Vec<Blocker>,
    pub resume_proposed: bool,
    /// Restore authority is owned by a qualified executor, never this proposal.
    restore_authorized: bool,
    /// External effects remain disabled pending executor qualification.
    effects_authorized: bool,
}
impl RecoveryProposal {
    pub fn restore_authorized(&self) -> bool {
        self.restore_authorized
    }
    pub fn effects_authorized(&self) -> bool {
        self.effects_authorized
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecoveryError {
    #[error("serialized input exceeds 1 MiB")]
    InputTooLarge,
    #[error("invalid recovery input: {0}")]
    Invalid(String),
}

pub fn artifact_closure_id(record: &ArtifactRecord) -> Result<String, RecoveryError> {
    serialized_size(record)?;
    if !valid_digest(&record.digest) || record.closure_id.len() > MAX_IDENTIFIER {
        return Err(RecoveryError::Invalid(
            "invalid bounded artifact identity".into(),
        ));
    }
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
    serialized_size(manifest)?;
    if manifest.artifact_closure.len() > MAX_COLLECTION
        || manifest.active_bundle_digests.len() > MAX_COLLECTION
    {
        return Err(RecoveryError::InputTooLarge);
    }
    #[derive(Serialize)]
    struct Content<'a> {
        scope: &'a RecoveryScope,
        schema_version: u32,
        snapshot_watermark: &'a DecimalU64,
        snapshot_digest: &'a str,
        snapshot_size: &'a DecimalU64,
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
        snapshot_digest: &manifest.snapshot_digest,
        snapshot_size: &manifest.snapshot_size,
        recovery_generation: &manifest.recovery_generation,
        artifact_closure: &artifacts,
        active_bundle_digests: &bundles,
    })
    .map_err(|e| RecoveryError::Invalid(e.to_string()))
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, RecoveryError> {
    bounded_json_size(value, MAX_JSON_BYTES).map_err(|_| RecoveryError::InputTooLarge)
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
    let scope = &manifest.scope;
    if scope.revision == 0
        || !valid_digest(&scope.contract_digest)
        || PrincipalId::new(&scope.store_id).is_err()
        || PrincipalId::new(&scope.executor_incarnation).is_err()
        || PrincipalId::new(&observation.old_executor_incarnation).is_err()
        || !valid_digest(&manifest.snapshot_digest)
        || !valid_digest(&manifest.manifest_digest)
        || manifest.snapshot_size.get() == 0
    {
        return Err(RecoveryError::Invalid(
            "invalid scoped store/snapshot identity".into(),
        ));
    }
    if observation.scope != *scope
        || observation.manifest_digest != manifest.manifest_digest
        || observation.snapshot_watermark != manifest.snapshot_watermark
        || observation.backup_generation != manifest.recovery_generation
        || observation.current_generation.get() < manifest.recovery_generation.get()
    {
        blockers.push(Blocker::ObservationMismatch);
    }
    if observation.snapshot_digest != manifest.snapshot_digest
        || observation.snapshot_size != manifest.snapshot_size
    {
        blockers.push(Blocker::SnapshotMismatch);
    }
    if !matches!(manifest.schema_version, 1 | 2) {
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
    if !required.values().any(|record| {
        record.digest == manifest.snapshot_digest
            && record.size == manifest.snapshot_size
            && record.verified
    }) {
        blockers.push(Blocker::SnapshotMismatch);
    }
    if manifest
        .active_bundle_digests
        .iter()
        .any(|digest| !required_digests.contains(digest.as_str()))
    {
        blockers.push(Blocker::MissingBundleClosure);
    }
    if observation.observed_bundle_digests.len() > MAX_COLLECTION {
        return Err(RecoveryError::InputTooLarge);
    }
    let observed_bundles: BTreeSet<_> = observation.observed_bundle_digests.iter().collect();
    if observed_bundles.len() != observation.observed_bundle_digests.len()
        || observation
            .observed_bundle_digests
            .iter()
            .any(|digest| !valid_digest(digest))
        || observed_bundles != manifest.active_bundle_digests.iter().collect()
    {
        blockers.push(Blocker::UnknownCompatibility);
    }
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
            if fence.scope != manifest.scope
                || fence.manifest_digest != manifest.manifest_digest
                || fence.old_generation != observation.current_generation
                || fence.old_executor_incarnation != observation.old_executor_incarnation
                || fence.new_executor_incarnation == fence.old_executor_incarnation
                || PrincipalId::new(&fence.new_executor_incarnation).is_err()
            {
                blockers.push(Blocker::FenceMismatch);
            }
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
        manifest_digest: manifest.manifest_digest.clone(),
        snapshot_watermark: manifest.snapshot_watermark.clone(),
        proposed_generation: fence.map(|fence| fence.new_generation.clone()),
        fence_receipt_digest: fence.map(|fence| fence.external_receipt_digest.clone()),
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
        let artifacts = vec![record(digest(1), 7), record(digest(2), 8)];
        let scope = RecoveryScope {
            project_id: ProjectId::new("p").expect("fixture project ID is valid"),
            root_id: RootId::new("r").expect("fixture root ID is valid"),
            store_id: "store-one".into(),
            attempt_id: AttemptId::new("attempt").expect("fixture attempt"),
            owner: PrincipalId::new("owner").expect("fixture owner"),
            revision: 1,
            contract_digest: digest(10),
            executor_incarnation: "backup-executor".into(),
            task_id: TaskId::new("t").expect("fixture task ID is valid"),
        };
        let mut m = BackupManifest {
            scope,
            schema_version: 1,
            snapshot_watermark: DecimalU64::new(9_007_199_254_740_993),
            snapshot_digest: digest(1),
            snapshot_size: DecimalU64::new(7),
            recovery_generation: DecimalU64::new(4),
            artifact_closure: artifacts.clone(),
            active_bundle_digests: vec![digest(2)],
            manifest_digest: String::new(),
        };
        m.manifest_digest = manifest_digest(&m).expect("fixture manifest hashes");
        let o = RecoveryObservation {
            scope: m.scope.clone(),
            manifest_digest: m.manifest_digest.clone(),
            snapshot_digest: m.snapshot_digest.clone(),
            snapshot_size: m.snapshot_size.clone(),
            snapshot_watermark: m.snapshot_watermark.clone(),
            backup_generation: m.recovery_generation.clone(),
            current_generation: DecimalU64::new(5),
            old_executor_incarnation: "old-executor".into(),
            observed_bundle_digests: m.active_bundle_digests.clone(),
            available_artifacts: artifacts,
            compatibility: Compatibility::Compatible,
            unresolved_effects: Reconciliation::Clear,
            session_custody: SessionCustody::Reconciled,
        };
        let f = ExternalFence {
            scope: m.scope.clone(),
            manifest_digest: m.manifest_digest.clone(),
            old_executor_incarnation: "old-executor".into(),
            new_executor_incarnation: "new-executor".into(),
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
        assert_eq!(p.manifest_digest, m.manifest_digest);
        assert_eq!(p.snapshot_watermark, m.snapshot_watermark);
        assert_eq!(p.proposed_generation, Some(f.new_generation));
        assert_eq!(p.fence_receipt_digest, Some(f.external_receipt_digest));
        assert!(!p.restore_authorized && !p.effects_authorized);
    }

    #[test]
    fn only_supported_snapshot_versions_propose_resume_and_schema_is_hash_bound() {
        for version in [0, 1, 2, 3, u32::MAX] {
            let (mut manifest, mut observation, mut fence, _) = fixture();
            manifest.schema_version = version;
            manifest.manifest_digest = manifest_digest(&manifest).expect("versioned manifest hash");
            observation.manifest_digest = manifest.manifest_digest.clone();
            fence.manifest_digest = manifest.manifest_digest.clone();
            let proposal = plan_recovery(&manifest, &observation, Some(&fence))
                .expect("schema compatibility is a named blocker");
            let supported = matches!(version, 1 | 2);
            assert_eq!(proposal.resume_proposed, supported, "schema {version}");
            assert_eq!(
                proposal.blockers.contains(&Blocker::InvalidManifest(
                    "unsupported schema version".into()
                )),
                !supported,
                "schema {version}"
            );
            assert!(!proposal.restore_authorized() && !proposal.effects_authorized());
        }

        let (mut manifest, observation, fence, _) = fixture();
        manifest.schema_version = 2; // Both versions are supported, but this hash is still v1.
        let proposal = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("changed schema remains bound by the original manifest digest");
        assert!(
            proposal
                .blockers
                .contains(&Blocker::InvalidManifest("manifest digest mismatch".into()))
        );
        assert!(!proposal.resume_proposed);
        assert!(!proposal.restore_authorized() && !proposal.effects_authorized());
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
        let (mut m, mut o, mut f, _s) = fixture();
        let extra = record(digest(4), 8);
        m.artifact_closure.push(extra.clone());
        m.manifest_digest = manifest_digest(&m).expect("order independent fixture hash");
        o.manifest_digest = m.manifest_digest.clone();
        f.manifest_digest = m.manifest_digest.clone();
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
    #[test]
    fn observation_and_fence_are_bound_to_scope_snapshot_and_actual_prior_generation() {
        let (m, o, f, _) = fixture();
        for field in ["scope", "watermark", "snapshot", "backup-generation"] {
            let mut changed = o.clone();
            match field {
                "scope" => {
                    changed.scope.attempt_id = AttemptId::new("other").expect("fixture attempt")
                }
                "watermark" => changed.snapshot_watermark = DecimalU64::new(1),
                "snapshot" => changed.snapshot_digest = digest(7),
                _ => changed.backup_generation = DecimalU64::new(3),
            }
            assert!(
                !plan_recovery(&m, &changed, Some(&f))
                    .expect("mismatch is a blocker")
                    .resume_proposed
            );
        }
        let mut wrong = f.clone();
        wrong.old_generation = DecimalU64::new(4);
        assert!(
            plan_recovery(&m, &o, Some(&wrong))
                .expect("actual executor generation pinned")
                .blockers
                .contains(&Blocker::FenceMismatch)
        );
        wrong = f.clone();
        wrong.new_executor_incarnation = wrong.old_executor_incarnation.clone();
        assert!(
            !plan_recovery(&m, &o, Some(&wrong))
                .expect("incarnation must rotate")
                .resume_proposed
        );
        wrong = f;
        wrong.manifest_digest = digest(8);
        assert!(
            !plan_recovery(&m, &o, Some(&wrong))
                .expect("fence pins backup identity")
                .resume_proposed
        );
    }
    #[test]
    fn complete_bundle_and_snapshot_closure_is_required_even_with_compatible_claim() {
        let (mut m, mut o, mut f, _) = fixture();
        m.active_bundle_digests.push(digest(9));
        m.manifest_digest = manifest_digest(&m).expect("new fixture manifest");
        o.manifest_digest = m.manifest_digest.clone();
        o.observed_bundle_digests = m.active_bundle_digests.clone();
        f.manifest_digest = m.manifest_digest.clone();
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("missing active bundle artifact")
                .blockers
                .contains(&Blocker::MissingBundleClosure)
        );
        let (m, mut o, f, _) = fixture();
        o.observed_bundle_digests.clear();
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("unknown compatibility coverage")
                .blockers
                .contains(&Blocker::UnknownCompatibility)
        );
        let (mut m, mut o, mut f, _) = fixture();
        m.snapshot_size = DecimalU64::new(99);
        m.manifest_digest = manifest_digest(&m).expect("new snapshot pin");
        o.snapshot_size = m.snapshot_size.clone();
        o.manifest_digest = m.manifest_digest.clone();
        f.manifest_digest = m.manifest_digest.clone();
        assert!(
            plan_recovery(&m, &o, Some(&f))
                .expect("snapshot must exist in closure")
                .blockers
                .contains(&Blocker::SnapshotMismatch)
        );
    }
    #[test]
    fn public_hash_helpers_bound_typed_inputs_before_copying_and_never_authorize_restore() {
        let (mut m, o, f, _) = fixture();
        m.scope.store_id = "x".repeat(MAX_JSON_BYTES + 1);
        assert_eq!(manifest_digest(&m), Err(RecoveryError::InputTooLarge));
        assert_eq!(
            plan_recovery(&m, &o, Some(&f)),
            Err(RecoveryError::InputTooLarge)
        );
        let (m, o, f, _) = fixture();
        let proposal = plan_recovery(&m, &o, Some(&f)).expect("bounded consistency only");
        assert!(!proposal.restore_authorized() && !proposal.effects_authorized());
    }
}
