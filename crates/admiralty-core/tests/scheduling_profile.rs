use admiralty_core::{ProjectId, RootId, interfaces::DecimalU64, scheduling_profile::*};

fn scope() -> ProfileScope {
    ProfileScope {
        project_id: ProjectId::new("project-a").expect("project"),
        root_id: RootId::new("root-a").expect("root"),
    }
}
fn profile(zone: &str, start: &str, end: &str) -> SchedulingProfile {
    SchedulingProfile {
        schema_version: 1,
        profile_id: "shift-a".into(),
        scope: scope(),
        revision: DecimalU64::new(1),
        allow_window_override: true,
        mode: SchedulingMode::Scheduled {
            timezone: zone.into(),
            windows: vec![WeeklyWindow {
                window_id: "window-a".into(),
                weekdays: vec![1, 2, 3, 4, 5, 6, 7],
                start: start.into(),
                end: end.into(),
            }],
            blocked_start_dates: vec![],
            fold: FoldPolicy::Earlier,
            gap: GapPolicy::SkipWindow,
        },
    }
}
fn ready() -> EnvironmentObservations {
    EnvironmentObservations {
        setup: SetupObservation::Checked {
            exit_code: 0,
            timed_out: false,
            timeout_seconds: 30,
            noninteractive_environment_digest: format!("sha256:{}", "a".repeat(64)),
        },
        memory: Availability::Available,
        disk: Availability::Available,
        quota: Availability::Available,
        decisions: Availability::Available,
    }
}
fn seconds(ts: &str) -> u64 {
    u64::try_from(
        ts.parse::<jiff::Timestamp>()
            .expect("timestamp")
            .as_second(),
    )
    .expect("positive")
}
fn resolved(p: &SchedulingProfile, d: &str) -> ResolvedWindow {
    match resolve_window(p, &scope(), "window-a", d).expect("resolution") {
        WindowResolution::Resolved { window } => *window,
        other => panic!("expected real resolved window, got {other:?}"),
    }
}

#[test]
fn actual_pinned_tzif_has_version_offsets_and_distinct_profile_identity() {
    let p = profile("America/New_York", "09:00:00", "17:00:00");
    let w = resolved(&p, "2026-01-05");
    assert_eq!(w.timezone.iana_version, "2026e");
    assert_eq!(w.timezone.parser, "jiff/0.2.38");
    assert_eq!(w.timezone.bundle_version, "jiff-tzdb/0.1.9");
    assert!(w.timezone.tzif_digest.starts_with("sha256:"));
    assert_eq!(w.start.offset_seconds, -18_000);
    assert_eq!(w.start.unix_seconds.get(), seconds("2026-01-05T14:00:00Z"));
    assert_ne!(
        w.identity_digest().expect("window digest"),
        p.identity_digest(&scope()).expect("profile digest")
    );
}

#[test]
fn real_new_york_spring_gap_skips_without_moving_the_requested_time() {
    let p = profile("America/New_York", "02:30:00", "03:30:00");
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2026-03-08"),
        Ok(WindowResolution::GapSkipped)
    );
    assert!(matches!(
        resolve_window(&p, &scope(), "window-a", "2026-03-09"),
        Ok(WindowResolution::Resolved { .. })
    ));
}

#[test]
fn real_new_york_fold_has_explicit_earlier_later_or_reject() {
    let mut p = profile("America/New_York", "01:15:00", "02:15:00");
    let early = resolved(&p, "2026-11-01");
    assert_eq!(
        early.start.unix_seconds.get(),
        seconds("2026-11-01T05:15:00Z")
    );
    assert_eq!(early.start.choice, EndpointChoice::FoldEarlier);
    if let SchedulingMode::Scheduled { fold, .. } = &mut p.mode {
        *fold = FoldPolicy::Later;
    }
    let late = resolved(&p, "2026-11-01");
    assert_eq!(
        late.start.unix_seconds.get(),
        seconds("2026-11-01T06:15:00Z")
    );
    assert_eq!(late.start.choice, EndpointChoice::FoldLater);
    assert_eq!(early.end.unix_seconds, late.end.unix_seconds);
    if let SchedulingMode::Scheduled { fold, .. } = &mut p.mode {
        *fold = FoldPolicy::Reject;
    }
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2026-11-01"),
        Ok(WindowResolution::FoldRejected)
    );
}

