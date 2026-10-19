//! Local regular-file fixtures only. No service, provider, process or cgroup
//! operation is executed; these do not qualify a live Linux resource backend.
use super::*;
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

fn write(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .expect("private fixture file");
    file.write_all(bytes).expect("write fixture");
}
fn fixture() -> (tempfile::TempDir, ResourceCgroupPin) {
    let dir = tempfile::tempdir().expect("fixture parent");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("private directory");
    for (name, contents) in [
        ("pids.current", "12\n"),
        ("pids.max", "32\n"),
        ("pids.peak", "20\n"),
        ("pids.events", "max 0\n"),
        ("pids.events.local", "max 0\n"),
        ("memory.current", "1000\n"),
        ("memory.max", "536870912\n"),
        ("memory.peak", "2000\n"),
        (
            "memory.events",
            "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\noom_group_kill 0\n",
        ),
        (
            "memory.events.local",
            "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\n",
        ),
        ("cgroup.events", "populated 1\nfrozen 0\n"),
    ] {
        write(&dir.path().join(name), contents.as_bytes());
    }
    let pin = ResourceCgroupPin::open(
        dir.path(),
        content_digest(&"original-observed-owner").expect("digest"),
    )
    .expect("pin original fixture");
    (dir, pin)
}
fn observed(sample: ResourceUsageSample) -> Box<CgroupResourceSnapshot> {
    match sample {
        ResourceUsageSample::Observed { snapshot } => snapshot,
        other => panic!("expected identity-bound sample, got {other:?}"),
    }
}

fn remove_dynamic_files(dir: &Path) {
    for name in [
        "pids.current",
        "pids.peak",
        "pids.events",
        "pids.events.local",
        "memory.current",
        "memory.peak",
        "memory.events",
        "memory.events.local",
        "cgroup.events",
    ] {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove dynamic fixture metric: {error}"),
        }
    }
}

#[test]
fn bounded_original_directory_reads_numeric_values_without_inferring_a_cause() {
    let (_dir, pin) = fixture();
    let value = observed(pin.sample());
    assert_eq!(value.pids_current, ResourceReading::Observed { value: 12 });
    assert_eq!(value.pids_max, ResourceReading::Observed { value: 32 });
    assert_eq!(
        value.memory_max,
        ResourceReading::Observed { value: 536870912 }
    );
    assert_eq!(
        value.pids_events,
        ResourceReading::Observed {
            value: PidsEventCounters { max: Some(0) }
        }
    );
    assert!(matches!(
        value.memory_events_local,
        ResourceReading::Observed {
            value: MemoryEventCounters {
                oom_group_kill: None,
                ..
            }
        }
    ));
    assert_eq!(value.identity, pin.identity);
    let json = serde_json::to_value(value).expect("advisory JSON");
    assert!(json.get("cause").is_none());
    assert!(json.get("execution_granted").is_none());
}

#[test]
fn unsupported_missing_and_unbounded_files_do_not_become_zero() {
    let (dir, pin) = fixture();
    fs::remove_file(dir.path().join("pids.peak")).expect("optional file absent");
    fs::remove_file(dir.path().join("memory.events.local")).expect("optional file absent");
    write(&dir.path().join("pids.max"), b"max\n");
    let value = observed(pin.sample());
    assert_eq!(
        value.pids_peak,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::Missing
        }
    );
    assert_eq!(
        value.memory_events_local,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::Missing
        }
    );
    assert_eq!(
        value.pids_max,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::Unbounded
        }
    );
    assert_eq!(
        value.pids_events,
        ResourceReading::Observed {
            value: PidsEventCounters { max: Some(0) }
        }
    );
}

#[test]
fn malformed_duplicate_overflow_and_extra_event_fields_fail_closed() {
    for text in [
        "max 1\nmax 2\n",
        "max 18446744073709551616\n",
        "max -1\n",
        "max 1 extra\n",
        "max 01\n",
        "",
        "max +1\n",
    ] {
        assert!(
            diagnostic_pids_events(text).is_err(),
            "invalid fixture {text:?}"
        );
    }
    assert_eq!(
        diagnostic_cgroup_events("populated 2\n"),
        Err(ResourceReadFailure::Malformed)
    );
    let future = diagnostic_memory_events("oom_kill 3\nfuture_counter 10\n")
        .expect("bounded future keys retained as unknown fields");
    assert_eq!(future.oom_kill, Some(3));
    assert_eq!(future.oom, None);
    let (dir, pin) = fixture();
    write(&dir.path().join("memory.events"), b"oom 1\noom 2\n");
    assert_eq!(
        observed(pin.sample()).memory_events,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::Malformed
        }
    );
}

