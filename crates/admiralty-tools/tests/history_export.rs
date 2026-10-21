//! Golden parity for `admiralty history export` (retired `tools/export_history.py`).
//!
//! `fixtures/history_export.bundle` holds a synthetic dashboard/captains runtime,
//! a pre-existing partial dataset (union merge) and the exact bytes the Python
//! exporter produced from them with `HOME=/home/synthetic`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use admiralty_tools::history_export::{ExportOptions, export};

const HOME: &str = "/home/synthetic";

fn bundle() -> BTreeMap<String, Vec<u8>> {
    let data = include_bytes!("fixtures/history_export.bundle");
    let mut entries = BTreeMap::new();
    let mut position = 0;
    while position < data.len() {
        let end = data[position..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(data.len(), |offset| position + offset);
        let line = std::str::from_utf8(&data[position..end]).expect("header is UTF-8");
        position = end + 1;
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let header = line.strip_prefix("--- ").expect("entry header");
        let (path, length) = header.rsplit_once(' ').expect("path and length");
        let length: usize = length.parse().expect("length");
        entries.insert(path.to_owned(), data[position..position + length].to_vec());
        position += length + 1;
    }
    entries
}

fn materialize(entries: &BTreeMap<String, Vec<u8>>, prefix: &str, root: &Path) {
    for (path, bytes) in entries {
        if let Some(relative) = path.strip_prefix(prefix) {
            let target = root.join(relative);
            fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
            fs::write(&target, bytes).expect("write fixture");
        }
    }
}

fn make_tokens_db(path: &Path) {
    let connection = rusqlite::Connection::open(path).expect("tokens.db");
    connection
        .execute_batch(
            "CREATE TABLE sessions (id TEXT, tool TEXT, model TEXT, effort TEXT, cwd TEXT, host TEXT, start REAL, end REAL, input INTEGER, cached INTEGER, output INTEGER, reasoning INTEGER, credits REAL);
             CREATE TABLE claude_msgs (ts REAL, model TEXT, cwd TEXT, input INTEGER, cached INTEGER, output INTEGER);",
        )
        .expect("schema");
    let joil = format!("{HOME}/joil/x");
    let none_text: Option<&str> = None;
    let none_int: Option<i64> = None;
    let none_real: Option<f64> = None;
    let sessions: [[&dyn rusqlite::ToSql; 13]; 4] = [
        [
            &"s1",
            &"codex",
            &"gpt-6-luna",
            &"medium",
            &joil,
            &"xbabe2",
            &1_760_760_000.5,
            &1_760_763_600_i64,
            &100_i64,
            &50_i64,
            &20_i64,
            &5_i64,
            &1.23456,
        ],
        [
            &"s2",
            &"claude",
            &none_text,
            &none_text,
            &"/srv/Jope-Dime",
            &"xbabe1",
            &none_real,
            &0_i64,
            &none_int,
            &none_int,
            &none_int,
            &none_int,
            &none_real,
        ],
        [
            &"s0",
            &"codex",
            &"m",
            &"hi",
            &none_text,
            &"xbabe3",
            &1_760_700_000_i64,
            &none_real,
            &1_i64,
            &2_i64,
            &3_i64,
            &4_i64,
            &2.0005,
        ],
        [
            &"s3",
            &"codex",
            &"m",
            &"hi",
            &"/x/JOPEDIME",
            &"xbabe3",
            &1_760_760_000.5,
            &none_real,
            &1_i64,
            &2_i64,
            &3_i64,
            &4_i64,
            &7_i64,
        ],
    ];
    for row in &sessions {
        connection
            .execute(
                "INSERT INTO sessions VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                row.as_slice(),
            )
            .expect("session row");
    }
    let dope_a = format!("{HOME}/dope/a");
    let dope_b = format!("{HOME}/dope/b");
    let messages: [[&dyn rusqlite::ToSql; 6]; 5] = [
        [
            &1_760_760_000_i64,
            &"opus",
            &dope_a,
            &10_i64,
            &1_i64,
            &2_i64,
        ],
        [
            &1_760_760_100_i64,
            &"opus",
            &dope_b,
            &5_i64,
            &none_int,
            &1_i64,
        ],
        [
            &1_760_763_700_i64,
            &none_text,
            &"/jarambit/z",
            &1_i64,
            &1_i64,
            &1_i64,
        ],
        [&none_real, &"x", &"y", &1_i64, &1_i64, &1_i64],
        [&1_760_760_050_i64, &"", &"/joil", &2.5, &0_i64, &0_i64],
    ];
    for row in &messages {
        connection
            .execute(
                "INSERT INTO claude_msgs VALUES (?,?,?,?,?,?)",
                row.as_slice(),
            )
            .expect("message row");
    }
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory)
            .expect("listing")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative")
                    .display()
                    .to_string();
                files.insert(relative, fs::read(&path).expect("read"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

struct Workspace {
    _temporary: tempfile::TempDir,
    options: ExportOptions,
    expected_out: BTreeMap<String, Vec<u8>>,
    expected_stdout: Vec<u8>,
}

fn workspace() -> Workspace {
    let entries = bundle();
    let temporary = tempfile::tempdir().expect("tempdir");
    let root = temporary.path();
    materialize(&entries, "input/", root);
    make_tokens_db(&root.join("dash").join("tokens.db"));
    // Hidden names never match Python's glob.
    fs::write(
        root.join("repo/captains/rates/.hidden_rates_x.json"),
        "not json",
    )
    .expect("hidden rates");
    let expected_out = entries
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("expected/out/")
                .map(|relative| (relative.to_owned(), bytes.clone()))
        })
        .collect();
    let expected_stdout = entries
        .get("expected/stdout.txt")
        .cloned()
        .expect("expected stdout");
    let options = ExportOptions {
        out: root.join("out"),
        dash: root.join("dash"),
        capd: root.join("capd"),
        check: false,
        repo: root.join("repo"),
        home: HOME.to_owned(),
    };
    Workspace {
        _temporary: temporary,
        options,
        expected_out,
        expected_stdout,
    }
}

fn run(options: &ExportOptions) -> (i32, Vec<u8>, Vec<u8>) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = export(options, &mut stdout, &mut stderr).expect("export runs");
    (code, stdout, stderr)
}

