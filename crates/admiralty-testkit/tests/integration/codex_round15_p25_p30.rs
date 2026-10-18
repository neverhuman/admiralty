use super::recovery_fixture;
use admiralty_runtime::recovery::{Blocker, Reconciliation, plan_recovery};

#[test]
fn codex_round15_p25_p30_release_smoke_and_replay_stress() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();

    // Multi-step deterministic replay stress: every replay of the same P25
    // recovery evidence yields the identical proposal and no restore/effect grant.
    let baseline = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete P25 recovery evidence is valid");
    assert!(baseline.resume_proposed);
    assert!(!baseline.restore_authorized());
    assert!(!baseline.effects_authorized());
    for _ in 0..128 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("replayed P25 recovery evidence remains valid");
        assert_eq!(replay, baseline);
    }

    // Failover is denied until fencing is positively observed.
    fence.old_executor_fenced = Some(false);
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("unfenced P25 failover is represented as a blocker");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    // An unknown effect reconciliation blocks the otherwise valid failover.
    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let uncertain =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("unknown effects fail closed");
    assert!(uncertain.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!uncertain.resume_proposed);
    assert!(!uncertain.restore_authorized());
    assert!(!uncertain.effects_authorized());

    // A malformed receipt digest is rejected before a proposal can authorize anything.
    fence.external_receipt_digest = "sha256:invalid".into();
    let invalid_receipt = plan_recovery(&manifest, &observation, Some(&fence))
        .expect_err("malformed release evidence fails closed");
    assert!(
        invalid_receipt
            .to_string()
            .contains("invalid external receipt digest")
    );

    // P30 release smoke: the deterministic, offline package receipt/integrity suite passes.
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let output = std::process::Command::new("python3")
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(workspace)
        .output()
        .expect("P30 release smoke starts");
    assert!(
        output.status.success(),
        "P30 release smoke failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Ran 10 tests"));
}
