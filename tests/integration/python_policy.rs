//! Enforce the maintainer's bounded legacy-Python exception during onboarding.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

fn rows(document: &str) -> Vec<Vec<&str>> {
    document
        .lines()
        .filter_map(|line| {
            let cells: Vec<_> = line.split('|').map(str::trim).collect();
            (cells.len() > 3 && cells[1].starts_with('`') && cells[1].ends_with(".py`"))
                .then(|| cells[1..cells.len() - 1].to_vec())
        })
        .collect()
}

fn validate(
    actual: &BTreeMap<String, usize>,
    allowed: &BTreeSet<String>,
    exceptions: &str,
    plan: &str,
) -> Result<(), String> {
    let mut justifications = BTreeMap::new();
    for row in rows(exceptions) {
        let path = row[0].trim_matches('`').to_owned();
        if row[1].is_empty() || justifications.insert(path.clone(), row[1]).is_some() {
            return Err(format!("missing or duplicate Python exception: {path}"));
        }
    }
    let mut inventory = BTreeMap::new();
    for row in rows(plan) {
        let path = row[0].trim_matches('`').to_owned();
        let count = row[1]
            .replace(',', "")
            .parse::<usize>()
            .map_err(|_| format!("invalid Python line count: {path}"))?;
        if row.len() != 5
            || row[2..].iter().any(|cell| cell.is_empty())
            || inventory.insert(path.clone(), count).is_some()
        {
            return Err(format!(
                "incomplete or duplicate Python migration row: {path}"
            ));
        }
    }
    for path in actual.keys() {
        if !allowed.contains(path) {
            return Err(format!(
                "net-new Python requires explicit maintainer exception: {path}"
            ));
        }
    }
    if actual.keys().collect::<Vec<_>>() != justifications.keys().collect::<Vec<_>>() {
        return Err(
            "Python exceptions must match existing files exactly (no missing or stale rows)".into(),
        );
    }
    if actual != &inventory {
        return Err(
            "Python migration inventory must match existing paths and physical line counts".into(),
        );
    }
    Ok(())
}

#[test]
fn existing_python_has_complete_exceptions_and_no_unapproved_growth() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .current_dir(&root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ":(icase)*.py",
            ":(icase)*.pyi",
            ":(icase)*.pyw",
        ])
        .output()
        .expect("enumerate Python without a shell");
    assert!(output.status.success(), "git file inventory failed");
    let mut actual = BTreeMap::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(bytes).expect("UTF-8 source path");
        let file = root.join(path);
        assert!(
            fs::symlink_metadata(&file)
                .expect("source metadata")
                .file_type()
                .is_file(),
            "Python source must be a regular file: {path}"
        );
        let count = fs::read_to_string(file)
            .expect("UTF-8 Python source")
            .lines()
            .count();
        actual.insert(path.to_owned(), count);
    }
    let allowed: BTreeSet<_> = include_str!("../fixtures/python-baseline.txt")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    validate(
        &actual,
        &allowed,
        &fs::read_to_string(root.join("PYTHON_EXCEPTIONS.md")).expect("exception policy"),
        &fs::read_to_string(root.join("PY_TO_RUST_PLAN.md")).expect("migration policy"),
    )
    .expect("maintainer Python policy must remain enforced");
}

#[test]
fn guard_rejects_new_python_even_if_it_adds_its_own_exception() {
    let actual = BTreeMap::from([("new.py".into(), 1)]);
    assert!(
        validate(
            &actual,
            &BTreeSet::new(),
            "| `new.py` | self-approved |",
            "| `new.py` | 1 | tool | retire | ADM-PY02 |"
        )
        .expect_err("a new Python file cannot approve itself")
        .contains("net-new Python")
    );
}

#[test]
fn guard_rejects_missing_stale_blank_duplicate_and_wrong_count_rows() {
    let actual = BTreeMap::from([("old.py".into(), 2)]);
    let allowed = BTreeSet::from(["old.py".into()]);
    let exception = "| `old.py` | retained for parity |";
    let plan = "| `old.py` | 2 | tool | retire | ADM-PY02 |";
    assert!(validate(&actual, &allowed, exception, plan).is_ok());
    for bad in [
        "".to_owned(),
        "| `old.py` | |".into(),
        format!("{exception}\n{exception}"),
        format!("{exception}\n| `gone.py` | stale |"),
    ] {
        assert!(validate(&actual, &allowed, &bad, plan).is_err());
    }
    for bad in [
        "".to_owned(),
        plan.replace("| 2 |", "| 3 |"),
        format!("{plan}\n{plan}"),
    ] {
        assert!(validate(&actual, &allowed, exception, &bad).is_err());
    }
    // Retiring source is allowed only when both inventories drop the same row.
    assert!(validate(&BTreeMap::new(), &allowed, "", "").is_ok());
}

