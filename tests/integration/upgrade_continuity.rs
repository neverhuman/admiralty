use admiralty_core::{DomainEvent, SourceIdentity};
use admiralty_runtime::recovery::{
    Blocker, Compatibility, DecimalU64, Reconciliation, SessionCustody, manifest_digest,
    plan_recovery,
};
use admiralty_store::Store;
use admiralty_testkit::clock::{ClockError, FakeClock};
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
    let replayed = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("replaying identical recovery evidence returns a proposal");
    assert_eq!(replayed, proposal, "recovery planning is replay-stable");
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

#[test]
fn tampered_backup_manifest_digest_blocks_upgrade_continuity() {
    let (mut manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    manifest.snapshot_watermark = admiralty_runtime::recovery::DecimalU64::new(10);

    let proposal = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("tampered backup evidence is returned as a blocked proposal");
    assert!(proposal.blockers.iter().any(|blocker| matches!(
        blocker,
        admiralty_runtime::recovery::Blocker::InvalidManifest(_)
    )));
    assert!(!proposal.resume_proposed);
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());
}

#[test]
fn supported_snapshot_versions_keep_fence_session_effect_and_compatibility_blocks() {
    for version in [1, 2] {
        let (mut manifest, mut observation, mut fence) =
            recovery_fixture::valid_recovery_evidence();
        manifest.schema_version = version;
        manifest.manifest_digest = manifest_digest(&manifest).expect("versioned manifest identity");
        observation.manifest_digest = manifest.manifest_digest.clone();
        fence.manifest_digest = manifest.manifest_digest.clone();
        let compatible = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("supported snapshot schema");
        assert!(compatible.resume_proposed, "schema {version}");
        assert!(!compatible.restore_authorized() && !compatible.effects_authorized());

        let mut blocked = vec![(
            plan_recovery(&manifest, &observation, None),
            Blocker::MissingExternalFence,
        )];
        let mut changed_fence = fence.clone();
        changed_fence.new_generation = changed_fence.old_generation.clone();
        blocked.push((
            plan_recovery(&manifest, &observation, Some(&changed_fence)),
            Blocker::StaleExternalGeneration,
        ));
        for fenced in [None, Some(false)] {
            changed_fence = fence.clone();
            changed_fence.old_executor_fenced = fenced;
            blocked.push((
                plan_recovery(&manifest, &observation, Some(&changed_fence)),
                Blocker::OldExecutorNotFenced,
            ));
        }
        let mut changed = observation.clone();
        changed.session_custody = SessionCustody::Unknown;
        blocked.push((
            plan_recovery(&manifest, &changed, Some(&fence)),
            Blocker::UnknownSessionCustody,
        ));
        for (state, blocker) in [
            (Reconciliation::Unknown, Blocker::UnknownRemoteMutation),
            (
                Reconciliation::Unresolved,
                Blocker::UnresolvedRemoteMutation,
            ),
        ] {
            changed = observation.clone();
            changed.unresolved_effects = state;
            blocked.push((plan_recovery(&manifest, &changed, Some(&fence)), blocker));
        }
        for (state, blocker) in [
            (Compatibility::Unknown, Blocker::UnknownCompatibility),
            (
                Compatibility::Incompatible,
                Blocker::IncompatibleActiveBundle,
            ),
        ] {
            changed = observation.clone();
            changed.compatibility = state;
            blocked.push((plan_recovery(&manifest, &changed, Some(&fence)), blocker));
        }
        changed = observation.clone();
        changed.observed_bundle_digests.clear();
        blocked.push((
            plan_recovery(&manifest, &changed, Some(&fence)),
            Blocker::UnknownCompatibility,
        ));
        changed = observation.clone();
        changed.snapshot_watermark = DecimalU64::new(1);
        blocked.push((
            plan_recovery(&manifest, &changed, Some(&fence)),
            Blocker::ObservationMismatch,
        ));
        changed = observation.clone();
        changed.available_artifacts.clear();
        blocked.push((
            plan_recovery(&manifest, &changed, Some(&fence)),
            Blocker::MissingArtifact(manifest.artifact_closure[0].closure_id.clone()),
        ));
        for (result, blocker) in blocked {
            let proposal = result.expect("unsafe evidence returns a named blocker");
            assert!(
                proposal.blockers.contains(&blocker),
                "schema {version}: {blocker:?}"
            );
            assert!(!proposal.resume_proposed);
            assert!(!proposal.restore_authorized() && !proposal.effects_authorized());
        }
    }
}

#[test]
fn backward_clock_skew_keeps_upgrade_clock_state_unchanged() {
    let mut clock = FakeClock::new(50);

    assert_eq!(
        clock.advance_to_ms(49),
        Err(ClockError::Backwards {
            now_ms: 50,
            target_ms: 49,
        })
    );
    assert_eq!(clock.now_ms(), 50);
    assert_eq!(clock.advance_ms(1), Ok(51));
}