#[test]
fn per_file_byte_and_event_count_bounds_precede_projection() {
    let (dir, pin) = fixture();
    write(
        &dir.path().join("pids.events"),
        &vec![b'x'; RESOURCE_DIAGNOSTIC_FILE_LIMIT + 1],
    );
    assert_eq!(
        observed(pin.sample()).pids_events,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::TooLarge
        }
    );
    let text = (0..33)
        .map(|i| format!("{} 0\n", "a".repeat(i + 1)))
        .collect::<String>();
    assert_eq!(diagnostic_events(&text), Err(ResourceReadFailure::TooLarge));
    assert!(diagnostic_events(&format!("{} 1\n", "x".repeat(65))).is_err());
}

#[test]
fn symlink_and_directory_metrics_never_return_other_file_contents() {
    use std::os::unix::fs::symlink;
    let (dir, pin) = fixture();
    let outside = dir.path().join("outside-private-value");
    write(&outside, b"PRIVATE-FIXTURE-NEVER-REPORTED");
    fs::remove_file(dir.path().join("pids.current")).expect("replace scalar");
    symlink(&outside, dir.path().join("pids.current")).expect("fixture symlink");
    fs::remove_file(dir.path().join("memory.current")).expect("replace scalar");
    fs::create_dir(dir.path().join("memory.current")).expect("fixture directory");
    let sample = pin.sample();
    let value = observed(sample.clone());
    assert!(matches!(
        value.pids_current,
        ResourceReading::Unavailable { .. }
    ));
    assert_eq!(
        value.memory_current,
        ResourceReading::Unavailable {
            reason: ResourceReadFailure::NotRegular
        }
    );
    assert!(
        !serde_json::to_string(&sample)
            .expect("sanitized diagnostics")
            .contains("PRIVATE-FIXTURE")
    );
}

#[test]
fn renamed_original_directory_is_retained_and_replacement_is_never_reopened() {
    let parent = tempfile::tempdir().expect("private parent");
    let original = parent.path().join("owned");
    fs::create_dir(&original).expect("original");
    write(&original.join("pids.current"), b"12\n");
    let pin =
        ResourceCgroupPin::open(&original, "original-owner".into()).expect("pin original inode");
    fs::rename(&original, parent.path().join("retained")).expect("move original");
    fs::create_dir(&original).expect("different replacement inode");
    write(&original.join("pids.current"), b"999\n");
    assert_eq!(
        observed(pin.sample()).pids_current,
        ResourceReading::Observed { value: 12 }
    );
    assert_ne!(
        fs::metadata(&original).expect("replacement metadata").ino(),
        pin.identity.inode
    );
}

#[test]
fn identity_failure_and_gone_final_sample_preserve_last_observed_snapshot() {
    let (dir, mut pin) = fixture();
    let mut diagnostics = ResourceUsageDiagnostics::default();
    diagnostics.record(pin.sample(), true, false);
    write(&dir.path().join("pids.events"), b"max 3\n");
    write(&dir.path().join("pids.peak"), b"32\n");
    diagnostics.record(pin.sample(), false, false);
    let latest = diagnostics.latest.clone().expect("last good sample");
    let before = diagnostics.before.clone();
    pin.identity.inode = pin.identity.inode.saturating_add(1);
    let failure = pin.sample();
    assert_eq!(
        failure,
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::IdentityChanged
        }
    );
    diagnostics.record(failure, false, true);
    assert_eq!(diagnostics.sample_attempts, 3);
    assert_eq!(diagnostics.before, before);
    assert_eq!(diagnostics.latest, Some(latest));
    assert_eq!(
        diagnostics.last_sample_failure,
        Some(ResourceReadFailure::IdentityChanged)
    );
    assert!(matches!(
        diagnostics.final_sample,
        Some(ResourceUsageSample::Unavailable { .. })
    ));
}

