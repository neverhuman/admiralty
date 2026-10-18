use admiralty_core::economics::{DecimalU128, ObservedAmount};
use admiralty_runtime::session::{LaunchSpec, MAX_LAUNCH_ARGS, validate_launch_envelope};
use std::path::PathBuf;

#[test]
fn p26_economics_counters_preserve_full_u128_and_unknown_costs() {
    let largest = DecimalU128::parse("340282366920938463463374607431768211455")
        .expect("u128 maximum is an exact decimal counter");
    assert_eq!(largest.as_u128(), u128::MAX);
    assert!(DecimalU128::parse("340282366920938463463374607431768211456").is_err());
    assert!(DecimalU128::parse("01").is_err());

    let unknown = ObservedAmount::Unknown {
        reason: "provider did not report cost".into(),
    };
    let json = serde_json::to_value(&unknown).expect("unknown cost serializes explicitly");
    assert_eq!(json["kind"], "unknown");
    assert_eq!(json["reason"], "provider did not report cost");
}

#[test]
fn p31_release_session_rejects_unbounded_or_relative_launch_envelopes() {
    let valid = LaunchSpec {
        run_id: "release-smoke".into(),
        program: PathBuf::from("/usr/bin/true"),
        args: vec!["--version".into()],
        cwd: PathBuf::from("/workspace"),
        spool_dir: PathBuf::from("/workspace/.spool"),
    };
    assert!(validate_launch_envelope(&valid).is_ok());

    let mut relative_program = valid.clone();
    relative_program.program = PathBuf::from("true");
    assert!(validate_launch_envelope(&relative_program).is_err());

    let mut traversing_run_id = valid.clone();
    traversing_run_id.run_id = "../escape".into();
    assert!(validate_launch_envelope(&traversing_run_id).is_err());

    let mut nul_argument = valid.clone();
    nul_argument.args = vec!["--name\0hidden".into()];
    assert!(validate_launch_envelope(&nul_argument).is_err());

    let mut max_arg_count = valid.clone();
    max_arg_count.args = vec![String::new(); MAX_LAUNCH_ARGS];
    assert!(validate_launch_envelope(&max_arg_count).is_ok());

    let mut too_many_args = valid.clone();
    too_many_args.args = vec![String::new(); MAX_LAUNCH_ARGS + 1];
    assert!(validate_launch_envelope(&too_many_args).is_err());

    let mut oversized_args = valid;
    oversized_args.args = vec!["x".repeat(64 * 1024 + 1)];
    assert!(validate_launch_envelope(&oversized_args).is_err());
}
