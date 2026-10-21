//! Custody and allowlist tests for `admiralty history publish` (retired
//! `tools/history_export_cron.sh`), driven against a local bare remote with an
//! injected in-process exporter that simulates success and failure exactly
//! like the synthetic `export_history.py` of the original shell test.

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use admiralty_tools::ToolResult;
use admiralty_tools::history_export::ExportOptions;
use admiralty_tools::history_publish::{PublishConfig, is_allowed_export_path, publish};

fn lookup_for(clone: &Path, log: &Path, lock: &Path) -> impl Fn(&str) -> Option<String> {
    let clone = clone.display().to_string();
    let log = log.display().to_string();
    let lock = lock.display().to_string();
    move |key: &str| match key {
        "ADMIRALTY_HISTORY_CLONE" => Some(clone.clone()),
        "ADMIRALTY_HISTORY_LOG" => Some(log.clone()),
        "ADMIRALTY_HISTORY_LOCK" => Some(lock.clone()),
        _ => None,
    }
}

/// The publisher never cleans, resets or stages the whole tree.
#[test]
fn publisher_source_has_no_destructive_or_unbounded_git() {
    let source = include_str!("../src/history_publish.rs");
    for forbidden in [
        "git clean",
        "git add -A",
        "git reset",
        "\"clean\"",
        "\"reset\"",
        "\"-A\"",
        "\"--all\"",
    ] {
        assert!(!source.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn allowlist_is_exact() {
    for allowed in [
        "data/history/README.md",
        "data/history/rates_index.json",
        "data/history/status.jsonl",
        "data/history/summary.json",
        "data/history/quota/codex_quota.csv",
        "data/history/resources/disk_used.csv",
        "data/history/resources/host_snapshots.csv",
        "data/history/tokens/codex_sessions.csv",
        "data/history/tokens/claude_hourly.csv",
        "data/history/timeseries/history.csv",
        "data/history/timeseries/history_2026-10-19.jsonl",
        "data/history/ledger/model_usage.csv",
        "data/history/ledger/model_usage_2026-10-19.jsonl",
        "data/history/token_yield/token_yield_2026-10-19.jsonl",
        "data/history/snapshots/projects_2026-10-19.jsonl",
    ] {
        assert!(is_allowed_export_path(allowed), "{allowed}");
    }
    for refused in [
        "README.md",
        "data/history/unrelated-private-synthetic.json",
        "data/history/status.jsonl.tmp",
        "data/history/timeseries/history_2026-10-1.jsonl",
        "data/history/timeseries/history_2026-10-19.jsonl/x",
        "data/history/timeseries/.history_2026-10-19.tmp",
        "data/history/../README.md",
        "\"data/history/status.jsonl\"",
        "data/history/snapshots/projects_YYYY-MM-DD.jsonl",
    ] {
        assert!(!is_allowed_export_path(refused), "{refused}");
    }
}

#[test]
fn environment_overrides_match_shell_defaults() {
    let config = PublishConfig::from_lookup(&|_| None);
    assert_eq!(
        config.clone,
        PathBuf::from("/home/ubuntu/.admiralty-history")
    );
    assert_eq!(
        config.log,
        PathBuf::from("/home/ubuntu/xbabe2_kicker/captains-dashboard/history_export.log")
    );
    assert_eq!(
        config.lock,
        PathBuf::from("/tmp/admiralty_history_export.lock")
    );
    assert_eq!(config.home, "/home/ubuntu");
    assert_eq!(
        config.path,
        "/home/ubuntu/.local/bin:/usr/local/bin:/usr/bin:/bin"
    );
    // `${VAR:-default}`: empty values keep the default.
    let empty =
        PublishConfig::from_lookup(&|key| (key == "ADMIRALTY_HISTORY_CLONE").then(String::new));
    assert_eq!(empty.clone, config.clone);
    let lookup = lookup_for(Path::new("/c"), Path::new("/l"), Path::new("/k"));
    let custom = PublishConfig::from_lookup(&lookup);
    assert_eq!(
        (custom.clone, custom.log, custom.lock),
        (
            PathBuf::from("/c"),
            PathBuf::from("/l"),
            PathBuf::from("/k")
        )
    );
    let options = config.export_options();
    assert_eq!(options.out, config.clone.join("data/history"));
    assert_eq!(options.repo, config.clone);
    assert_eq!(options.home, "/home/ubuntu");
    assert_eq!(
        options.capd,
        PathBuf::from("/home/ubuntu/xbabe2_kicker/captains-20261017")
    );
}

fn refusal_case(config: &mut PublishConfig) -> (i32, String) {
    let mut stderr = Vec::new();
    let mut exporter = |_: &ExportOptions, _: &mut dyn Write| -> ToolResult<i32> {
        panic!("exporter must not run")
    };
    config.lower_priority = false;
    let code = publish(config, &mut exporter, &mut stderr).expect("publish");
    (code, String::from_utf8_lossy(&stderr).into_owned())
}

/// Injected-host variant of `linux_history_export_refuses_unsupported_host_before_any_git_or_lock_effect`
/// that runs on every platform.
#[test]
fn unsupported_host_is_refused_before_any_git_or_lock_effect() {
    let directory = tempfile::tempdir().expect("private history fixture");
    let log = directory.path().join("history.log");
    let lock = directory.path().join("history.lock");
    let clone = directory.path().join("must-not-be-created");
    let mut config = PublishConfig::from_lookup(&lookup_for(&clone, &log, &lock));
    config.host_os = "macos".to_owned();
    let (code, stderr) = refusal_case(&mut config);
    assert_ne!(code, 0);
    assert!(stderr.contains("requires Linux"));
    assert!(!clone.exists());
    assert!(!log.exists());
    assert!(!lock.exists());
}

#[test]
fn missing_required_tool_is_refused_before_any_lock_effect() {
    let directory = tempfile::tempdir().expect("private history fixture");
    let log = directory.path().join("history.log");
    let lock = directory.path().join("history.lock");
    let clone = directory.path().join("must-not-be-created");
    let mut config = PublishConfig::from_lookup(&lookup_for(&clone, &log, &lock));
    config.host_os = "linux".to_owned();
    config.required_tools = vec!["admiralty-definitely-missing-tool".to_owned()];
    let (code, stderr) = refusal_case(&mut config);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("history export missing required tool: admiralty-definitely-missing-tool")
    );
    assert!(!clone.exists());
    assert!(!log.exists());
    assert!(!lock.exists());
}

/// Ported from tests/integration/python_policy.rs::linux_history_export_refuses_unsupported_host_before_any_git_or_lock_effect
#[cfg(not(target_os = "linux"))]
#[test]
fn linux_history_export_refuses_unsupported_host_before_any_git_or_lock_effect() {
    let directory = tempfile::tempdir().expect("private history fixture");
    let log = directory.path().join("history.log");
    let lock = directory.path().join("history.lock");
    let clone = directory.path().join("must-not-be-created");
    let mut config = PublishConfig::from_lookup(&lookup_for(&clone, &log, &lock));
    let (code, stderr) = refusal_case(&mut config);
    assert_ne!(code, 0);
    assert!(stderr.contains("requires Linux"));
    assert!(!clone.exists());
    assert!(!log.exists());
    assert!(!lock.exists());
}

#[cfg(target_os = "linux")]
fn git(directory: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .status()
        .expect("git")
        .success()
}

#[cfg(target_os = "linux")]
struct Remote {
    _directory: tempfile::TempDir,
    remote: PathBuf,
    clone: PathBuf,
    log: PathBuf,
    lock: PathBuf,
}

#[cfg(target_os = "linux")]
fn local_bare_remote() -> Remote {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .prefix("adm-hist-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let base = directory.path().to_path_buf();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    let remote = base.join("remote.git");
    let clone = base.join("clone");
    assert!(
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&remote)
            .status()
            .expect("git init bare")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("clone")
            .arg(&remote)
            .arg(&clone)
            .status()
            .expect("git clone")
            .success()
    );
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(git(&clone, &["config", key, value]));
    }
    fs::create_dir_all(clone.join("data/history/timeseries")).expect("mkdir timeseries");
    fs::write(clone.join("README.md"), "root\n").expect("write README");
    fs::write(clone.join("data/history/README.md"), "hist\n").expect("write hist README");
    fs::write(clone.join("data/history/status.jsonl"), "{}\n").expect("write status");
    fs::write(clone.join("data/history/summary.json"), "{}\n").expect("write summary");
    fs::write(
        clone.join("data/history/rates_index.json"),
        "{\"files\":[]}\n",
    )
    .expect("write rates");
    assert!(git(&clone, &["add", "."]));
    assert!(git(&clone, &["commit", "-m", "init"]));
    assert!(git(&clone, &["push", "-u", "origin", "main"]));
    Remote {
        log: base.join("history_export.log"),
        lock: base.join("history_export.lock"),
        _directory: directory,
        remote,
        clone,
    }
}

#[cfg(target_os = "linux")]
impl Remote {
    fn head(&self) -> String {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&self.remote)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("rev-parse remote");
        String::from_utf8(out.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    fn config(&self) -> PublishConfig {
        let mut config =
            PublishConfig::from_lookup(&lookup_for(&self.clone, &self.log, &self.lock));
        // Never renice/ionice the test harness itself.
        config.lower_priority = false;
        config
    }

    /// Publish with the synthetic exporter (`ok` or `fail`, like TEST_EXPORT_MODE).
    fn publish(&self, mode: &str) -> i32 {
        let ran = Cell::new(false);
        let mut exporter = |options: &ExportOptions, _log: &mut dyn Write| -> ToolResult<i32> {
            ran.set(true);
            let out = &options.out;
            if mode == "fail" {
                fs::write(out.join("status.jsonl"), "corrupted\n")?;
                fs::write(out.join("status.jsonl.tmp"), "scratch\n")?;
                return Ok(2);
            }
            fs::write(out.join("status.jsonl"), "{\"exported\":true}\n")?;
            fs::write(
                out.join("timeseries/history_2026-10-19.jsonl"),
                "{\"ts_utc\":\"2026-10-19T23:00:00Z\"}\n",
            )?;
            Ok(0)
        };
        let mut stderr = Vec::new();
        publish(&self.config(), &mut exporter, &mut stderr).expect("publish runs")
    }

    fn staged(&self) -> String {
        let staged = Command::new("git")
            .arg("-C")
            .arg(&self.clone)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .expect("diff cached");
        String::from_utf8_lossy(&staged.stdout).trim().to_owned()
    }
}

/// Ported from tests/integration/python_policy.rs::sc186o_history_export_cron_custody_and_allowlist_against_local_bare_remote
#[cfg(target_os = "linux")]
#[test]
fn sc186o_history_export_cron_custody_and_allowlist_against_local_bare_remote() {
    let fixture = local_bare_remote();
    let clone = &fixture.clone;
    let head0 = fixture.head();

    // Case 1: Unrelated pre-existing staged data/history file -> refuses without resetting index/worktree/remote
    let unrelated = clone.join("data/history/unrelated-private-synthetic.json");
    fs::write(&unrelated, "{\"private\":true}\n").expect("write unrelated");
    assert!(git(
        clone,
        &["add", "data/history/unrelated-private-synthetic.json"]
    ));
    assert_ne!(fixture.publish("ok"), 0);
    assert_eq!(fixture.head(), head0);
    assert_eq!(
        fixture.staged(),
        "data/history/unrelated-private-synthetic.json"
    );
    assert_eq!(
        fs::read_to_string(&unrelated).expect("read unrelated"),
        "{\"private\":true}\n"
    );
    assert!(git(
        clone,
        &[
            "rm",
            "-f",
            "-q",
            "data/history/unrelated-private-synthetic.json"
        ]
    ));

    // Case 2: Pre-existing staged file outside data/history -> refuses without resetting index/worktree/remote
    fs::write(clone.join("README.md"), "staged-outside\n").expect("write README");
    assert!(git(clone, &["add", "README.md"]));
    assert_ne!(fixture.publish("ok"), 0);
    assert_eq!(fixture.head(), head0);
    assert_eq!(fixture.staged(), "README.md");
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).expect("read README"),
        "staged-outside\n"
    );
    assert!(git(clone, &["checkout", "HEAD", "--", "README.md"]));

    // Case 3: Pre-existing dirty output file under data/history -> refuses and preserves dirty worktree
    fs::write(
        clone.join("data/history/status.jsonl"),
        "{\"dirty_local\":true}\n",
    )
    .expect("dirty status");
    assert_ne!(fixture.publish("ok"), 0);
    assert_eq!(fixture.head(), head0);
    assert_eq!(
        fs::read_to_string(clone.join("data/history/status.jsonl")).expect("read status"),
        "{\"dirty_local\":true}\n"
    );
    assert!(git(
        clone,
        &["checkout", "HEAD", "--", "data/history/status.jsonl"]
    ));

    // Case 4: Exporter failure -> exits nonzero, restores tracked data/history to HEAD, removes .tmp, leaves remote untouched
    assert_ne!(fixture.publish("fail"), 0);
    assert_eq!(fixture.head(), head0);
    assert_eq!(
        fs::read_to_string(clone.join("data/history/status.jsonl")).expect("read status"),
        "{}\n"
    );
    assert!(!clone.join("data/history/status.jsonl.tmp").exists());

    // Case 5: Clean positive export -> commits and pushes only allowlisted data/history files to local bare remote
    assert_eq!(fixture.publish("ok"), 0);
    let head1 = fixture.head();
    assert_ne!(head1, head0);
    let committed = Command::new("git")
        .arg("--git-dir")
        .arg(&fixture.remote)
        .args(["diff-tree", "--no-commit-id", "--name-only", "-r", &head1])
        .output()
        .expect("diff-tree");
    let mut files: Vec<_> = String::from_utf8_lossy(&committed.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            "data/history/status.jsonl".to_owned(),
            "data/history/timeseries/history_2026-10-19.jsonl".to_owned()
        ]
    );
    let message = Command::new("git")
        .arg("--git-dir")
        .arg(&fixture.remote)
        .args(["log", "-1", "--format=%an <%ae>%n%B", &head1])
        .output()
        .expect("log");
    let message = String::from_utf8_lossy(&message.stdout);
    assert!(message.starts_with(
        "jepsontaylor <130782313+jepsontaylor@users.noreply.github.com>\ndata(history): hourly export "
    ));
    assert!(message.contains(
        "Automated: admiralty history export via admiralty history publish (exact allowlisted data/history paths only; see data/history/README.md)."
    ));
    let log = fs::read_to_string(&fixture.log).expect("log");
    assert!(log.contains("pushed "));
    assert!(
        log.contains("export failed or scan hit - restoring clean HEAD data/history tracked files")
    );
    // An unchanged second run is a successful no-op.
    assert_eq!(fixture.publish("ok"), 0);
    assert_eq!(fixture.head(), head1);
    assert!(
        fs::read_to_string(&fixture.log)
            .expect("log")
            .contains("no data change")
    );
}