#[test]
fn history_keeps_last_eight_readable_snapshots_across_unknown_final_observations() {
    let (dir, mut pin) = fixture();
    let mut diagnostics = ResourceUsageDiagnostics::default();
    for value in 0..10 {
        write(
            &dir.path().join("pids.current"),
            format!("{value}\n").as_bytes(),
        );
        diagnostics.record(pin.sample(), false, false);
    }
    // An unavailable attempt advances the sequence but does not erase history.
    diagnostics.record(
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::IdentityChanged,
        },
        false,
        false,
    );
    // Keep the descriptor's identity valid while every dynamic metric file is
    // absent. The actual sample remains Observed, with all metrics unknown and
    // the two limit-only files still present.
    remove_dynamic_files(dir.path());
    let all_missing = pin.sample();
    assert!(
        matches!(&all_missing, ResourceUsageSample::Observed { snapshot }
        if matches!(&snapshot.pids_current, ResourceReading::Unavailable { .. })
            && matches!(&snapshot.memory_current, ResourceReading::Unavailable { .. })
            && matches!(&snapshot.pids_max, ResourceReading::Observed { .. })
            && matches!(&snapshot.memory_max, ResourceReading::Observed { .. }))
    );
    diagnostics.record(all_missing.clone(), false, false);
    pin.identity.inode = pin.identity.inode.saturating_add(1);
    diagnostics.record(pin.sample(), false, true);

    assert_eq!(diagnostics.sample_attempts, 13);
    assert_eq!(
        diagnostics
            .history
            .iter()
            .map(|entry| entry.as_ref().expect("full bounded history").attempt_number)
            .collect::<Vec<_>>(),
        (3..=10).collect::<Vec<_>>()
    );
    for (entry, value) in diagnostics.history.iter().zip(2..10) {
        let HistoricalResourceSample {
            sample: ResourceUsageSample::Observed { snapshot },
            ..
        } = entry.as_ref().expect("chronological history")
        else {
            panic!("history contains only complete observed snapshots");
        };
        assert_eq!(snapshot.pids_current, ResourceReading::Observed { value });
        assert_eq!(snapshot.pids_max, ResourceReading::Observed { value: 32 });
    }
    assert_eq!(
        diagnostics.latest,
        match all_missing {
            ResourceUsageSample::Observed { snapshot } => Some(snapshot),
            _ => unreachable!(),
        }
    );
    assert!(matches!(
        diagnostics.final_sample,
        Some(ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::IdentityChanged
        })
    ));
}

#[test]
fn history_preserves_partial_unknowns_and_skips_limit_only_observations() {
    let (dir, pin) = fixture();
    fs::remove_file(dir.path().join("pids.peak")).expect("one dynamic metric unavailable");
    let partial = pin.sample();
    let mut diagnostics = ResourceUsageDiagnostics::default();
    diagnostics.record(partial.clone(), false, false);
    assert_eq!(
        diagnostics.history[0]
            .as_ref()
            .expect("history sample")
            .sample,
        partial
    );
    assert!(matches!(
        diagnostics.history[0].as_ref().expect("history sample").sample,
        ResourceUsageSample::Observed { ref snapshot }
            if matches!(&snapshot.pids_peak, ResourceReading::Unavailable { .. })
                && matches!(&snapshot.memory_current, ResourceReading::Observed { .. })
                && matches!(&snapshot.pids_events, ResourceReading::Observed { value }
                    if value.max == Some(0))
                && matches!(&snapshot.cgroup_events, ResourceReading::Observed { value }
                    if value.populated == Some(true) && value.frozen == Some(false))
    ));

    remove_dynamic_files(dir.path());
    let limits_only = pin.sample();
    diagnostics.record(limits_only.clone(), false, false);
    assert_eq!(diagnostics.history.iter().flatten().count(), 1);

    // A present, parsed event file with no known counters/flags is not a
    // dynamic observation. Limit fields stay intact in latest but add no slot.
    let ResourceUsageSample::Observed { mut snapshot } = limits_only.clone() else {
        panic!("limit-only fixture sample remains observed");
    };
    snapshot.pids_current = ResourceReading::Unavailable {
        reason: ResourceReadFailure::Missing,
    };
    snapshot.pids_peak = ResourceReading::Unavailable {
        reason: ResourceReadFailure::Missing,
    };
    snapshot.memory_current = ResourceReading::Unavailable {
        reason: ResourceReadFailure::Missing,
    };
    snapshot.memory_peak = ResourceReading::Unavailable {
        reason: ResourceReadFailure::Missing,
    };
    snapshot.pids_events = ResourceReading::Observed {
        value: PidsEventCounters { max: None },
    };
    snapshot.pids_events_local = ResourceReading::Observed {
        value: PidsEventCounters { max: None },
    };
    snapshot.memory_events = ResourceReading::Observed {
        value: MemoryEventCounters {
            low: None,
            high: None,
            max: None,
            oom: None,
            oom_kill: None,
            oom_group_kill: None,
        },
    };
    snapshot.memory_events_local = snapshot.memory_events.clone();
    snapshot.cgroup_events = ResourceReading::Observed {
        value: CgroupEventCounters {
            populated: None,
            frozen: None,
        },
    };
    let empty_events = ResourceUsageSample::Observed { snapshot };
    diagnostics.record(empty_events.clone(), false, false);
    assert_eq!(diagnostics.history.iter().flatten().count(), 1);

    // The fixture's retained history demonstrates Some(0) and Some(false)
    // count as actual observations, rather than being mistaken for emptiness.
    assert_eq!(
        diagnostics.history[0]
            .as_ref()
            .expect("history sample")
            .sample,
        partial
    );
    assert_eq!(
        diagnostics.latest,
        match empty_events {
            ResourceUsageSample::Observed { snapshot } => Some(snapshot),
            _ => unreachable!(),
        }
    );
}