#[test]
fn sc186o_token_yield_parameterized_sql_rejects_injection_metacharacters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = fs::read_to_string(root.join("dashboard/token_yield.py")).expect("token_yield.py");
    for line in source.lines() {
        if line.contains("db.execute(") && (line.contains("INSERT") || line.contains("WHERE")) {
            assert!(
                line.contains('?'),
                "expected parameterized SQL placeholder on line: {line}"
            );
        }
    }
    let dir = tempfile::Builder::new()
        .prefix("adm-ty-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let home = dir.path();
    let runtime_dir = home.join("xbabe2_kicker/captains-dashboard");
    fs::create_dir_all(&runtime_dir).expect("runtime_dir");
    fs::create_dir_all(home.join("xbabe2_kicker/captains-20261017")).expect("captains_dir");
    let sess_dir = home.join(".codex/sessions/2026/10/19");
    fs::create_dir_all(&sess_dir).expect("sess_dir");
    let evil_sid = "sid'; DROP TABLE sessions; --";
    let evil_cwd = "/tmp/cwd'; DROP TABLE files; --";
    let record = serde_json::json!({
        "timestamp": "2099-01-01T00:00:00Z",
        "type": "session_meta",
        "payload": { "id": evil_sid, "cwd": evil_cwd }
    });
    let evil_file = sess_dir.join("rollout-evil'; DROP TABLE sessions; --.jsonl");
    fs::write(&evil_file, format!("{record}\n")).expect("write rollout jsonl");

    let run = Command::new("python3")
        .arg(root.join("dashboard/token_yield.py"))
        .env("ADMIRALTY_TY_HOME", home)
        .env("CDASH_RUNTIME", &runtime_dir)
        .output()
        .expect("run token_yield.py");
    assert!(
        run.status.success(),
        "token_yield failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let conn = rusqlite::Connection::open(runtime_dir.join("tokens.db")).expect("open tokens.db");
    let mut stmt = conn
        .prepare("SELECT id, cwd FROM sessions WHERE cwd = ?1")
        .expect("prepare query");
    let rows: Vec<(String, String)> = stmt
        .query_map([evil_cwd], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query sessions")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect rows");
    assert_eq!(
        rows,
        vec![(
            "rollout-evil'; DROP TABLE sessions; --".to_owned(),
            evil_cwd.to_owned()
        )]
    );
    let (off, size): (i64, i64) = conn
        .query_row("SELECT off, size FROM files", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("query files");
    assert!(off > 0 && size > 0);
}

#[test]
fn sc186o_history_export_cron_custody_and_allowlist_against_local_bare_remote() {
    use std::os::unix::fs::PermissionsExt;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = root.join("tools/history_export_cron.sh");
    let script = fs::read_to_string(&script_path).expect("history_export_cron.sh");
    assert!(!script.contains("git clean"));
    assert!(!script.contains("git add -A"));
    assert!(!script.contains("git reset"));

    let dir = tempfile::Builder::new()
        .prefix("adm-hist-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let base = dir.path();
    fs::set_permissions(base, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    let remote = base.join("remote.git");
    let clone = base.join("clone");
    let log_file = base.join("history_export.log");
    let lock_file = base.join("history_export.lock");

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
    for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&clone)
                .args(["config", k, v])
                .status()
                .expect("git config")
                .success()
        );
    }
    fs::create_dir_all(clone.join("data/history/timeseries")).expect("mkdir timeseries");
    fs::create_dir_all(clone.join("tools")).expect("mkdir tools");
    fs::write(clone.join("README.md"), "root\n").expect("write README");
    fs::write(clone.join("data/history/README.md"), "hist\n").expect("write hist README");
    fs::write(clone.join("data/history/status.jsonl"), "{}\n").expect("write status");
    fs::write(clone.join("data/history/summary.json"), "{}\n").expect("write summary");
    fs::write(
        clone.join("data/history/rates_index.json"),
        "{\"files\":[]}\n",
    )
    .expect("write rates");
    fs::write(
        clone.join("tools/export_history.py"),
        concat!(
            "import os, pathlib, sys\n",
            "out = pathlib.Path(sys.argv[sys.argv.index('--out') + 1])\n",
            "mode = os.environ.get('TEST_EXPORT_MODE', 'ok')\n",
            "if mode == 'fail':\n",
            "    (out / 'status.jsonl').write_text('corrupted\\n')\n",
            "    (out / 'status.jsonl.tmp').write_text('scratch\\n')\n",
            "    sys.exit(2)\n",
            "(out / 'status.jsonl').write_text('{\"exported\":true}\\n')\n",
            "(out / 'timeseries/history_2026-10-19.jsonl').write_text('{\"ts_utc\":\"2026-10-19T23:00:00Z\"}\\n')\n",
        ),
    )
    .expect("write synthetic exporter");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["add", "."])
            .status()
            .expect("git add")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["commit", "-m", "init"])
            .status()
            .expect("git commit")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["push", "-u", "origin", "main"])
            .status()
            .expect("git push")
            .success()
    );

    let remote_head = || -> String {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("rev-parse remote");
        String::from_utf8(out.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    };
    let run_wrapper = |mode: &str| {
        Command::new("bash")
            .arg(&script_path)
            .env("ADMIRALTY_HISTORY_CLONE", &clone)
            .env("ADMIRALTY_HISTORY_LOG", &log_file)
            .env("ADMIRALTY_HISTORY_LOCK", &lock_file)
            .env("TEST_EXPORT_MODE", mode)
            .output()
            .expect("run history_export_cron.sh")
    };
    let head0 = remote_head();

    // Case 1: Unrelated pre-existing staged data/history file -> refuses without resetting index/worktree/remote
    let unrelated = clone.join("data/history/unrelated-private-synthetic.json");
    fs::write(&unrelated, "{\"private\":true}\n").expect("write unrelated");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["add", "data/history/unrelated-private-synthetic.json"])
            .status()
            .expect("stage unrelated")
            .success()
    );
    assert!(!run_wrapper("ok").status.success());
    assert_eq!(remote_head(), head0);
    let staged = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .expect("diff cached");
    assert_eq!(
        String::from_utf8_lossy(&staged.stdout).trim(),
        "data/history/unrelated-private-synthetic.json"
    );
    assert_eq!(
        fs::read_to_string(&unrelated).expect("read unrelated"),
        "{\"private\":true}\n"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args([
                "rm",
                "-f",
                "-q",
                "data/history/unrelated-private-synthetic.json"
            ])
            .status()
            .expect("cleanup unrelated")
            .success()
    );

    // Case 2: Pre-existing staged file outside data/history -> refuses without resetting index/worktree/remote
    fs::write(clone.join("README.md"), "staged-outside\n").expect("write README");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["add", "README.md"])
            .status()
            .expect("stage README")
            .success()
    );
    assert!(!run_wrapper("ok").status.success());
    assert_eq!(remote_head(), head0);
    let staged = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["diff", "--cached", "--name-only"])
        .output()
        .expect("diff cached");
    assert_eq!(String::from_utf8_lossy(&staged.stdout).trim(), "README.md");
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).expect("read README"),
        "staged-outside\n"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["checkout", "HEAD", "--", "README.md"])
            .status()
            .expect("restore README")
            .success()
    );

    // Case 3: Pre-existing dirty output file under data/history -> refuses and preserves dirty worktree
    fs::write(
        clone.join("data/history/status.jsonl"),
        "{\"dirty_local\":true}\n",
    )
    .expect("dirty status");
    assert!(!run_wrapper("ok").status.success());
    assert_eq!(remote_head(), head0);
    assert_eq!(
        fs::read_to_string(clone.join("data/history/status.jsonl")).expect("read status"),
        "{\"dirty_local\":true}\n"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["checkout", "HEAD", "--", "data/history/status.jsonl"])
            .status()
            .expect("restore status")
            .success()
    );

    // Case 4: Exporter failure -> exits nonzero, restores tracked data/history to HEAD, removes .tmp, leaves remote untouched
    assert!(!run_wrapper("fail").status.success());
    assert_eq!(remote_head(), head0);
    assert_eq!(
        fs::read_to_string(clone.join("data/history/status.jsonl")).expect("read status"),
        "{}\n"
    );
    assert!(!clone.join("data/history/status.jsonl.tmp").exists());

    // Case 5: Clean positive export -> commits and pushes only allowlisted data/history files to local bare remote
    assert!(run_wrapper("ok").status.success());
    let head1 = remote_head();
    assert_ne!(head1, head0);
    let committed = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
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
}

