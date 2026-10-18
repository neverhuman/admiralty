use admiralty_core::economics::{DecimalU128, ObservedAmount};
use admiralty_core::verification::{MAX_RECEIPT_BYTES, parse_receipt};
use admiralty_runtime::session::{LaunchSpec, MAX_LAUNCH_ARGS, validate_launch_envelope};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[test]
fn p26_p31_round12_item5_redteam_audit_gate() {
    // P26 receipt fields preserve exact integer identity. Any byte-level
    // alteration changes the integrity digest, and untrusted claims fail closed.
    let amount = ObservedAmount::Known {
        amount: DecimalU128::parse("340282366920938463463374607431768211455")
            .expect("maximum u128 amount"),
    };
    let wire = serde_json::to_vec(&amount).expect("serialize amount receipt field");
    let digest = Sha256::digest(&wire);
    let mut tampered = wire.clone();
    let digit = tampered
        .iter()
        .position(|byte| *byte == b'3')
        .expect("decimal digit");
    tampered[digit] = b'x';
    assert_ne!(Sha256::digest(&tampered).as_slice(), digest.as_slice());
    assert!(serde_json::from_slice::<ObservedAmount>(&tampered).is_err());
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
    assert_ne!(
        amount,
        ObservedAmount::Unknown {
            reason: "provider omitted usage".into()
        },
        "missing usage must never be treated as measured zero"
    );

    // Receipt parser enforces bounded, closed schemas. Forged trust flags,
    // unknown fields, malformed encoding, and oversize inputs are rejected.
    let forged = br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"v","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(parse_receipt(forged).is_err());
    assert!(parse_receipt(br#"{"unknown":true}"#).is_err());
    assert!(parse_receipt(&[0xff]).is_err());
    assert!(parse_receipt(&vec![b' '; MAX_RECEIPT_BYTES + 1]).is_err());

    // P31 launch validation rejects hostile paths/arguments and excess count
    // before a process can be constructed or spawned.
    let valid = LaunchSpec {
        run_id: "round12-redteam".into(),
        program: PathBuf::from("/usr/bin/true"),
        args: vec!["--version".into()],
        cwd: PathBuf::from("/workspace/project"),
        spool_dir: PathBuf::from("/workspace/project/.spool"),
    };
    assert!(validate_launch_envelope(&valid).is_ok());
    for candidate in [
        LaunchSpec {
            run_id: "../escape".into(),
            ..valid.clone()
        },
        LaunchSpec {
            program: PathBuf::from("relative"),
            ..valid.clone()
        },
        LaunchSpec {
            args: vec!["bad\0arg".into()],
            ..valid.clone()
        },
        LaunchSpec {
            args: vec![String::new(); MAX_LAUNCH_ARGS + 1],
            ..valid.clone()
        },
    ] {
        assert!(validate_launch_envelope(&candidate).is_err());
    }
}
