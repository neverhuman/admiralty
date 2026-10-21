use std::path::{Path, PathBuf};

use super::*;
use crate::FleetConfig;
use crate::system::CmdOutput;
use crate::testing::{FakeClock, FakeRunner, text};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/resources")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture")
}

/// Runner serving the captured PROBE outputs (local host over `bash -c`,
/// remote hosts over ssh).
fn fixture_runner() -> FakeRunner {
    FakeRunner::new(|spec| {
        let args = spec.args_lossy();
        let host = if spec.program == "bash" {
            assert_eq!(args, ["-c", PROBE]);
            "xbabe2".to_string()
        } else {
            assert_eq!(spec.program, "ssh");
            let target = args[args.len() - 2].clone();
            target.trim_start_matches("ubuntu@").to_string()
        };
        assert_ne!(host, "xbabe0");
        Ok(CmdOutput {
            code: Some(0),
            stdout: read(&format!("{host}.out")).into_bytes(),
            stderr: read(&format!("{host}.err")).into_bytes(),
            timed_out: false,
        })
    })
}

/// The JSON document, the history file and the printed table must equal
/// what `dashboard/globalres.py` produced for the same probe outputs,
/// history and clock (captured before the script was retired).
#[test]
fn sampler_matches_python_outputs_byte_for_byte() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        fixture("history_before.json"),
        dir.path().join(".res_history.json"),
    )
    .expect("seed history");
    let cfg = FleetConfig {
        dashboard_runtime: dir.path().to_path_buf(),
        ..FleetConfig::default()
    };
    let runner = fixture_runner();
    let clock = FakeClock::at("2026-10-21T12:00:00.250Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(resources(&mut ctx, false).expect("resources"), 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("global_resources.json")).expect("doc"),
        read("global_resources.json")
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".res_history.json")).expect("hist"),
        read("history_after.json")
    );
    assert_eq!(text(&out), read("table.txt"));
    assert!(!dir.path().join("global_resources.json.tmp").exists());
    assert_eq!(runner.calls().len(), 3);
}

#[test]
fn quiet_prints_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = FleetConfig {
        dashboard_runtime: dir.path().to_path_buf(),
        ..FleetConfig::default()
    };
    let runner = fixture_runner();
    let clock = FakeClock::at("2026-10-21T12:00:00Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(resources(&mut ctx, true).expect("resources"), 0);
    assert!(out.is_empty());
    assert!(dir.path().join("global_resources.json").exists());
}

#[test]
fn xbabe0_is_never_probed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = FleetConfig {
        dashboard_runtime: dir.path().to_path_buf(),
        resource_hosts: vec!["xbabe1".to_string(), "xbabe0".to_string()],
        ..FleetConfig::default()
    };
    let runner = fixture_runner();
    let clock = FakeClock::at("2026-10-21T12:00:00Z");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    assert_eq!(resources(&mut ctx, false).expect("resources"), 1);
    assert!(runner.calls().is_empty());
    assert!(text(&err).contains("xbabe0"));
    assert!(refuse_forbidden_hosts(&[" XBABE0 ".to_string()]).is_err());
    assert!(refuse_forbidden_hosts(&FleetConfig::default().resource_hosts).is_ok());
}

