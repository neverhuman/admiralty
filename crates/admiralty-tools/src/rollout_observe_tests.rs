//! Ported from `tests/legacy/test_rollout_observe.py` (ObserverTests).

use std::cell::{Cell, RefCell};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Stdio};

use chrono::TimeZone;

use super::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0)
        .single()
        .expect("valid fixed time")
}

fn json(text: &str) -> Value {
    pyjson::loads(text, LoadOptions::default()).expect("test JSON")
}

fn empty() -> Value {
    Value::Object(Map::new())
}

fn template() -> Value {
    json(
        r#"{"schema_version": 1, "project": {"id": "arbitrary-project"},
            "updated_utc": "2026-10-18T10:00:00Z", "producer": "integration-owner",
            "packets": [{"id": "P00", "status": "integrated", "verification": {"receipt": "real"}, "integration": {"receipt": "real"}},
                        {"id": "P01", "status": "planned", "verification": null, "integration": null}],
            "lanes": [{"id": "pending-lane", "owner": "captain", "status": "claimed"}]}"#,
    )
}

fn screens(available: bool, sessions: &[(&str, i128)]) -> ScreenSnapshot {
    ScreenSnapshot {
        available,
        sessions: sessions
            .iter()
            .map(|(name, pid)| ((*name).to_owned(), *pid))
            .collect(),
        reason: String::new(),
    }
}

fn merge_with(reports: &Value, screens: Option<&ScreenSnapshot>) -> ToolResult<Value> {
    let mut options = MergeOptions::new(now());
    options.prefix = Some("adm_lane_");
    options.screens = screens;
    merge(&template(), Some(reports), &empty(), &empty(), &options)
}

fn lane<'a>(result: &'a Value, identity: &str) -> &'a Map {
    result
        .get_value("lanes")
        .ok()
        .and_then(Value::as_array)
        .and_then(|lanes| {
            lanes
                .iter()
                .filter_map(Value::as_object)
                .find(|row| row.get("id") == Some(&Value::Str(identity.to_owned())))
        })
        .expect("lane present")
}

fn field<'a>(row: &'a Map, key: &str) -> &'a Value {
    row.get(key).unwrap_or(&NULL)
}

fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

fn activity(result: &Value, identity: &str) -> Map {
    field(lane(result, identity), "activity")
        .as_object()
        .cloned()
        .expect("activity present")
}

/// Ported from test_rollout_observe.py::test_live_lanes_do_not_promote_packets_or_freshen_source
#[test]
fn test_live_lanes_do_not_promote_packets_or_freshen_source() {
    let template = template();
    let report = json(
        r#"{"updated_utc": "2026-10-18T12:00:00Z", "lanes": [{"id": "worker", "screen": "123.adm_lane_core", "packet": "P01", "status": "integrated", "verification": {"receipt": "forged"}}]}"#,
    );
    let result = merge_with(&report, Some(&screens(true, &[("adm_lane_core", 123)])))
        .expect("merge succeeds");
    assert_eq!(
        result.get_value("packets").ok(),
        template.get_value("packets").ok()
    );
    assert_eq!(
        result.get_value("updated_utc").ok(),
        template.get_value("updated_utc").ok()
    );
    assert_eq!(
        result.get_value("producer").ok(),
        template.get_value("producer").ok()
    );
    let worker = lane(&result, "worker");
    assert_eq!(text_of(field(worker, "screen_state")), "present");
    assert_eq!(text_of(field(worker, "observation_state")), "fresh");
    let observer = result.get_value("observer").expect("observer");
    assert_eq!(
        text_of(observer.get_value("observed_utc").expect("observed")),
        "2026-10-18T12:00:00+00:00"
    );
}

