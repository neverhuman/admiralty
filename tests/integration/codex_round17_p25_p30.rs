use admiralty_runtime::recovery::{Blocker, plan_recovery};

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn p25_p30_round17_post_item5_release_audit_cross_packet_invariants() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();

    // Cross-packet binding: recovery output carries the exact backup and fence evidence.
    let accepted = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("consistent P25/P30 evidence supports a resume proposal");
    assert!(accepted.resume_proposed);
    assert_eq!(accepted.scope, manifest.scope);
    assert_eq!(accepted.manifest_digest, manifest.manifest_digest);
    assert_eq!(
        accepted.fence_receipt_digest,
        Some(fence.external_receipt_digest.clone())
    );
    assert!(!accepted.restore_authorized());
    assert!(!accepted.effects_authorized());

    // Receipt replay stays idempotent across repeated release-audit passes.
    for _ in 0..64 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("replaying unchanged evidence returns a valid proposal");
        assert_eq!(replay, accepted);
    }

    // Cross-packet manifest drift is a blocker, and failover authority stays closed.
    observation.manifest_digest = recovery_fixture::digest(9);
    let drifted = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("manifest drift becomes a fail-closed proposal");
    assert!(drifted.blockers.contains(&Blocker::ObservationMismatch));
    assert!(!drifted.resume_proposed);
    assert!(!drifted.restore_authorized());
    assert!(!drifted.effects_authorized());

    // Executor cutover cannot proceed without positive fencing evidence.
    observation
        .manifest_digest
        .clone_from(&manifest.manifest_digest);
    fence.old_executor_fenced = None;
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("missing old-executor fence blocks cutover");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    // P30 release audit verifies deterministic package-manifest and tamper checks offline.
    let report = admiralty_qualify::source_package::self_test();
    assert!(
        report.failures.is_empty(),
        "P30 release audit failed:\n{}",
        report.failures.join("\n")
    );
    assert!(
        report.cases_run == 10,
        "P30 release audit did not run all ten package checks"
    );
}