#[test]
fn history_defaults_for_legacy_json_and_rejects_wrong_array_length() {
    let legacy = serde_json::from_str::<ResourceUsageDiagnostics>("{}")
        .expect("legacy history omission defaults empty");
    assert!(legacy.history.iter().all(Option::is_none));
    assert!(legacy.is_empty());
    let wrong_length = r#"{"history":[null]}"#;
    assert!(serde_json::from_str::<ResourceUsageDiagnostics>(wrong_length).is_err());
}

#[test]
fn deleted_fixture_has_only_explicit_unknown_metrics_never_zero() {
    let (dir, pin) = fixture();
    fs::remove_dir_all(dir.path()).expect("simulated collected directory");
    match pin.sample() {
        ResourceUsageSample::Unavailable { reason } => {
            assert_eq!(reason, ResourceReadFailure::Missing)
        }
        ResourceUsageSample::Observed { snapshot } => {
            assert_eq!(
                snapshot.pids_current,
                ResourceReading::Unavailable {
                    reason: ResourceReadFailure::Missing
                }
            );
            assert_eq!(
                snapshot.memory_current,
                ResourceReading::Unavailable {
                    reason: ResourceReadFailure::Missing
                }
            );
        }
    }
}

fn binding_fixture() -> (
    RecoveryIdentity,
    EnforcementObservation,
    LaunchSpec,
    QualifiedEnvironment,
    SystemdBackendConfig,
) {
    let identity = AdmissionIdentity {
        project_id: ProjectId::new("project").expect("id"),
        task_id: TaskId::new("task").expect("id"),
        attempt_id: AttemptId::new("attempt").expect("id"),
        node_id: NodeId::new("node").expect("id"),
        lease_id: "lease".into(),
    };
    let spec = LaunchSpec {
        run_id: "run".into(),
        program: "/usr/bin/true".into(),
        args: vec!["literal".into()],
        cwd: "/owned/work".into(),
        spool_dir: "/owned/spool".into(),
    };
    let env = QualifiedEnvironment::new(BTreeMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), "/nonexistent".into()),
        ("TMPDIR".into(), "/tmp".into()),
    ]))
    .expect("nonsecret qualified env");
    let backend = SystemdBackendConfig {
        runtime_directory: "/run/user/example".into(),
        delegated_slice: "admiralty.slice".into(),
    };
    let recovery = RecoveryIdentity {
        identity,
        launch_spec_digest: content_digest(&spec).expect("specdigest"),
        worker_environment_digest: env.digest().expect("envdigest"),
        unit: "adm-resource-example.service".into(),
        run_id: "run".into(),
        permit_id: "permit".into(),
        reservation_ref: "reservation".into(),
        host_incarnation: "boot".into(),
        backend_config_digest: backend.digest().expect("backenddigest"),
        invocation_id: Some("a".repeat(32)),
        main_pid: Some(42),
        process_start: Some("123".into()),
    };
    let observation = EnforcementObservation {
        unit: recovery.unit.clone(),
        invocation_id: "a".repeat(32),
        main_pid: 42,
        process_start: "123".into(),
        process_ids: vec![42],
        cpu_set: BTreeSet::from([0, 1]),
        memory_max_bytes: 536870912,
        tasks_max: 32,
        actual_executable: spec.program.clone(),
        actual_cwd: spec.cwd.clone(),
        actual_argv_digest: content_digest(&argv_bytes(&spec).expect("argv")).expect("digest"),
        actual_environment_digest: env.digest().expect("digest"),
        cgroup_path: "/sys/fs/cgroup/owned/example".into(),
    };
    (recovery, observation, spec, env, backend)
}

