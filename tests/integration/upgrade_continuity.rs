use admiralty_core::{DomainEvent, SourceIdentity};
use admiralty_runtime::recovery::{Compatibility, plan_recovery};
use admiralty_store::Store;
use admiralty_testkit::task_fixture;

#[path = "support/recovery.rs"]
mod recovery_fixture;

fn private_dir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix("admiralty-upgrade-continuity-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .expect("private continuity fixture directory")
}

#[test]
fn backup_reopens_with_identical_snapshot_and_recovery_proposal_keeps_authority_disabled() {
    let directory = private_dir();
    let database = directory.path().join("live.sqlite");
    let backup = directory.path().join("backup.sqlite");
    let store = Store::open(&database).expect("open source store");
    store
        .append(
            SourceIdentity {
                producer_id: "upgrade-fixture".into(),
                incarnation: "boot-one".into(),
                sequence: "1".into(),
            },
            DomainEvent::TaskSubmitted {
                contract: task_fixture("continuity-task"),
            },
            "2026-10-18T00:00:00Z".into(),
        )
        .expect("append durable task event");
    let before = store.snapshot().expect("source snapshot");
    let backed_up = store.backup(&backup).expect("consistent online backup");
    assert_eq!(backed_up, before);
    drop(store);

    let reopened = Store::open(&backup).expect("reopen supported backup");
    assert_eq!(reopened.snapshot().expect("reopened snapshot"), before);

    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let proposal = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("compatible upgrade evidence proposes continuity");
    assert!(proposal.resume_proposed);
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());
}

#[test]
fn incompatible_active_bundle_blocks_upgrade_continuity() {
    let (manifest, mut observation, fence) = recovery_fixture::valid_recovery_evidence();
    observation.compatibility = Compatibility::Incompatible;
    let proposal = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("incompatibility is represented as a blocker");
    assert!(!proposal.resume_proposed);
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());
}