/// Ported from test_rollout_observe.py::test_stale_orphaned_pending_and_unknown_are_explicit
#[test]
fn test_stale_orphaned_pending_and_unknown_are_explicit() {
    let reports = json(
        r#"[{"id": "old", "updated_utc": "2026-10-18T10:00:00Z", "screen": "adm_lane_old", "status": "running"},
            {"id": "gone", "updated_utc": "2026-10-18T12:00:00Z", "screen": "adm_lane_gone"}]"#,
    );
    let result =
        merge_with(&reports, Some(&screens(true, &[("adm_lane_old", 10)]))).expect("merge");
    assert_eq!(
        text_of(field(lane(&result, "old"), "observation_state")),
        "stale"
    );
    assert_eq!(
        text_of(field(lane(&result, "gone"), "observation_state")),
        "orphaned"
    );
    assert_eq!(
        text_of(field(lane(&result, "pending-lane"), "observation_state")),
        "pending"
    );
    let result = merge_with(&reports, Some(&screens(false, &[]))).expect("merge");
    for row in result
        .get_value("lanes")
        .ok()
        .and_then(Value::as_array)
        .expect("lanes")
    {
        assert_ne!(
            row.get_value("screen_state").ok(),
            Some(&Value::from("absent"))
        );
    }
}

/// Ported from test_rollout_observe.py::test_pid_mismatch_and_out_of_scope_sessions
#[test]
fn test_pid_mismatch_and_out_of_scope_sessions() {
    let reports = json(
        r#"[{"id": "wrong", "screen": "12.adm_lane_core"}, {"id": "foreign", "screen": "captain_other"}]"#,
    );
    let result = merge_with(
        &reports,
        Some(&screens(
            true,
            &[("adm_lane_core", 13), ("captain_other", 14)],
        )),
    )
    .expect("merge");
    assert_eq!(
        text_of(field(lane(&result, "wrong"), "screen_state")),
        "identity_mismatch"
    );
    assert_eq!(
        text_of(field(lane(&result, "foreign"), "screen_state")),
        "out_of_scope"
    );
}