fn out_dir(options: &ExportOptions) -> PathBuf {
    options.out.clone()
}

/// Ported from tools/export_history.py: same files, columns, row order, bytes and summary.
#[test]
fn export_matches_python_golden_bytes_and_summary() {
    let workspace = workspace();
    let (code, stdout, stderr) = run(&workspace.options);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
    assert_eq!(
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&workspace.expected_stdout)
    );
    let produced = tree(&out_dir(&workspace.options));
    assert_eq!(
        produced.keys().collect::<Vec<_>>(),
        workspace.expected_out.keys().collect::<Vec<_>>()
    );
    for (path, bytes) in &workspace.expected_out {
        assert_eq!(
            String::from_utf8_lossy(&produced[path]),
            String::from_utf8_lossy(bytes),
            "{path}"
        );
    }
}

/// Ported from tools/export_history.py: re-running over the merged dataset is idempotent.
#[test]
fn export_rerun_is_idempotent() {
    let workspace = workspace();
    let (first_code, first_stdout, _) = run(&workspace.options);
    let first = tree(&out_dir(&workspace.options));
    let (second_code, second_stdout, _) = run(&workspace.options);
    assert_eq!((first_code, second_code), (0, 0));
    assert_eq!(first_stdout, second_stdout);
    assert_eq!(first, tree(&out_dir(&workspace.options)));
}

/// Ported from tools/export_history.py: home paths are rewritten and never exported.
#[test]
fn export_rewrites_home_paths() {
    let workspace = workspace();
    run(&workspace.options);
    for (path, bytes) in tree(&out_dir(&workspace.options)) {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(HOME), "{path} leaks the home path");
    }
    let ledger = fs::read_to_string(out_dir(&workspace.options).join("ledger/model_usage.csv"))
        .expect("ledger");
    assert!(ledger.contains("ok ~/x"));
}

/// Ported from tools/export_history.py: any secret-pattern hit aborts with exit code 3.
#[test]
fn secret_scan_hit_exits_three_with_python_tuple_lines() {
    let workspace = workspace();
    let out = out_dir(&workspace.options);
    fs::create_dir_all(out.join("extra")).expect("extra");
    fs::write(
        out.join("extra/leak.txt"),
        format!("token ghp_abcdefghijklmnopqrstuvwxyz0123\r\nkey: password = \"abcdefghijklmnop1234\"\nAuthorization: Bearer abcdefghijklmnop\n{HOME}/leak\n"),
    )
    .expect("leak");
    fs::write(
        out.join("z.txt"),
        "-----BEGIN RSA PRIVATE KEY-----\nAKIAABCDEFGHIJKLMNOP\n",
    )
    .expect("key");
    let (code, stdout, stderr) = run(&workspace.options);
    assert_eq!(code, 3);
    let stdout = String::from_utf8_lossy(&stdout);
    assert!(stdout.starts_with("{\"timeseries\": "));
    assert!(!stdout.contains("secret scan: clean"));
    let stderr = String::from_utf8_lossy(&stderr);
    for line in [
        "SECRET-SCAN HIT ('extra/leak.txt', 'gh[pousr]_[A-Za-z0-9]{20,}', 6)",
        "SECRET-SCAN HIT ('extra/leak.txt', '(?i)\\\\b(api[_-]?key|secret|passwd|passwor', 46)",
        "SECRET-SCAN HIT ('extra/leak.txt', 'absolute home path', 119)",
        "SECRET-SCAN HIT ('z.txt', '\\\\bAKIA[0-9A-Z]{16}\\\\b', 32)",
        "SECRET-SCAN HIT ('z.txt', '-----BEGIN [A-Z ]*PRIVATE KEY-----', 0)",
    ] {
        assert!(
            stderr.lines().any(|candidate| candidate == line),
            "{line}\n{stderr}"
        );
    }
    let check = ExportOptions {
        check: true,
        ..workspace.options.clone()
    };
    let (code, stdout, _) = run(&check);
    assert_eq!(code, 3);
    assert!(stdout.is_empty(), "--check writes no summary");
}

/// Ported from tools/export_history.py: `--check` only scans and reports clean.
#[test]
fn check_mode_only_scans() {
    let workspace = workspace();
    let check = ExportOptions {
        check: true,
        ..workspace.options.clone()
    };
    let before = tree(&out_dir(&check));
    let (code, stdout, stderr) = run(&check);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8_lossy(&stdout), "secret scan: clean\n");
    assert!(stderr.is_empty());
    assert_eq!(before, tree(&out_dir(&check)), "--check never writes");
}