#[test]
fn sc186o_security_supply_chain_fails_closed_on_missing_scanner_or_symlink_tmpdir() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = root.join("scripts/check/security-supply-chain.sh");
    let dir = tempfile::Builder::new()
        .prefix("adm-sec-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let base = dir.path();
    let empty_bin = base.join("empty_bin");
    fs::create_dir_all(&empty_bin).expect("mkdir empty_bin");
    for util in [
        "bash",
        "dirname",
        "pwd",
        "cd",
        "test",
        "stat",
        "mktemp",
        "sha256sum",
        "grep",
    ] {
        for candidate in [format!("/usr/bin/{util}"), format!("/bin/{util}")] {
            let p = Path::new(&candidate);
            if p.exists() {
                let _ = symlink(p, empty_bin.join(util));
                break;
            }
        }
    }
    let missing = Command::new("/bin/bash")
        .arg(&script_path)
        .env("ADMIRALTY_SECURITY_NO_DEFAULT_PATH", "1")
        .env("PATH", &empty_bin)
        .output()
        .expect("run security-supply-chain with missing scanners");
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no missing-tool skip allowed"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let real_tmp = base.join("real_tmp");
    fs::create_dir_all(&real_tmp).expect("mkdir real_tmp");
    fs::set_permissions(&real_tmp, fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    let sym_tmp = base.join("sym_tmp");
    symlink(&real_tmp, &sym_tmp).expect("symlink sym_tmp");
    let sym_res = Command::new("/bin/bash")
        .arg(&script_path)
        .env("TMPDIR", &sym_tmp)
        .output()
        .expect("run security-supply-chain with symlink TMPDIR");
    assert!(!sym_res.status.success());
    assert!(
        String::from_utf8_lossy(&sym_res.stderr).contains("must not be a symlink"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&sym_res.stderr)
    );
}