#[test]
fn real_lord_howe_fold_is_thirty_minutes_not_an_assumed_hour() {
    let mut p = profile("Australia/Lord_Howe", "01:30:00", "02:30:00");
    let early = resolved(&p, "2026-04-05");
    if let SchedulingMode::Scheduled { fold, .. } = &mut p.mode {
        *fold = FoldPolicy::Later;
    }
    let late = resolved(&p, "2026-04-05");
    assert_eq!(
        early.start.unix_seconds.get(),
        seconds("2026-04-04T14:30:00Z")
    );
    assert_eq!(
        late.start.unix_seconds.get(),
        seconds("2026-04-04T15:00:00Z")
    );
    assert_eq!(
        late.start.unix_seconds.get() - early.start.unix_seconds.get(),
        1800
    );
}

#[test]
fn real_apia_skipped_date_is_a_gap_not_a_fabricated_day() {
    let p = profile("Pacific/Apia", "12:00:00", "13:00:00");
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2011-12-30"),
        Ok(WindowResolution::GapSkipped)
    );
}

#[test]
fn overnight_shift_uses_start_weekday_and_start_holiday_across_utc_date() {
    let mut p = profile("Asia/Tokyo", "23:00:00", "02:00:00");
    if let SchedulingMode::Scheduled {
        windows,
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        windows[0].weekdays = vec![1]; // Monday start, even after local midnight.
        blocked_start_dates.push("2026-01-06".into()); // Tuesday end date is irrelevant.
    }
    let at = seconds("2026-01-05T16:00:00Z"); // Tuesday 01:00 in Tokyo.
    let d = evaluate_eligibility(&p, &scope(), Some(at), &ready(), None).expect("eligible");
    assert!(d.eligible);
    let w = d.window.expect("overnight window");
    assert_eq!(w.shift_start_date, "2026-01-05");
    assert_eq!(w.start.unix_seconds.get(), seconds("2026-01-05T14:00:00Z"));
    assert_eq!(w.end.unix_seconds.get(), seconds("2026-01-05T17:00:00Z"));
    if let SchedulingMode::Scheduled {
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        blocked_start_dates.push("2026-01-05".into());
    }
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2026-01-05"),
        Ok(WindowResolution::BlockedStartDate)
    );
}

#[test]
fn current_window_is_half_open_and_restart_does_not_extend_it() {
    let p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let w = resolved(&p, "2026-01-05");
    let h = w.identity_digest().expect("digest");
    assert!(
        evaluate_eligibility(
            &p,
            &scope(),
            Some(w.start.unix_seconds.get()),
            &ready(),
            None
        )
        .expect("start")
        .eligible
    );
    assert!(
        !evaluate_eligibility(&p, &scope(), Some(w.end.unix_seconds.get()), &ready(), None)
            .expect("end")
            .eligible
    );
    let restart = evaluate_recorded_eligibility(
        &p,
        &scope(),
        &w,
        &h,
        Some(w.end.unix_seconds.get() + 1),
        &ready(),
        None,
    )
    .expect("restart");
    assert!(!restart.eligible);
    assert!(restart.reasons.contains(&EligibilityReason::WaitingWindow));
}

#[test]
fn recorded_replay_uses_bound_utc_even_when_metadata_names_an_old_bundle() {
    let p = profile("America/New_York", "09:00:00", "10:00:00");
    let mut historical = resolved(&p, "2026-01-05");
    // Model a separately retained historical record, not a new tzdb resolution.
    historical.timezone.parser = "historical-parser".into();
    historical.timezone.bundle_version = "historical-bundle".into();
    historical.timezone.iana_version = "historical-iana".into();
    historical.timezone.tzif_digest = format!("sha256:{}", "b".repeat(64));
    let retained = historical.identity_digest().expect("retained digest");
    let now = historical.start.unix_seconds.get() + 1;
    let replay = evaluate_recorded_eligibility(
        &p,
        &scope(),
        &historical,
        &retained,
        Some(now),
        &ready(),
        None,
    )
    .expect("historical replay");
    assert!(replay.eligible);
    assert_eq!(replay.window.expect("record").timezone, historical.timezone);
    assert!(!replay.execution_granted);
}

