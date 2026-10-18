use admiralty_core::{AttemptId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::recovery::{
    ArtifactRecord, BackupManifest, Compatibility, DecimalU64, ExternalFence, Reconciliation,
    RecoveryObservation, RecoveryScope, SessionCustody, artifact_closure_id, manifest_digest,
};

pub fn digest(value: u8) -> String {
    format!("sha256:{value:064x}")
}

fn artifact(value: u8, size: u64) -> ArtifactRecord {
    let mut record = ArtifactRecord {
        closure_id: String::new(),
        digest: digest(value),
        size: DecimalU64::new(size),
        verified: true,
    };
    record.closure_id = artifact_closure_id(&record).expect("fixture closure ID is valid");
    record
}

pub fn valid_recovery_evidence() -> (BackupManifest, RecoveryObservation, ExternalFence) {
    let scope = RecoveryScope {
        project_id: ProjectId::new("project-p25").expect("fixture project ID is valid"),
        root_id: RootId::new("root-p25").expect("fixture root ID is valid"),
        task_id: TaskId::new("task-p25").expect("fixture task ID is valid"),
        store_id: "store-p25".into(),
        attempt_id: AttemptId::new("attempt-p25").expect("fixture attempt ID is valid"),
        owner: PrincipalId::new("owner-p25").expect("fixture owner ID is valid"),
        revision: 1,
        contract_digest: digest(4),
        executor_incarnation: "backup-executor".into(),
    };
    let artifacts = vec![artifact(1, 7), artifact(2, 8)];
    let mut manifest = BackupManifest {
        scope: scope.clone(),
        schema_version: 1,
        snapshot_watermark: DecimalU64::new(9_007_199_254_740_993),
        snapshot_digest: digest(1),
        snapshot_size: DecimalU64::new(7),
        recovery_generation: DecimalU64::new(4),
        artifact_closure: artifacts.clone(),
        active_bundle_digests: vec![digest(2)],
        manifest_digest: String::new(),
    };
    manifest.manifest_digest = manifest_digest(&manifest).expect("fixture manifest digest");
    let observation = RecoveryObservation {
        scope: scope.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        snapshot_digest: manifest.snapshot_digest.clone(),
        snapshot_size: manifest.snapshot_size.clone(),
        snapshot_watermark: manifest.snapshot_watermark.clone(),
        backup_generation: manifest.recovery_generation.clone(),
        current_generation: DecimalU64::new(5),
        old_executor_incarnation: "old-executor".into(),
        observed_bundle_digests: manifest.active_bundle_digests.clone(),
        available_artifacts: artifacts,
        compatibility: Compatibility::Compatible,
        unresolved_effects: Reconciliation::Clear,
        session_custody: SessionCustody::Reconciled,
    };
    let fence = ExternalFence {
        scope,
        manifest_digest: manifest.manifest_digest.clone(),
        old_executor_incarnation: "old-executor".into(),
        new_executor_incarnation: "new-executor".into(),
        old_generation: DecimalU64::new(5),
        new_generation: DecimalU64::new(6),
        external_receipt_digest: digest(3),
        old_executor_fenced: Some(true),
    };
    (manifest, observation, fence)
}
