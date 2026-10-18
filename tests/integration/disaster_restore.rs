use admiralty_runtime::recovery::{Blocker, DecimalU64, plan_recovery};
use std::{path::PathBuf, process::Command};

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn missing_artifact_or_stale_external_fence_blocks_restore_proposal() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    observation.available_artifacts.clear();
    let missing = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("missing evidence remains a non-authorizing proposal");
    assert!(
        missing
            .blockers
            .iter()
            .any(|b| matches!(b, Blocker::MissingArtifact(_)))
    );
    assert!(!missing.restore_authorized());
    assert!(!missing.effects_authorized());

    observation.available_artifacts = manifest.artifact_closure.clone();
    fence.new_generation = DecimalU64::new(5);
    let stale = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("stale fencing evidence remains a non-authorizing proposal");
    assert!(stale.blockers.contains(&Blocker::StaleExternalGeneration));
    assert!(!stale.restore_authorized());
    assert!(!stale.effects_authorized());
}

#[test]
fn p30_package_builder_runs_its_real_offline_self_test() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let output = Command::new("python3")
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(root)
        .output()
        .expect("python3 and the package builder are available");
    assert!(
        output.status.success(),
        "package builder self-test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains("Ran 10 tests"),
        "unexpected self-test report: {report}"
    );
}