/// A contended lock is a logged, successful no-op that touches nothing else.
#[cfg(target_os = "linux")]
#[test]
fn contended_lock_is_a_logged_noop() {
    let fixture = local_bare_remote();
    let head0 = fixture.head();
    let holder = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&fixture.lock)
        .expect("lock file");
    rustix::fs::flock(&holder, rustix::fs::FlockOperation::LockExclusive).expect("hold lock");
    let mut exporter = |_: &ExportOptions, _: &mut dyn Write| -> ToolResult<i32> {
        panic!("exporter must not run under a contended lock")
    };
    let mut stderr = Vec::new();
    let code = publish(&fixture.config(), &mut exporter, &mut stderr).expect("publish");
    assert_eq!(code, 0);
    assert_eq!(fixture.head(), head0);
    let log = fs::read_to_string(&fixture.log).expect("log");
    assert!(
        log.trim_end().ends_with("skip: previous run still active"),
        "{log}"
    );
}

/// Exporter errors are failures that restore tracked data and leave the remote alone.
#[cfg(target_os = "linux")]
#[test]
fn exporter_error_restores_and_refuses_non_allowlisted_output() {
    let fixture = local_bare_remote();
    let head0 = fixture.head();
    let mut failing = |options: &ExportOptions, _: &mut dyn Write| -> ToolResult<i32> {
        fs::write(options.out.join("summary.json"), "partial\n")?;
        Err(admiralty_tools::ToolError::invalid(
            "synthetic exporter crash",
        ))
    };
    let mut stderr = Vec::new();
    assert_eq!(
        publish(&fixture.config(), &mut failing, &mut stderr).expect("publish"),
        1
    );
    assert_eq!(
        fs::read_to_string(fixture.clone.join("data/history/summary.json")).expect("summary"),
        "{}\n"
    );
    let mut stray = |options: &ExportOptions, _: &mut dyn Write| -> ToolResult<i32> {
        fs::write(options.out.join("unexpected.txt"), "stray\n")?;
        Ok(0)
    };
    assert_eq!(
        publish(&fixture.config(), &mut stray, &mut stderr).expect("publish"),
        1
    );
    assert_eq!(fixture.head(), head0);
    assert!(fixture.staged().is_empty(), "nothing staged on refusal");
    assert!(fs::read_to_string(&fixture.log).expect("log").contains(
        "refusing: exporter modified or created non-allowlisted path: data/history/unexpected.txt"
    ));
}