/// Ported from test_rollout_observe.py::test_duplicate_lanes_are_stable_and_conflicts_do_not_look_healthy
#[test]
fn test_duplicate_lanes_are_stable_and_conflicts_do_not_look_healthy() {
    let rows =
        json(r#"[{"id": "worker", "status": "running"}, {"id": "worker", "status": "done"}]"#);
    let reversed =
        json(r#"[{"id": "worker", "status": "done"}, {"id": "worker", "status": "running"}]"#);
    let first = merge_with(&rows, None).expect("merge");
    let second = merge_with(&reversed, None).expect("merge");
    assert_eq!(
        first.get_value("lanes").ok(),
        second.get_value("lanes").ok()
    );
    let worker = lane(&first, "worker");
    assert_eq!(text_of(field(worker, "observation_state")), "pending");
    assert!(text_of(field(worker, "status")).contains("conflicting duplicate"));
    let same =
        json(r#"[{"id": "worker", "status": "running"}, {"id": "worker", "status": "running"}]"#);
    let merged = merge_with(&same, None).expect("merge");
    assert_eq!(
        merged
            .get_value("lanes")
            .ok()
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
}

/// Ported from test_rollout_observe.py::test_invalid_shapes_ids_and_limits
#[test]
fn test_invalid_shapes_ids_and_limits() {
    let many: Vec<String> = (0..201).map(|i| format!(r#"{{"id": "x{i}"}}"#)).collect();
    let many = format!("[{}]", many.join(","));
    for bad in [
        r#"{"lanes": {}}"#,
        r#"[{"id": "../escape"}]"#,
        r#"[{"id": "x", "screen_pid": -1}]"#,
        many.as_str(),
    ] {
        let result = merge_with(&json(bad), None);
        assert!(
            matches!(result, Err(ToolError::Invalid(_))),
            "{}",
            &bad[..bad.len().min(50)]
        );
    }
}

/// Ported from test_rollout_observe.py::test_json_inputs_are_regular_bounded_owned_and_no_symlinks
#[test]
fn test_json_inputs_are_regular_bounded_owned_and_no_symlinks() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let directory = temporary.path();
    let source = directory.join("source.json");
    std::fs::write(&source, pyjson::dumps_default(&template())).expect("write source");
    read_owned(&source).expect("owned regular JSON is accepted");
    let link = directory.join("link.json");
    symlink(&source, &link).expect("symlink");
    assert!(matches!(read_owned(&link), Err(ToolError::Invalid(_))));
    assert!(matches!(
        read_owned(&directory.join("..").join("source.json")),
        Err(ToolError::Invalid(_))
    ));
    std::fs::write(&source, vec![b' '; MAX_BYTES + 1]).expect("oversize");
    assert!(matches!(read_owned(&source), Err(ToolError::Invalid(_))));
    std::fs::write(&source, r#"{"a":1,"a":2}"#).expect("duplicate");
    assert!(matches!(read_owned(&source), Err(ToolError::Invalid(_))));
}

/// Ported from test_rollout_observe.py::test_provider_internal_sessions_remain_unknown_and_cpu_reservation_is_not_pinning
#[test]
fn test_provider_internal_sessions_remain_unknown_and_cpu_reservation_is_not_pinning() {
    let reports = json(
        r#"{"launch_method": "provider-internal subagents", "updated_utc": "2026-10-18T12:00:00Z", "lanes": [
            {"lane": "adm_lane_1_ui", "task": "ADM-P12A", "model": "gpt-6-luna", "effort": "medium", "tier": "T0",
             "screen": null, "pid": null, "agent_session": "/root/adm_lane_1_ui", "status": "candidate_committed",
             "reserved_cpus": [104, 105], "cpus_pinned": false}]}"#,
    );
    let result = merge_with(&reports, Some(&screens(true, &[]))).expect("merge");
    let row = lane(&result, "adm_lane_1_ui");
    assert_eq!(text_of(field(row, "screen_state")), "not_exposed");
    assert_ne!(text_of(field(row, "observation_state")), "orphaned");
    assert!(text_of(field(row, "status")).contains("candidate_committed"));
    assert!(text_of(field(row, "status")).contains("NOT pinned"));
    assert_eq!(text_of(field(row, "packet_id")), "ADM-P12A");
    assert!(field(row, "screen_pid").is_null());
    assert_eq!(
        result.get_value("packets").ok(),
        template().get_value("packets").ok()
    );
}

/// Ported from test_rollout_observe.py::test_missing_lane_source_is_pending_without_subprocess
#[test]
fn test_missing_lane_source_is_pending_without_subprocess() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let directory = temporary.path();
    let source = directory.join("source.json");
    let lanes = directory.join("lanes.json");
    let output = directory.join("rollout.json");
    std::fs::write(&source, pyjson::dumps_default(&template())).expect("write source");
    let options = ObserveOptions {
        screen_probe: &|_: &str| -> ToolResult<ScreenSnapshot> {
            panic!("no screen probe expected")
        },
        ..ObserveOptions::default()
    };
    let result = observe(&source, &lanes, &output, &options).expect("observe");
    let observer = result.get_value("observer").expect("observer");
    assert_eq!(
        observer.get_value("reports_state").ok(),
        Some(&Value::from("pending"))
    );
    let first = result
        .get_value("lanes")
        .ok()
        .and_then(Value::as_array)
        .and_then(|lanes| lanes.first())
        .expect("first lane");
    assert_eq!(
        first.get_value("observation_state").ok(),
        Some(&Value::from("pending"))
    );
    let written = std::fs::read_to_string(&output).expect("output");
    assert_eq!(json(&written), result);
}

/// Ported from test_rollout_observe.py::test_safe_atomic_output_preserves_previous_on_failure
#[test]
fn test_safe_atomic_output_preserves_previous_on_failure() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let directory = temporary.path();
    let source = directory.join("source.json");
    let output = directory.join("rollout.json");
    std::fs::write(&source, "{}").expect("source");
    std::fs::write(&output, "previous").expect("output");
    let inputs = [safe_path(&source).expect("safe source")];
    assert!(matches!(
        atomic_output(&source, &empty(), &inputs, &fsx::os_replace),
        Err(ToolError::Invalid(_))
    ));
    let crash = |_: &Path, _: &Path| -> io::Result<()> { Err(io::Error::other("synthetic crash")) };
    assert!(matches!(
        atomic_output(&output, &empty(), &inputs, &crash),
        Err(ToolError::Io(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&output).expect("output"),
        "previous"
    );
    let leftovers: Vec<_> = std::fs::read_dir(directory)
        .expect("listing")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".rollout-observe-")
        })
        .collect();
    assert!(leftovers.is_empty(), "temporary output must not survive");
    let link = directory.join("unsafe.json");
    symlink(&source, &link).expect("symlink");
    assert!(matches!(
        atomic_output(&link, &empty(), &[], &fsx::os_replace),
        Err(ToolError::Invalid(_))
    ));
}

/// Ported from test_rollout_observe.py::test_input_objects_never_mutated
#[test]
fn test_input_objects_never_mutated() {
    let template = template();
    let reports = json(r#"[{"id": "worker", "status": "running"}]"#);
    let before = (template.clone(), reports.clone());
    merge(
        &template,
        Some(&reports),
        &empty(),
        &empty(),
        &MergeOptions::new(now()),
    )
    .expect("merge");
    assert_eq!((template, reports), before);
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    worktree: PathBuf,
}

fn mkdir_mode(path: &Path, mode: u32) {
    std::fs::create_dir(path).expect("mkdir");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn witness_fixture() -> Fixture {
    let temporary = tempfile::tempdir().expect("tempdir");
    let root = temporary.path().join("owned");
    mkdir_mode(&root, 0o700);
    let worktree = root.join("lane");
    mkdir_mode(&worktree, 0o700);
    mkdir_mode(&worktree.join(".git"), 0o700);
    std::fs::create_dir(worktree.join("src")).expect("src");
    std::fs::write(
        worktree.join("src").join("one.rs"),
        "PRIVATE SOURCE MUST NEVER BE RETURNED",
    )
    .expect("source");
    Fixture {
        _temporary: temporary,
        root,
        worktree,
    }
}

fn witness_merge(
    reports: &Value,
    root: Option<&Path>,
    probe: HeadProbe<'_>,
    limits: Limits,
) -> Value {
    let mut options = MergeOptions::new(now());
    options.worktree_root = root;
    options.head_probe = probe;
    options.limits = limits;
    merge(&template(), Some(reports), &empty(), &empty(), &options).expect("merge")
}

fn report(value: Value) -> Value {
    Value::Array(vec![value])
}

/// Ported from test_rollout_observe.py::test_worktree_witness_is_opt_in_and_never_refreshes_report_or_packet_proof
#[test]
fn test_worktree_witness_is_opt_in_and_never_refreshes_report_or_packet_proof() {
    let fixture = witness_fixture();
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "candidate_paths" => Value::Array(vec!["src/**".into()]),
        "head_sha" => "a".repeat(40),
        "status" => "verified",
        "updated_utc" => "2026-10-18T10:00:00Z",
    });
    let calls = Cell::new(0);
    let probe = |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> {
        calls.set(calls.get() + 1);
        Ok("a".repeat(40))
    };
    let disabled = witness_merge(&reports, None, &probe, Limits::default());
    assert_eq!(calls.get(), 0, "head probe must not run when disabled");
    let observed = witness_merge(&reports, Some(&fixture.root), &probe, Limits::default());
    assert!(!lane(&disabled, "worker").contains_key("activity"));
    let row = lane(&observed, "worker");
    assert_eq!(text_of(field(row, "observation_state")), "stale");
    assert_eq!(
        text_of(field(row, "source_updated_utc")),
        "2026-10-18T10:00:00Z"
    );
    let witnessed = activity(&observed, "worker");
    assert_eq!(witnessed.get("owned_scope_files"), Some(&Value::Int(1)));
    assert_eq!(
        witnessed.get("reported_head_matches"),
        Some(&Value::Bool(true))
    );
    assert!(text_of(field(row, "status")).contains("owned scope files 1"));
    assert_eq!(
        observed.get_value("packets").ok(),
        template().get_value("packets").ok()
    );
    assert_eq!(
        observed.get_value("updated_utc").ok(),
        template().get_value("updated_utc").ok()
    );
    assert!(!pyjson::dumps_default(&observed).contains("PRIVATE SOURCE"));
}

/// Ported from test_rollout_observe.py::test_scope_metadata_never_follows_directory_or_file_symlinks
#[test]
fn test_scope_metadata_never_follows_directory_or_file_symlinks() {
    let fixture = witness_fixture();
    let outside = fixture.root.parent().expect("parent").join("outside");
    std::fs::create_dir(&outside).expect("outside");
    std::fs::write(outside.join("never-read.rs"), "OUTSIDE PRIVATE SOURCE").expect("outside file");
    symlink(&outside, fixture.worktree.join("src").join("escape")).expect("dir symlink");
    symlink(
        outside.join("never-read.rs"),
        fixture.worktree.join("src").join("escape.rs"),
    )
    .expect("file symlink");
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "candidate_paths" => Value::Array(vec!["src/**".into(), "src/escape/never-read.rs".into()]),
    });
    let probe = |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> { Ok("b".repeat(40)) };
    let result = witness_merge(&reports, Some(&fixture.root), &probe, Limits::default());
    let witnessed = activity(&result, "worker");
    assert_eq!(witnessed.get("owned_scope_files"), Some(&Value::Int(1)));
    let skipped = witnessed
        .get("skipped_entries")
        .and_then(Value::as_exact_int)
        .expect("skipped count");
    assert!(skipped >= 2);
    assert!(!pyjson::dumps_default(&result).contains("OUTSIDE PRIVATE SOURCE"));
}

