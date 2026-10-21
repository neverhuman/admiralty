use admiralty_runtime::recovery::{Blocker, DecimalU64, Reconciliation, plan_recovery};

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn p25_p30_round16_end_to_end_cutover_fault_containment() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();

    // Cutover may be proposed only for the next generation and exact fence receipt.
    let cutover = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("validated backup and external fence produce a cutover proposal");
    assert!(cutover.resume_proposed);
    assert_eq!(cutover.proposed_generation, Some(DecimalU64::new(6)));
    assert_eq!(
        cutover.fence_receipt_digest,
        Some(fence.external_receipt_digest.clone())
    );
    assert!(!cutover.restore_authorized());
    assert!(!cutover.effects_authorized());

    // Replaying the complete sequence never changes proposal identity.
    for _ in 0..32 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("identical cutover replay remains valid");
        assert_eq!(replay, cutover);
    }

    // Contain failover when the old executor fence is absent or negative.
    fence.old_executor_fenced = Some(false);
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("negative fence observation becomes a blocker");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    // Contain ambiguous remote effects even after executor fencing succeeds.
    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let ambiguous = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("unknown remote effects are a blocked cutover");
    assert!(ambiguous.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!ambiguous.resume_proposed);
    assert!(!ambiguous.restore_authorized());
    assert!(!ambiguous.effects_authorized());

    // Corrupt release evidence is an error, never a permissive fallback.
    fence.external_receipt_digest = "sha256:corrupt".into();
    let invalid = plan_recovery(&manifest, &observation, Some(&fence))
        .expect_err("receipt corruption fails closed");
    assert!(
        invalid
            .to_string()
            .contains("invalid external receipt digest")
    );

    // P30 smoke checks release manifest hashing and tamper rejection offline.
    let report = admiralty_qualify::source_package::self_test();
    assert!(
        report.failures.is_empty(),
        "P30 release smoke failed:\n{}",
        report.failures.join("\n")
    );
    assert!(
        report.cases_run == 10,
        "P30 release smoke did not run all ten package checks"
    );
}
