use admiralty_core::economics::{DecimalU128, ObservedAmount};
use admiralty_runtime::session::{LaunchSpec, MAX_LAUNCH_ARGS, validate_launch_envelope};
use admiralty_testkit::clock::{ClockError, FakeClock};
use std::path::PathBuf;

#[test]
fn p26_p31_round10_item3_workspace_gate_audit() {
    // P26: accounting keeps exact large counters and distinguishes missing usage
    // from a measured zero at the workspace gate.
    let largest = DecimalU128::parse("340282366920938463463374607431768211455")
        .expect("u128 maximum is exact");
    assert_eq!(largest.as_u128(), u128::MAX);
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
    assert!(DecimalU128::parse("01").is_err());
    let zero = ObservedAmount::Known {
        amount: DecimalU128(0),
    };
    let unknown = ObservedAmount::Unknown {
        reason: "provider usage unavailable".into(),
    };
    assert_ne!(zero, unknown);
    let unknown_json = serde_json::to_value(&unknown).expect("serialize unknown usage");
    assert_eq!(unknown_json["kind"], "unknown");

    // P31: the launch envelope admits the reviewed fixture and rejects edge cases.
    let valid = LaunchSpec {
        run_id: "round10-gate".into(),
        program: PathBuf::from("/usr/bin/true"),
        args: vec!["--version".into()],
        cwd: PathBuf::from("/workspace/project"),
        spool_dir: PathBuf::from("/workspace/project/.spool"),
    };
    assert!(validate_launch_envelope(&valid).is_ok());
    let mut invalid = valid.clone();
    invalid.run_id = "../escape".into();
    assert!(validate_launch_envelope(&invalid).is_err());
    let mut boundary = valid.clone();
    boundary.args = vec![String::new(); MAX_LAUNCH_ARGS];
    assert!(validate_launch_envelope(&boundary).is_ok());
    boundary.args.push(String::new());
    assert!(validate_launch_envelope(&boundary).is_err());

    // Logical cutover time is deterministic: rejected skew/overflow leaves the
    // gate clock unchanged, while deadline equality is due.
    let mut clock = FakeClock::new(u64::MAX - 1);
    let before = clock.clone();
    assert_eq!(
        clock.advance_ms(2),
        Err(ClockError::Overflow {
            now_ms: u64::MAX - 1,
            delta_ms: 2,
        })
    );
    assert_eq!(clock, before);
    assert_eq!(
        clock.advance_to_ms(u64::MAX - 2),
        Err(ClockError::Backwards {
            now_ms: u64::MAX - 1,
            target_ms: u64::MAX - 2,
        })
    );
    assert_eq!(clock, before);
    let deadline = FakeClock::new(40)
        .deadline_after_ms(5)
        .expect("fixed deadline");
    assert!(!deadline.is_due(44));
    assert!(deadline.is_due(45));
}
