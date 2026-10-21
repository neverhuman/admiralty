//! End-to-end audit runs against a fake host and a hermetic HOME: output
//! files, key order, Markdown layout, sorting and the progress history.

use std::time::Duration;

use admiralty_audit::host::CmdOut;
use admiralty_audit::host::fake::FakeHost;
use admiralty_audit::py::{self, J};
use admiralty_audit::{AuditOptions, run_audit};

use crate::common::{at, hermetic_config};

const LS: &str = "There are screens on:\n\t101.captain_joil\t(Detached)\n\t102.captain_dope\t(Detached)\n\t103.captain_jopedime\t(Detached)\n\t104.agy_captain_admiralty\t(Detached)\n\t105.super_captain\t(Detached)\n5 Sockets in /run/screen/S-ubuntu.\n";

fn host(now: &str) -> FakeHost {
    FakeHost::new(at(now))
        .with_runner(|argv, _| match argv {
            ["screen", "-ls"] => CmdOut::rc(1, LS),
            ["git", ..] => CmdOut::rc(128, ""),
            ["ps", ..] => CmdOut::ok(""),
            other => panic!("unexpected command with --no-gh: {other:?}"),
        })
        .with_hardcopy(|_, _| "working on it\nesc to cancel\n".to_string())
}

fn keys(j: &J) -> Vec<String> {
    j.as_obj()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn codes(report: &J) -> Vec<String> {
    report
        .as_obj()
        .and_then(|o| o.get("findings"))
        .and_then(J::as_arr)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    f.as_obj()
                        .and_then(|o| o.get("code"))
                        .and_then(J::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn two_runs_write_python_shaped_outputs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_string_lossy().to_string();
    let cfg = hermetic_config(&home);
    std::fs::create_dir_all(&cfg.kick).expect("mkdir");
    std::fs::write(
        cfg.status_json(),
        r#"{"updated_utc": "2026-10-21T15:20:00Z", "captains": {"jopedime": {"done": 5, "state": "5/66 done", "workers_idle": 0}}}"#,
    )
    .expect("write status");
    let out = format!("{home}/out");
    let opts = AuditOptions {
        out: out.clone(),
        quiet: true,
        no_gh: true,
    };

    // ---- first run
    let h1 = host("2026-10-21T15:30:00+00:00");
    let r1 = run_audit(&cfg, &h1, &opts).expect("run 1");
    assert_eq!(
        keys(&r1.report),
        [
            "utc",
            "projects",
            "screens",
            "lanes",
            "native_queue",
            "progress",
            "watch",
            "super_captain_status_utc",
            "findings",
            "summary",
            "must_act"
        ]
    );
    let projects = r1
        .report
        .as_obj()
        .and_then(|o| o.get("projects"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        keys(&projects),
        ["jopedime", "dope", "jarambit", "joil", "admiralty"]
    );
    let c1 = codes(&r1.report);
    for want in [
        "SCREEN_MISSING",
        "LEDGER_MISSING",
        "LEDGER_UNREADABLE",
        "NATIVE_QUEUE_UNKNOWN",
        "SC_REPORTED_OVERCLAIM",
        "SC_STATE_TEXT_OVERCLAIM",
    ] {
        assert!(c1.contains(&want.to_string()), "{want} missing from {c1:?}");
    }
    assert!(
        !c1.contains(&"NO_PROGRESS".to_string()),
        "no baseline on the first run"
    );
    // joil's exception text matches Python's str(TypeError)
    let joil_text = r1
        .report
        .as_obj()
        .and_then(|o| o.get("findings"))
        .and_then(J::as_arr)
        .and_then(|a| {
            a.iter().find(|f| {
                py::py_str(f.as_obj().and_then(|o| o.get("scope")).unwrap_or(&J::Null)) == "joil"
            })
        })
        .and_then(|f| f.as_obj().and_then(|o| o.get("text")).cloned());
    assert_eq!(
        joil_text,
        Some(J::from("'NoneType' object is not subscriptable"))
    );
    // findings are sorted red < amber < info, then scope, then code
    let findings = r1
        .report
        .as_obj()
        .and_then(|o| o.get("findings"))
        .and_then(J::as_arr)
        .cloned()
        .unwrap_or_default();
    let rank = |s: &str| match s {
        "red" => 0,
        "amber" => 1,
        _ => 2,
    };
    let sort_keys: Vec<(i32, String, String)> = findings
        .iter()
        .map(|f| {
            let g = |k: &str| py::py_str(f.as_obj().and_then(|o| o.get(k)).unwrap_or(&J::Null));
            (rank(&g("severity")), g("scope"), g("code"))
        })
        .collect();
    let mut sorted = sort_keys.clone();
    sorted.sort();
    assert_eq!(sort_keys, sorted);
    // files
    for f in ["state.json", "latest.json", "latest.md", "audit.jsonl"] {
        assert!(std::path::Path::new(&format!("{out}/{f}")).is_file(), "{f}");
    }
    assert!(!std::path::Path::new(&format!("{out}/state.json.tmp")).exists());
    let latest = std::fs::read_to_string(format!("{out}/latest.json")).expect("latest");
    assert_eq!(latest, py::dumps(&r1.report, Some(1)));
    assert!(latest.starts_with("{\n \"utc\": \"2026-10-21T15:30:00+00:00\",\n \"projects\": {"));
    let md = std::fs::read_to_string(format!("{out}/latest.md")).expect("md");
    assert_eq!(md, r1.markdown);
    let lines: Vec<&str> = md.lines().collect();
    assert_eq!(lines[0], "# super_captain audit 2026-10-21T15:30:00+00:00");
    assert_eq!(
        lines[2],
        "| Project | done (recomputed) / total | super_captain reported | captain claim | source |"
    );
    assert!(
        lines[4].starts_with("| jopedime | 0 / 66 | 5 | "),
        "{}",
        lines[4]
    );
    assert!(
        md.contains("| jarambit | None / None | None |  |  |"),
        "{md}"
    );
    let summary = r1
        .report
        .as_obj()
        .and_then(|o| o.get("summary"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(keys(&summary).last().map(String::as_str), Some("must_act"));
    assert!(md.contains(&format!("## Findings ({})", py::py_repr(&summary))));
    assert!(md.ends_with('\n'));
    let state = std::fs::read_to_string(format!("{out}/state.json")).expect("state");
    let state_j = py::loads(&state).expect("state json");
    assert_eq!(keys(&state_j), ["hashes", "progress", "last_utc"]);

    // ---- second run 35 minutes later: same screens, same counts
    let h2 = host("2026-10-21T16:05:00+00:00");
    let r2 = run_audit(&cfg, &h2, &opts).expect("run 2");
    let c2 = codes(&r2.report);
    for want in [
        "NO_PROGRESS",
        "SCREEN_FROZEN",
        "MANAGED_SCREEN_FROZEN",
        "SC_STALE",
    ] {
        assert!(c2.contains(&want.to_string()), "{want} missing from {c2:?}");
    }
    let progress = r2
        .report
        .as_obj()
        .and_then(|o| o.get("progress"))
        .and_then(|p| p.as_obj())
        .and_then(|p| p.get("jopedime"))
        .cloned();
    let progress = progress.expect("progress entry");
    assert_eq!(
        progress.as_obj().and_then(|o| o.get("baseline_utc")),
        Some(&J::from("2026-10-21T15:30:00+00:00"))
    );
    assert_eq!(
        progress
            .as_obj()
            .and_then(|o| o.get("merged_prs_since_baseline")),
        Some(&J::Null)
    );
    let jsonl = std::fs::read_to_string(format!("{out}/audit.jsonl")).expect("jsonl");
    let rows: Vec<J> = jsonl.lines().map(|l| py::loads(l).expect("line")).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(keys(&rows[0]), ["utc", "summary", "findings", "done"]);
    assert!(jsonl.lines().all(|l| l.starts_with("{\"utc\": ")));
    // the hardcopy hashes survive between runs (screens unchanged for 35 min)
    let screens = r2
        .report
        .as_obj()
        .and_then(|o| o.get("screens"))
        .cloned()
        .unwrap_or_default();
    let jope = screens
        .as_obj()
        .and_then(|o| o.get("captain_jopedime"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        jope.as_obj().and_then(|o| o.get("unchanged_min")),
        Some(&J::Int(35))
    );
    // every bounded child stays within the Python bounds (jarambit's `git show` was unbounded in Python too)
    assert!(
        h2.timeouts
            .borrow()
            .iter()
            .flatten()
            .all(|d| *d <= Duration::from_secs(90))
    );
}