/// Ported from test_rollout_observe.py::test_outside_symlink_and_unbounded_scope_reports_fail_closed
#[test]
fn test_outside_symlink_and_unbounded_scope_reports_fail_closed() {
    let fixture = witness_fixture();
    let alias = fixture.root.join("alias");
    symlink(&fixture.worktree, &alias).expect("alias");
    let parent = fixture.root.parent().expect("parent").to_path_buf();
    let mut variants = vec![
        (parent.display().to_string(), "src/one.rs".to_owned()),
        (alias.display().to_string(), "src/one.rs".to_owned()),
    ];
    for scope in [
        "**".to_owned(),
        "./**".to_owned(),
        "../outside".to_owned(),
        "src/*.rs".to_owned(),
        parent.join("outside").display().to_string(),
    ] {
        variants.push((fixture.worktree.display().to_string(), scope));
    }
    let probe = |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> {
        panic!("unsafe report must not invoke Git")
    };
    for (worktree, scope) in variants {
        let reports = report(crate::pyobj! {
            "id" => "worker",
            "worktree" => worktree.as_str(),
            "candidate_paths" => Value::Array(vec![scope.as_str().into()]),
        });
        let result = witness_merge(&reports, Some(&fixture.root), &probe, Limits::default());
        assert_eq!(
            activity(&result, "worker").get("state"),
            Some(&Value::from("unsafe")),
            "{worktree} {scope}"
        );
    }
    std::fs::set_permissions(&fixture.root, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(matches!(
        WorktreeWitness::open(&fixture.root, Limits::default(), &bounded_git_head),
        Err(ToolError::Invalid(_))
    ));
    assert!(matches!(
        WorktreeWitness::open(
            Path::new("relative-root"),
            Limits::default(),
            &bounded_git_head
        ),
        Err(ToolError::Invalid(_))
    ));
}

/// Ported from test_rollout_observe.py::test_recursive_metadata_has_entry_and_depth_bounds_and_deduplicates_paths
#[test]
fn test_recursive_metadata_has_entry_and_depth_bounds_and_deduplicates_paths() {
    let fixture = witness_fixture();
    for number in 0..8 {
        std::fs::write(
            fixture.worktree.join("src").join(format!("{number}.rs")),
            "fixture",
        )
        .expect("fixture file");
    }
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "candidate_paths" => Value::Array(vec!["src/one.rs".into(), "src/**".into()]),
    });
    let probe = |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> { Ok("c".repeat(40)) };
    let complete = witness_merge(&reports, Some(&fixture.root), &probe, Limits::default());
    let bounded_limits = Limits {
        max_scope_entries: 3,
        ..Limits::default()
    };
    let bounded = witness_merge(&reports, Some(&fixture.root), &probe, bounded_limits);
    let full = activity(&complete, "worker");
    let partial = activity(&bounded, "worker");
    assert_eq!(full.get("owned_scope_files"), Some(&Value::Int(9)));
    assert_eq!(partial.get("state"), Some(&Value::from("bounded")));
    let scanned = partial
        .get("scanned_entries")
        .and_then(Value::as_exact_int)
        .expect("scanned");
    assert!(scanned <= 3);
    let nested = fixture.worktree.join("nested");
    std::fs::create_dir(&nested).expect("nested");
    std::fs::create_dir(nested.join("child")).expect("child");
    std::fs::create_dir(nested.join("child").join("deep")).expect("deep");
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "candidate_path" => "nested/**",
    });
    let shallow = Limits {
        max_scope_depth: 1,
        ..Limits::default()
    };
    let result = witness_merge(&reports, Some(&fixture.root), &probe, shallow);
    assert_eq!(
        activity(&result, "worker").get("state"),
        Some(&Value::from("bounded"))
    );
}

