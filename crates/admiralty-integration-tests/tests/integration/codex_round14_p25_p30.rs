use admiralty_runtime::recovery::{Blocker, Reconciliation, plan_recovery};

#[path = "../../../../tests/integration/support/recovery.rs"]
mod recovery_fixture;

#[test]
fn codex_round14_p25_p30() {
    // Replaying the same recovery evidence must produce an identical proposal.
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    let first = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete recovery evidence is valid");
    let replay = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("same recovery evidence replays deterministically");
    assert_eq!(first, replay);
    assert!(first.resume_proposed);
    assert!(!first.restore_authorized());
    assert!(!first.effects_authorized());

    // Failover cannot resume while the previous executor is not fenced.
    fence.old_executor_fenced = Some(false);
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("unfenced failover is a blocked proposal");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    // Unknown remote effects fail closed even with an otherwise valid fence.
    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let uncertain = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("uncertain remote effects become a blocked proposal");
    assert!(uncertain.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!uncertain.resume_proposed);
    assert!(!uncertain.restore_authorized());
    assert!(!uncertain.effects_authorized());

    // Malformed release evidence is rejected before a recovery proposal exists.
    fence.external_receipt_digest = "sha256:bad".into();
    let invalid_receipt = plan_recovery(&manifest, &observation, Some(&fence))
        .expect_err("invalid receipt digest must fail closed");
    assert!(
        invalid_receipt
            .to_string()
            .contains("invalid external receipt digest")
    );

    // P30 release-package verification is deterministic and offline.
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let output = std::process::Command::new("python3")
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(workspace)
        .output()
        .expect("P30 offline release evidence checks start");
    assert!(
        output.status.success(),
        "P30 release evidence self-test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Ran 10 tests"));
}
