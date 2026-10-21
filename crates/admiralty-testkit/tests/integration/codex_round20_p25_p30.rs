#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::recovery_fixture;
use admiralty_runtime::recovery::{Blocker, Reconciliation, plan_recovery};

#[test]
fn codex_round20_p25_p30_post_pr57_release_qualification() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();

    // P25 recovery output is stable across a longer replay sequence and keeps
    // restore/effect authority disabled until an executor owns that authority.
    let baseline = plan_recovery(&manifest, &observation, Some(&fence)).expect("valid evidence");
    assert!(baseline.resume_proposed);
    assert!(!baseline.restore_authorized());
    assert!(!baseline.effects_authorized());
    for _ in 0..128 {
        let replay =
            plan_recovery(&manifest, &observation, Some(&fence)).expect("replay remains valid");
        assert_eq!(replay, baseline);
    }

    // Cross the failover boundary only with a positive executor fence.
    fence.old_executor_fenced = Some(false);
    let unfenced =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("negative fence is a blocker");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let ambiguous = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("ambiguous effects fail closed");
    assert!(ambiguous.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!ambiguous.resume_proposed);
    assert!(!ambiguous.restore_authorized());
    assert!(!ambiguous.effects_authorized());

    // P30 release smoke keeps package integrity checks deterministic and offline.
    let report = admiralty_qualify::source_package::self_test();
    assert!(
        report.failures.is_empty(),
        "P30 package self-test failed:\n{}",
        report.failures.join("\n")
    );
    assert!(
        report.cases_run == 10,
        "P30 package self-test did not run all ten package checks"
    );
}
