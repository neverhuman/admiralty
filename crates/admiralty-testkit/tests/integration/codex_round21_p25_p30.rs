#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::recovery_fixture;
use admiralty_runtime::recovery::{Blocker, Reconciliation, plan_recovery};
use sha2::{Digest, Sha256};

#[test]
fn codex_round21_p25_p30_hermetic_cross_packet_release_qualification() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    let baseline = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("valid recovery evidence produces a bounded proposal");
    assert!(baseline.resume_proposed);
    assert_eq!(baseline.manifest_digest, manifest.manifest_digest);
    assert!(!baseline.restore_authorized());
    assert!(!baseline.effects_authorized());

    // Stable replay remains tied to the same P25 manifest and P30 fence receipt.
    for _ in 0..96 {
        let replay = plan_recovery(&manifest, &observation, Some(&fence))
            .expect("unchanged evidence replays deterministically");
        assert_eq!(replay, baseline);
    }

    // Missing proof of executor fencing and uncertain external effects fail closed.
    fence.old_executor_fenced = None;
    let unfenced = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("missing fence becomes a non-authorizing blocker");
    assert!(unfenced.blockers.contains(&Blocker::OldExecutorNotFenced));
    assert!(!unfenced.resume_proposed);
    assert!(!unfenced.restore_authorized());
    assert!(!unfenced.effects_authorized());

    fence.old_executor_fenced = Some(true);
    observation.unresolved_effects = Reconciliation::Unknown;
    let uncertain = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("uncertain effects remain fail closed");
    assert!(uncertain.blockers.contains(&Blocker::UnknownRemoteMutation));
    assert!(!uncertain.resume_proposed);
    assert!(!uncertain.restore_authorized());
    assert!(!uncertain.effects_authorized());

    // Hermetic release parity: derive the workspace from Cargo's manifest path,
    // then check committed schema bundle digests without host-specific paths/tools.
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schema_digests = [
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
    for (relative_path, expected_digest) in schema_digests {
        let bytes = std::fs::read(workspace.join(relative_path))
            .expect("release schema is present in the workspace");
        let parsed = serde_json::from_slice::<serde_json::Value>(&bytes)
            .expect("release schema is valid JSON");
        assert!(parsed.is_object());
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected_digest);
    }
}