/// Ported from test_rollout_observe.py::test_head_only_suffix_requires_full_matching_observed_reference
#[test]
fn test_head_only_suffix_requires_full_matching_observed_reference() {
    let fixture = witness_fixture();
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "head_sha" => "d".repeat(40),
    });
    let matching_probe =
        |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> { Ok("d".repeat(40)) };
    let matching = witness_merge(
        &reports,
        Some(&fixture.root),
        &matching_probe,
        Limits::default(),
    );
    assert!(
        text_of(field(lane(&matching, "worker"), "status")).contains("candidate HEAD dddddddddddd")
    );
    let mismatch_probe =
        |_: BorrowedFd<'_>, _: Duration| -> ToolResult<String> { Ok("e".repeat(40)) };
    let mismatch = witness_merge(
        &reports,
        Some(&fixture.root),
        &mismatch_probe,
        Limits::default(),
    );
    assert_eq!(
        activity(&mismatch, "worker").get("reported_head_matches"),
        Some(&Value::Bool(false))
    );
    assert!(!text_of(field(lane(&mismatch, "worker"), "status")).contains("candidate HEAD"));
}

fn process_gone(pid: u32) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists()
}

/// Ported from test_rollout_observe.py::test_git_probe_is_fixed_head_only_descriptor_bound_and_output_bounded
#[test]
fn test_git_probe_is_fixed_head_only_descriptor_bound_and_output_bounded() {
    let fixture = witness_fixture();
    let root = File::open(&fixture.root).expect("root descriptor");
    let root_inode = std::fs::metadata(&fixture.root).expect("root meta").ino();

    // Fixed argv and sanitized environment (foreign GIT_* never leaks).
    let foreign = [
        (OsString::from("GIT_DIR"), OsString::from("/foreign")),
        (OsString::from("GIT_WORK_TREE"), OsString::from("/foreign")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ];
    let (argv, environment) = git_head_invocation("/proc/self/fd/7", foreign);
    assert_eq!(
        argv[..5],
        [
            "git",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-C"
        ]
    );
    assert_eq!(argv[argv.len() - 3..], ["rev-parse", "--verify", "HEAD"]);
    assert!(!argv.iter().any(|arg| arg == "status"));
    assert_eq!(
        environment.get(OsStr::new("GIT_WORK_TREE")),
        Some(&OsString::from(&argv[5]))
    );
    assert!(!environment.contains_key(OsStr::new("GIT_DIR")));

    // Descriptor inheritance uses Linux procfs. Other platforms must refuse
    // before spawning rather than silently falling back to a mutable pathname.
    if !Path::new("/proc/self/fd").is_dir() {
        let called = Cell::new(false);
        let denied_spawn = |_: &[String], _: &BTreeMap<OsString, OsString>| {
            called.set(true);
            Err(io::Error::other(
                "must not spawn without descriptor support",
            ))
        };
        let error = bounded_git_head_with(root.as_fd(), Duration::from_secs(5), &denied_spawn)
            .expect_err("unsupported descriptor-bound Git must refuse");
        assert!(
            error
                .to_string()
                .contains("descriptor-bound Git unavailable")
        );
        assert!(!called.get());
        return;
    }

    // The spawned child receives a descriptor-bound target for the same directory.
    let recorded: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let pid = Cell::new(0u32);
    let spawner = |script: &'static str| {
        let recorded = &recorded;
        let pid = &pid;
        move |argv: &[String], environment: &BTreeMap<OsString, OsString>| -> io::Result<Child> {
            recorded.replace(argv.to_vec());
            assert_eq!(
                environment.get(OsStr::new("GIT_WORK_TREE")),
                Some(&OsString::from(&argv[5]))
            );
            let target = std::fs::metadata(&argv[5])?;
            assert_eq!(target.ino(), root_inode, "descriptor-bound target");
            let child = Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            pid.set(child.id());
            Ok(child)
        }
    };
    let good = spawner("printf '%s\\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(
        bounded_git_head_with(root.as_fd(), Duration::from_secs(5), &good).expect("head"),
        "a".repeat(40)
    );
    assert!(recorded.borrow()[5].starts_with("/proc/self/fd/"));

    let oversized = spawner("head -c 257 /dev/zero | tr '\\000' f; exec sleep 30");
    let started = Instant::now();
    assert!(matches!(
        bounded_git_head_with(root.as_fd(), Duration::from_secs(5), &oversized),
        Err(ToolError::Invalid(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(process_gone(pid.get()), "oversized child must be killed");

    let silent = spawner("printf '%s' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; exec sleep 30");
    let started = Instant::now();
    assert!(matches!(
        bounded_git_head_with(root.as_fd(), Duration::from_secs(1), &silent),
        Err(ToolError::Invalid(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(process_gone(pid.get()), "timed-out child must be killed");
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        && Path::new("/proc/self/fd").is_dir()
}

/// Ported from test_rollout_observe.py::test_real_git_head_probe_uses_only_disposable_worktree_metadata
#[test]
fn test_real_git_head_probe_uses_only_disposable_worktree_metadata() {
    if !git_available() {
        eprintln!("skipped: Linux procfs and Git required for descriptor-bound HEAD check");
        return;
    }
    let fixture = witness_fixture();
    let metadata = fixture.worktree.join(".git");
    std::fs::create_dir(metadata.join("objects")).expect("objects");
    std::fs::create_dir_all(metadata.join("refs").join("heads")).expect("refs");
    std::fs::write(metadata.join("HEAD"), "ref: refs/heads/main\n").expect("HEAD");
    std::fs::write(
        metadata.join("refs").join("heads").join("main"),
        format!("{}\n", "f".repeat(40)),
    )
    .expect("main");
    // This synthetic ref names no accepted commit or task; the witness only
    // corroborates repository HEAD metadata and never grants acceptance.
    let reports = report(crate::pyobj! {
        "id" => "worker",
        "worktree" => fixture.worktree.display().to_string(),
        "head_sha" => "f".repeat(40),
    });
    let result = witness_merge(
        &reports,
        Some(&fixture.root),
        &bounded_git_head,
        Limits::default(),
    );
    let witnessed = activity(&result, "worker");
    assert_eq!(witnessed.get("head_sha"), Some(&Value::Str("f".repeat(40))));
    assert_eq!(
        witnessed.get("reported_head_matches"),
        Some(&Value::Bool(true))
    );
}
