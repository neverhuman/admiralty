use admiralty_core::economics::{DecimalU128, ObservedAmount};
use admiralty_runtime::session::{
    LaunchSpec, MAX_LAUNCH_ARG_BYTES, MAX_LAUNCH_ARGS, validate_launch_envelope,
};
use admiralty_testkit::clock::{ClockError, FakeClock};
use std::path::PathBuf;

#[test]
fn p26_p31_round11_item4_final_validation_gate() {
    // P26 accounting round trips the largest supported counter exactly and
    // keeps unavailable usage distinct from a measured zero.
    let max = DecimalU128::parse("340282366920938463463374607431768211455")
        .expect("maximum canonical u128");
    let wire = serde_json::to_string(&max).expect("serialize decimal counter");
    assert_eq!(wire, "\"340282366920938463463374607431768211455\"");
    assert_eq!(
        serde_json::from_str::<DecimalU128>(&wire).expect("round trip"),
        max
    );
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
    let observed_zero = ObservedAmount::Known {
        amount: DecimalU128(0),
    };
    let unavailable = ObservedAmount::Unknown {
        reason: "provider did not report usage".into(),
    };
    assert_ne!(observed_zero, unavailable);
    assert_eq!(
        serde_json::to_value(&unavailable).expect("unknown usage wire value")["kind"],
        "unknown"
    );

    // P31 validates the complete launch envelope at both accepted boundaries
    // and rejects malformed identity, paths, NUL bytes, and over-limit data.
    let launch = LaunchSpec {
        run_id: "round11-final-gate".into(),
        program: PathBuf::from("/usr/bin/true"),
        args: vec!["--version".into()],
        cwd: PathBuf::from("/workspace/project"),
        spool_dir: PathBuf::from("/workspace/project/.spool"),
    };
    assert!(validate_launch_envelope(&launch).is_ok());
    for mutate in [
        0_u8, // unsafe run ID
        1,    // relative program
        2,    // argument containing NUL
        3,    // too many arguments
        4,    // argument byte limit exceeded
    ] {
        let mut candidate = launch.clone();
        match mutate {
            0 => candidate.run_id = "../escape".into(),
            1 => candidate.program = PathBuf::from("true"),
            2 => candidate.args = vec!["bad\0arg".into()],
            3 => candidate.args = vec![String::new(); MAX_LAUNCH_ARGS + 1],
            _ => candidate.args = vec!["x".repeat(MAX_LAUNCH_ARG_BYTES + 1)],
        }
        assert!(validate_launch_envelope(&candidate).is_err());
    }
    let mut at_arg_limit = launch;
    at_arg_limit.args = vec!["x".repeat(MAX_LAUNCH_ARG_BYTES)];
    assert!(validate_launch_envelope(&at_arg_limit).is_ok());

    // Clock faults do not change the accepted timestamp; deadline equality is
    // deterministic, so a cutover cannot drift based on wall-clock scheduling.
    let mut clock = FakeClock::new(u64::MAX - 1);
    let accepted_time = clock.now_ms();
    assert!(matches!(
        clock.advance_ms(2),
        Err(ClockError::Overflow { .. })
    ));
    assert_eq!(clock.now_ms(), accepted_time);
    assert!(matches!(
        clock.advance_to_ms(u64::MAX - 2),
        Err(ClockError::Backwards { .. })
    ));
    assert_eq!(clock.now_ms(), accepted_time);
    let deadline = FakeClock::new(100)
        .deadline_after_ms(25)
        .expect("bounded deterministic deadline");
    assert!(!deadline.is_due(124));
    assert!(deadline.is_due(125));
}
