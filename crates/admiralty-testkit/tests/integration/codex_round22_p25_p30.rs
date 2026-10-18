#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::recovery_fixture;
use admiralty_runtime::recovery::{Blocker, Reconciliation, plan_recovery};
use std::process::Command;

#[test]
fn codex_round22_p25_p30_canary_smoke_release_package_qualification() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    let baseline = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete recovery evidence produces a deterministic proposal");
    assert!(baseline.resume_proposed);
    assert!(!baseline.restore_authorized());
    assert!(!baseline.effects_authorized());

    // A repeated replay cannot change the proposed recovery or grant authority.
    for _ in 0..128 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("identical recovery evidence remains replayable");
        assert_eq!(replay, baseline);
        assert!(!replay.restore_authorized());
        assert!(!replay.effects_authorized());
    }

    // Failover is denied without a positive fence and with uncertain effects.
    fence.old_executor_fenced = Some(false);
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("unfenced executor is represented as a blocker");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let uncertain = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("uncertain remote effects fail closed");
    assert!(uncertain.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!uncertain.resume_proposed);
    assert!(!uncertain.restore_authorized());
    assert!(!uncertain.effects_authorized());

    // Both local release qualification entry points run their bounded offline self-tests.
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for script in [
        "scripts/release/package.py",
        "scripts/check/canary-smoke.py",
    ] {
        let output = Command::new("python3")
            .args([script, "--self-test"])
            .current_dir(&workspace)
            .output()
            .expect("release qualification self-test starts");
        assert!(
            output.status.success(),
            "{script} self-test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("OK"),
            "{script} self-test did not report a successful unittest run"
        );
    }
}