#[test]
fn recorded_data_needs_independent_digest_and_consistent_arithmetic() {
    let p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let mut w = resolved(&p, "2026-01-05");
    let retained = w.identity_digest().expect("digest");
    w.start.offset_seconds = 3600;
    assert_eq!(
        validate_recorded_window(&p, &scope(), &w, &retained),
        Err(SchedulingProfileError::Binding)
    );
    let rewritten = w.identity_digest().expect("rewritten digest");
    assert_eq!(
        validate_recorded_window(&p, &scope(), &w, &rewritten),
        Err(SchedulingProfileError::Binding)
    );
}

#[test]
fn overlap_choice_is_lexical_identity_not_input_order_or_priority() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    if let SchedulingMode::Scheduled { windows, .. } = &mut p.mode {
        let mut other = windows[0].clone();
        other.window_id = "aaa".into();
        windows.push(other);
    }
    let d = evaluate_eligibility(
        &p,
        &scope(),
        Some(seconds("2026-01-05T09:30:00Z")),
        &ready(),
        None,
    )
    .expect("decision");
    assert_eq!(d.window.expect("chosen").window_id, "aaa");
}

#[test]
fn setup_resources_quota_and_decisions_are_separate_waits_never_authority() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    p.mode = SchedulingMode::Immediate;
    let env = EnvironmentObservations {
        setup: SetupObservation::Checked {
            exit_code: 12,
            timed_out: false,
            timeout_seconds: 30,
            noninteractive_environment_digest: String::new(),
        },
        memory: Availability::Waiting,
        disk: Availability::Unknown,
        quota: Availability::Waiting,
        decisions: Availability::Unknown,
    };
    let d = evaluate_eligibility(&p, &scope(), Some(100), &env, None).expect("wait");
    assert!(d.window_eligible);
    assert!(!d.eligible && !d.execution_granted);
    assert_eq!(
        d.reasons,
        vec![
            EligibilityReason::NoninteractiveEnvironmentUnknown,
            EligibilityReason::SetupFailed,
            EligibilityReason::MemoryPressure,
            EligibilityReason::DiskUnknown,
            EligibilityReason::QuotaWait,
            EligibilityReason::DecisionUnknown
        ]
    );
}

#[test]
fn absent_toolchain_timeout_and_unknown_setup_remain_distinct() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    p.mode = SchedulingMode::Immediate;
    for (setup, reason) in [
        (SetupObservation::Unknown, EligibilityReason::SetupUnknown),
        (
            SetupObservation::ToolchainAbsent,
            EligibilityReason::ToolchainAbsent,
        ),
        (
            SetupObservation::Checked {
                exit_code: 0,
                timed_out: true,
                timeout_seconds: 30,
                noninteractive_environment_digest: format!("sha256:{}", "a".repeat(64)),
            },
            EligibilityReason::SetupTimeout,
        ),
    ] {
        let mut env = ready();
        env.setup = setup;
        assert_eq!(
            evaluate_eligibility(&p, &scope(), Some(100), &env, None)
                .expect("wait")
                .reasons,
            vec![reason]
        );
    }
}

