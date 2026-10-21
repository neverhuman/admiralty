#[test]
fn p25_p30_round13_wo_timezone_schedule_intake() {
    use admiralty_core::workplans::preview_workplan;

    // Work-order intake is an offline preview: deterministic identity/order,
    // with unknown scheduling fields rejected instead of silently ignored.
    let valid = br#"{"schema_version":1,"plan_id":"round13-plan","revision":1,"project_id":"project-p25","tasks":[]}"#;
    let first = preview_workplan(valid).expect("minimal work-order intake is valid");
    let replay = preview_workplan(valid).expect("replaying identical intake is deterministic");
    assert_eq!(first, replay);
    assert!(first.task_order.is_empty());
    assert!(first.task_digests.is_empty());
    assert!(first.source_digest.starts_with("sha256:"));

    let timezone_schedule = br#"{"schema_version":1,"plan_id":"round13-plan","revision":1,"project_id":"project-p25","tasks":[],"timezone":"America/New_York","schedule":{"local_time":"09:30"}}"#;
    assert!(
        preview_workplan(timezone_schedule).is_err(),
        "unrecognized timezone/schedule data must fail closed"
    );

    // P30 release evidence remains digest checked and does not grant acceptance
    // or execution authority merely because package self-tests pass.
    let report = admiralty_qualify::source_package::self_test();
    assert!(
        report.failures.is_empty(),
        "P30 release-evidence self-test failed:\n{}",
        report.failures.join("\n")
    );
    assert!(
        report.cases_run == 10,
        "P30 release-evidence self-test did not run all ten package checks"
    );
}
