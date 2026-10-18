#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::recovery_fixture;
use admiralty_runtime::recovery::{Blocker, plan_recovery};
use sha2::{Digest, Sha256};

#[test]
fn codex_round18_p25_p30_release_bundle_schema_parity() {
    let (manifest, observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    let baseline = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("complete P25 release evidence yields a resume proposal");
    assert!(baseline.resume_proposed);
    assert_eq!(baseline.manifest_digest, manifest.manifest_digest);
    assert_eq!(
        baseline.fence_receipt_digest,
        Some(fence.external_receipt_digest.clone())
    );
    assert!(!baseline.restore_authorized());
    assert!(!baseline.effects_authorized());

    // Replay must preserve the same cross-packet proposal over multiple passes.
    for _ in 0..64 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("identical recovery evidence replays");
        assert_eq!(replay, baseline);
    }

    // Failover remains closed if the old executor is not positively fenced.
    fence.old_executor_fenced = None;
    let blocked = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("missing fence is represented as a fail-closed proposal");
    assert!(blocked.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!blocked.resume_proposed);
    assert!(!blocked.restore_authorized());
    assert!(!blocked.effects_authorized());

    // P30 release bundle schema parity is pinned to the committed generated files.
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let expected = [
        (
            "schemas/task-contract.schema.json",
            "e092c83e2755e01574391921cbe5830d27a38ed8dfe6013eac337c83f40bcb38",
        ),
        (
            "schemas/coordination-message.schema.json",
            "d256d8affcf471048800130daba6a42651995c7875051ecb0ad0999fb7a79d26",
        ),
        (
            "schemas/domain-event.schema.json",
            "2c3884e9f6fff328a22133fde98d94143cfd753e9a8b88c6047917283071cfc7",
        ),
    ];
    for (relative_path, expected_digest) in expected {
        let bytes =
            std::fs::read(workspace.join(relative_path)).expect("release bundle schema exists");
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
        let actual_digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            actual_digest, expected_digest,
            "schema drift in {relative_path}"
        );
    }

    let output = std::process::Command::new("python3")
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(workspace)
        .output()
        .expect("P30 release bundle smoke starts");
    assert!(
        output.status.success(),
        "P30 release bundle smoke failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Ran 10 tests"));
}