#[test]
fn override_is_explicit_expiring_and_cannot_clear_other_waits() {
    let p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let at = seconds("2026-01-05T12:00:00Z");
    let grant = ExternallyGrantedOverride::from_trusted_binding(
        scope(),
        p.identity_digest(&scope()).expect("profile"),
        "authority-1".into(),
        "authorized one-time shift".into(),
        at,
        at + 60,
    )
    .expect("trusted caller binding");
    let mut env = ready();
    env.quota = Availability::Waiting;
    let d = evaluate_eligibility(&p, &scope(), Some(at), &env, Some(&grant)).expect("override");
    assert!(d.window_eligible && !d.eligible && !d.execution_granted);
    assert_eq!(d.reasons, vec![EligibilityReason::QuotaWait]);
    assert_eq!(
        d.override_provenance.expect("provenance").authority_ref,
        "authority-1"
    );
    let expired =
        evaluate_eligibility(&p, &scope(), Some(at + 60), &ready(), Some(&grant)).expect("expiry");
    assert!(!expired.window_eligible && !expired.eligible);
    assert!(
        expired
            .reasons
            .contains(&EligibilityReason::OverrideExpired)
    );
    let future = evaluate_eligibility(&p, &scope(), Some(at - 1), &ready(), Some(&grant))
        .expect("not yet valid");
    assert!(
        future
            .reasons
            .contains(&EligibilityReason::OverrideNotYetValid)
    );
}

#[test]
fn unknown_clock_and_untrusted_scope_or_changed_profile_fail_closed() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    p.mode = SchedulingMode::Immediate;
    let unknown = evaluate_eligibility(&p, &scope(), None, &ready(), None).expect("unknown");
    assert!(!unknown.window_eligible && !unknown.eligible);
    assert_eq!(unknown.reasons, vec![EligibilityReason::ClockUnknown]);
    let foreign = ProfileScope {
        project_id: ProjectId::new("foreign").expect("project"),
        root_id: scope().root_id,
    };
    assert_eq!(
        p.validate_structure(&foreign),
        Err(SchedulingProfileError::ScopeMismatch)
    );
    let grant = ExternallyGrantedOverride::from_trusted_binding(
        scope(),
        p.identity_digest(&scope()).expect("digest"),
        "authority".into(),
        "reason".into(),
        1,
        100,
    )
    .expect("binding");
    p.revision = DecimalU64::new(2);
    assert_eq!(
        evaluate_eligibility(&p, &scope(), Some(2), &ready(), Some(&grant)),
        Err(SchedulingProfileError::Binding)
    );
}

#[test]
fn timezone_never_falls_back_to_host_or_folds_capitalization() {
    for zone in [
        "America/Not_A_Zone",
        "america/new_york",
        "",
        "../etc/localtime",
    ] {
        let p = profile(zone, "09:00:00", "10:00:00");
        assert!(resolve_window(&p, &scope(), "window-a", "2026-01-05").is_err());
    }
}

#[test]
fn closed_wire_and_typed_callers_enforce_bounds_before_hashing() {
    let p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let mut json = serde_json::to_value(&p).expect("JSON");
    json["unknown"] = true.into();
    assert!(parse_scheduling_profile(&serde_json::to_vec(&json).expect("JSON")).is_err());
    json.as_object_mut().expect("object").remove("unknown");
    json["revision"] = 1.into();
    assert!(parse_scheduling_profile(&serde_json::to_vec(&json).expect("JSON")).is_err());
    assert_eq!(
        parse_scheduling_profile(&vec![b' '; MAX_PROFILE_BYTES + 1]),
        Err(SchedulingProfileError::Limit)
    );
    let mut huge = p.clone();
    huge.profile_id = "a".repeat(MAX_PROFILE_BYTES);
    assert_eq!(
        huge.identity_digest(&scope()),
        Err(SchedulingProfileError::Limit)
    );
    assert!(evaluate_eligibility(&p, &scope(), Some(u64::MAX), &ready(), None).is_err());
}