#[test]
fn advisory_journal_changes_preserve_exact_legacy_closure_and_launch_binding_bytes() {
    #[derive(serde::Serialize)]
    struct LegacyClosed<'a> {
        identity: &'a RecoveryIdentity,
        observation: &'a EnforcementObservation,
        exit_code: Option<i32>,
        success: bool,
        closure_digest: &'a str,
    }
    let (recovery, observation, spec, env, backend) = binding_fixture();
    let hash = closed_admission_digest(&recovery, &observation, Some(0), true).expect("closure");
    let legacy_tuple = (
        "closed-admission-v1",
        &recovery,
        &observation,
        Some(0),
        true,
    );
    assert_eq!(
        hash,
        content_digest(&legacy_tuple).expect("unchanged exact prior expression")
    );
    let original_tuple_bytes = serde_json::to_vec(&legacy_tuple).expect("legacy binding bytes");
    let launch_bytes = serde_json::to_vec(&spec).expect("launch bytes");
    let env_digest = env.digest().expect("env");
    let backend_digest = backend.digest().expect("backend");
    let legacy = LegacyClosed {
        identity: &recovery,
        observation: &observation,
        exit_code: Some(0),
        success: true,
        closure_digest: &hash,
    };
    let mut closed = ClosedAdmission {
        identity: recovery.clone(),
        observation: observation.clone(),
        exit_code: Some(0),
        success: true,
        closure_digest: hash.clone(),
        resource_usage: ResourceUsageDiagnostics::default(),
    };
    let old_journal = serde_json::to_vec(&legacy).expect("legacy journal");
    assert_eq!(
        serde_json::to_vec(&closed).expect("empty advisory omitted"),
        old_journal
    );
    assert_eq!(
        serde_json::to_vec(&closed.authoritative_binding()).expect("explicit original binding"),
        old_journal
    );
    let (_dir, pin) = fixture();
    closed.resource_usage.record(pin.sample(), true, false);
    closed.resource_usage.record(
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::Missing,
        },
        false,
        true,
    );
    assert_ne!(
        serde_json::to_vec(&closed).expect("advisory journal"),
        old_journal
    );
    assert_eq!(
        serde_json::to_vec(&closed.authoritative_binding())
            .expect("advisory excluded from authority bytes"),
        old_journal
    );
    assert_eq!(
        serde_json::to_vec(&(
            "closed-admission-v1",
            &closed.identity,
            &closed.observation,
            closed.exit_code,
            closed.success
        ))
        .expect("bound tuple"),
        original_tuple_bytes
    );
    assert_eq!(closed.closure_digest(), hash);
    assert_eq!(
        closed_admission_digest(
            &closed.identity,
            &closed.observation,
            closed.exit_code,
            closed.success
        )
        .expect("same closure"),
        hash
    );
    assert_eq!(
        serde_json::to_vec(&spec).expect("launch unchanged"),
        launch_bytes
    );
    assert_eq!(env.digest().expect("env unchanged"), env_digest);
    assert_eq!(backend.digest().expect("backend unchanged"), backend_digest);
    assert_eq!(closed.resource_usage_diagnostics().sample_attempts, 2);
}

#[test]
fn legacy_absent_advisory_fields_decode_to_empty_data_not_a_capability() {
    assert_eq!(
        serde_json::from_str::<ResourceUsageDiagnostics>("{}").expect("legacy omission"),
        ResourceUsageDiagnostics::default()
    );
    let mut diagnostics = ResourceUsageDiagnostics::default();
    diagnostics.record(
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::NoObservedCgroup,
        },
        false,
        true,
    );
    assert!(diagnostics.latest.is_none());
    assert_eq!(diagnostics.sample_attempts, 1);
    assert!(!diagnostics.is_empty());
}

#[test]
fn first_closed_diagnostics_are_immutable_for_lost_journal_ack_retry() {
    let (dir, pin) = fixture();
    let mut diagnostics = ResourceUsageDiagnostics::default();
    diagnostics.record(pin.sample(), true, false);
    diagnostics.record(
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::Missing,
        },
        false,
        true,
    );
    diagnostics.frozen = true; // Same internal fence installed after actual closure proof.
    let retained = serde_json::to_vec(&diagnostics).expect("first closure advisory bytes");
    write(&dir.path().join("pids.events"), b"max 99\n");
    diagnostics.record(pin.sample(), false, false);
    diagnostics.record(
        ResourceUsageSample::Unavailable {
            reason: ResourceReadFailure::IdentityChanged,
        },
        false,
        true,
    );
    assert_eq!(
        serde_json::to_vec(&diagnostics).expect("exact retry bytes"),
        retained
    );
    assert_eq!(diagnostics.sample_attempts, 2);
    // cgroup.events legitimately has a public `frozen` counter. Only the
    // private top-level immutability fence must be absent from advisory JSON.
    let value: serde_json::Value = serde_json::from_slice(&retained).expect("JSON");
    assert!(value.get("frozen").is_none());
    let decoded: ResourceUsageDiagnostics =
        serde_json::from_str("{\"frozen\":true}").expect("legacy advisory JSON");
    assert!(!decoded.frozen);
}