#[test]
fn probe_commands_match_python_argv() {
    let runner = FakeRunner::new(|_| Ok(CmdOutput::ok()));
    let clock = FakeClock::at("2026-10-21T12:00:00Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ctx = Ctx::new(&runner, &clock, &cfg, &mut out, &mut err);
    let remote = probe_command(&ctx, "xbabe1");
    assert_eq!(
        remote.argv_lossy(),
        [
            "ssh",
            "-n",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=6",
            "-o",
            "ServerAliveInterval=5",
            "ubuntu@xbabe1",
            PROBE
        ]
    );
    assert_eq!(remote.timeout, Duration::from_secs(25));
    let local = probe_command(&ctx, "xbabe2");
    assert_eq!(local.argv_lossy(), ["bash", "-c", PROBE]);
}

#[test]
fn probe_failures_become_python_error_strings() {
    let clock = FakeClock::at("2026-10-21T12:00:00Z");
    let cfg = FleetConfig::default();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let missing = FakeRunner::new(|_| Err(io::Error::new(io::ErrorKind::NotFound, "x")));
    let ctx = Ctx::new(&missing, &clock, &cfg, &mut out, &mut err);
    assert_eq!(
        probe(&ctx, "xbabe1"),
        (
            String::new(),
            "[Errno 2] No such file or directory: 'ssh'".to_string()
        )
    );
    let slow = FakeRunner::new(|_| {
        Ok(CmdOutput {
            timed_out: true,
            ..CmdOutput::default()
        })
    });
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ctx = Ctx::new(&slow, &clock, &cfg, &mut out, &mut err);
    let (o, e) = probe(&ctx, "xbabe3");
    assert!(o.is_empty());
    assert!(e.starts_with("Command '['ssh', '-n', '-o', 'BatchMode=yes', "));
    assert_eq!(e.chars().count(), 200);
    let noisy = FakeRunner::new(|_| {
        Ok(CmdOutput {
            code: Some(255),
            stderr: format!("{}tail", "x".repeat(300)).into_bytes(),
            ..CmdOutput::default()
        })
    });
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ctx = Ctx::new(&noisy, &clock, &cfg, &mut out, &mut err);
    let (_, e) = probe(&ctx, "xbabe1");
    assert_eq!(e.chars().count(), 200);
    assert!(e.ends_with("tail"));
}

#[test]
fn parse_handles_unreachable_and_partial_output() {
    let r = parse_probe("xbabe3", "", "  ").expect("parse");
    assert!(!r.reachable);
    assert_eq!(r.error.as_deref(), Some("no output"));
    let r = parse_probe("xbabe3", "@@HOST h\n@@NPROC\n@@LOAD\n", "boom\n").expect("parse");
    assert_eq!(r.error.as_deref(), Some("boom"));
    assert_eq!(r.nproc, Some(0));
    assert_eq!(r.load, Some(vec![]));
    assert_eq!(r.hostname.as_deref(), Some("h"));
    let json = py_json(&r, JsonLayout::Compact, true).expect("json");
    assert_eq!(
        json,
        r#"{"host": "xbabe3", "reachable": false, "error": "boom", "disks": [], "gpus": [], "top_rss": [], "screens": [], "hostname": "h", "nproc": 0, "load": [], "warnings": []}"#
    );
    // Malformed sections are hard errors, like the Python's uncaught exceptions.
    assert!(parse_probe("h", "@@MEM\nno colon here\n@@END\n", "").is_err());
    assert!(parse_probe("h", "@@DF\n/ 1 2\n@@END\n", "").is_err());
    assert!(parse_probe("h", "@@TOP\n1 root 2\n@@END\n", "").is_err());
    // Malformed GPU rows are skipped (the Python's try/except).
    let r = parse_probe("h", "@@GPU\n0, A, [N/A], 1, 2, 3\n1, B\n@@END\n", "").expect("parse");
    assert!(r.gpus.is_empty());
}

#[test]
fn growth_and_hours_to_full_follow_python_formulas() {
    let ts = 1_792_584_000.25;
    let mut hist = History::default();
    hist.set(
        "h:/",
        vec![
            serde_json::json!([ts - 7.0 * 3600.0, 1]),
            serde_json::json!([ts - 3.0 * 3600.0, 100_000_000_000_i64]),
            serde_json::json!([ts - 600.0, 200_000_000_000_i64]),
        ],
    );
    let mut d = Disk {
        mount: "/".to_string(),
        size: 1_000_000_000_000,
        used: 400_000_000_000,
        avail: 500_000_000_000,
        pct: py_round(100.0 * 400e9 / 900e9, 1),
        growth_gb_per_h: None,
        hours_to_full: None,
        rate_window_min: None,
        level: String::new(),
    };
    let w = assess_disk("h", &mut d, &mut hist, ts);
    // rate = (400e9 - 100e9) / (3 h) = 100 GB/h; 500 GB / 100 GB/h = 5 h.
    assert_eq!(d.growth_gb_per_h, Some(100.0));
    assert_eq!(d.hours_to_full, Some(5.0));
    assert_eq!(d.rate_window_min, Some(180));
    assert_eq!(d.level, "red");
    assert_eq!(
        w.map(|w| w.text),
        Some("h / 44.4% used, 500 GB free, ~5.0 h to full at 100.0 GB/h".to_string())
    );
    // The 7 h old sample was pruned; the new sample was appended.
    assert_eq!(hist.get("h:/").map(Vec::len), Some(3));
    // Base exactly 20 minutes old qualifies; younger does not.
    let mut hist = History::default();
    hist.set("h:/", vec![serde_json::json!([ts - 1199.0, 0])]);
    let mut d2 = d.clone();
    assess_disk("h", &mut d2, &mut hist, ts);
    assert_eq!(d2.growth_gb_per_h, None);
    assert_eq!(d2.rate_window_min, None);
    let mut hist = History::default();
    hist.set(
        "h:/",
        vec![serde_json::json!([ts - 1200.0, 400_000_000_000_i64])],
    );
    let mut d3 = d.clone();
    assess_disk("h", &mut d3, &mut hist, ts);
    assert_eq!(d3.growth_gb_per_h, Some(0.0));
    assert_eq!(d3.hours_to_full, None);
    assert_eq!(d3.rate_window_min, Some(20));
    assert_eq!(d3.level, "ok");
}

#[test]
fn history_preserves_key_order_and_python_compact_layout() {
    let h: History =
        serde_json::from_str(r#"{"z": [[1.5, 2]], "a": [], "z": [[3.0, 4]], "bad": 7}"#)
            .expect("parse");
    assert_eq!(
        py_json(&h, JsonLayout::Compact, true).expect("json"),
        r#"{"z": [[3.0, 4]], "a": [], "bad": []}"#
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("h.json");
    std::fs::write(&p, "not json").expect("write");
    assert_eq!(History::load(&p), History::default());
    assert_eq!(
        History::load(&dir.path().join("missing")),
        History::default()
    );
}

#[test]
fn timestamp_matches_python_datetime_timestamp() {
    let t = DateTime::parse_from_rfc3339("2026-10-21T12:00:00.250Z")
        .expect("time")
        .with_timezone(&Utc);
    assert_eq!(py_float_repr(py_timestamp(t)), "1792584000.25");
}