#[test]
fn invalid_calendar_counts_days_times_and_override_bounds_are_rejected() {
    for (start, end) in [
        ("24:00:00", "01:00:00"),
        ("23:59:60", "01:00:00"),
        ("09:00:00", "09:00:00"),
    ] {
        assert!(
            profile("Etc/UTC", start, end)
                .validate_structure(&scope())
                .is_err()
        );
    }
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    if let SchedulingMode::Scheduled {
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        blocked_start_dates.push("2026-02-30".into());
    }
    assert!(p.validate_structure(&scope()).is_err());
    if let SchedulingMode::Scheduled {
        windows,
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        blocked_start_dates.clear();
        windows[0].weekdays = vec![1, 1];
    }
    assert!(p.validate_structure(&scope()).is_err());
    assert!(
        ExternallyGrantedOverride::from_trusted_binding(
            scope(),
            format!("sha256:{}", "a".repeat(64)),
            "authority".into(),
            "reason".into(),
            1,
            MAX_OVERRIDE_SECONDS + 2
        )
        .is_err()
    );
}

#[test]
fn valid_epoch_window_is_not_blocked_by_negative_epoch_neighbor_window() {
    let mut p = profile("Asia/Tokyo", "00:00:00", "01:00:00");
    if let SchedulingMode::Scheduled { windows, .. } = &mut p.mode {
        windows.push(WeeklyWindow {
            window_id: "valid-epoch".into(),
            weekdays: vec![1, 2, 3, 4, 5, 6, 7],
            start: "09:00:00".into(),
            end: "10:00:00".into(),
        });
    }
    let d = evaluate_eligibility(&p, &scope(), Some(1800), &ready(), None)
        .expect("valid epoch observation");
    assert!(d.eligible);
    assert_eq!(d.window.expect("valid window").window_id, "valid-epoch");
}

#[test]
fn valid_last_year_window_is_not_blocked_by_out_of_range_future_date() {
    let p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let d = evaluate_eligibility(
        &p,
        &scope(),
        Some(seconds("9999-12-30T09:30:00Z")),
        &ready(),
        None,
    )
    .expect("valid last-year observation");
    assert!(d.eligible);
    assert_eq!(
        d.window.expect("valid window").shift_start_date,
        "9999-12-30"
    );
}

#[test]
fn current_unknown_zone_is_rejected_even_for_blocked_or_inapplicable_dates() {
    let mut p = profile("America/Not_A_Zone", "09:00:00", "10:00:00");
    if let SchedulingMode::Scheduled {
        windows,
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        windows[0].weekdays = vec![2];
        blocked_start_dates.push("2026-01-06".into());
    }
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2026-01-05"),
        Err(SchedulingProfileError::Timezone)
    );
    assert_eq!(
        resolve_window(&p, &scope(), "window-a", "2026-01-06"),
        Err(SchedulingProfileError::Timezone)
    );
}

#[test]
fn recorded_removed_zone_replays_while_current_resolution_rejects_it() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    let mut archived = resolved(&p, "2026-01-05");
    // A fixture representing an independently bound historical record. This
    // asserts no current lookup, not that this name was an actual IANA zone.
    if let SchedulingMode::Scheduled { timezone, .. } = &mut p.mode {
        *timezone = "Historical/Removed_Zone".into();
    }
    archived.profile_digest = p
        .identity_digest(&scope())
        .expect("structural historical identity");
    archived.timezone.timezone = "Historical/Removed_Zone".into();
    let expected = archived
        .identity_digest()
        .expect("retained independent digest");
    assert_eq!(
        p.validate_current(&scope()),
        Err(SchedulingProfileError::Timezone)
    );
    assert!(
        evaluate_recorded_eligibility(
            &p,
            &scope(),
            &archived,
            &expected,
            Some(archived.start.unix_seconds.get() + 1),
            &ready(),
            None
        )
        .expect("historical replay without lookup")
        .eligible
    );
}

#[test]
fn invalid_profile_range_is_not_swallowed_as_an_irrelevant_candidate() {
    let mut p = profile("Etc/UTC", "09:00:00", "10:00:00");
    if let SchedulingMode::Scheduled {
        blocked_start_dates,
        ..
    } = &mut p.mode
    {
        blocked_start_dates.push("1969-12-31".into());
    }
    assert_eq!(
        evaluate_eligibility(
            &p,
            &scope(),
            Some(seconds("2026-01-05T09:30:00Z")),
            &ready(),
            None
        ),
        Err(SchedulingProfileError::Range)
    );
}
