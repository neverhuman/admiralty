//! Repository language policy.
//!
//! Admiralty's stack is Rust (services, CLI, tools, automation) and
//! Vite + TypeScript + React (console, repository scripts). The last Python
//! files and logic-bearing shell scripts were ported to Rust and TypeScript on
//! 21 October 2026 (see `PY_TO_RUST_PLAN.md`). This guard keeps it that way:
//! a new Python file, or a new executable script outside the reviewed test
//! doubles and fixed-path entry points, fails the workspace tests. There is no exception list to extend;
//! changing the stack requires changing this test in review.
use std::{fs, io::Read, path::Path, process::Command};

/// Executable scripts that remain on purpose: fake provider binaries that
/// qualification tests execute in place of a real CLI. They contain no
/// repository logic.
const REVIEWED_SCRIPT_TEST_DOUBLES: &[&str] = &["tests/qualification/codex/P08Q_fake_codex.sh"];

/// Entry points that external tools look for at a fixed path. Each must stay
/// exactly this content: one `exec` of the Rust implementation, no logic.
const REVIEWED_ENTRY_POINTS: &[(&str, &str)] = &[(
    "tools/security-lane.sh",
    "#!/usr/bin/env bash\n\
     # Jankurai security-lane entry point (the `jankurai security run` default path).\n\
     # The lane itself is Rust: crates/admiralty-xtask/src/security.rs.\n\
     exec cargo xtask security \"$@\"\n",
)];

#[derive(Debug, PartialEq, Eq)]
enum Finding {
    Python,
    Script,
}

/// Classify one tracked path by extension and, for extensionless or script
/// files, by its first bytes. `#![…]` (a Rust inner attribute) is not a shebang,
/// and a Node shebang marks an in-stack TypeScript entry point
/// (`scripts/contracts/generate.ts`), not a shell script.
fn classify(path: &str, head: &[u8]) -> Option<Finding> {
    let lower = path.to_ascii_lowercase();
    if [".py", ".pyi", ".pyw", ".ipynb"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return Some(Finding::Python);
    }
    let shebang = head.starts_with(b"#!/") || head.starts_with(b"#! /");
    if shebang {
        let line = head.split(|b| *b == b'\n').next().unwrap_or_default();
        let line = String::from_utf8_lossy(line);
        if line.contains("python") {
            return Some(Finding::Python);
        }
        let interpreter = line
            .trim_start_matches("#!")
            .split_whitespace()
            .collect::<Vec<_>>();
        let node = match interpreter.as_slice() {
            [env, program, ..] if env.ends_with("/env") => *program == "node",
            [program, ..] => program.ends_with("/node"),
            [] => false,
        };
        if node
            && [".ts", ".mts", ".js", ".mjs"]
                .iter()
                .any(|ext| lower.ends_with(ext))
        {
            return None;
        }
        return Some(Finding::Script);
    }
    if [".sh", ".bash", ".zsh", ".ksh"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return Some(Finding::Script);
    }
    None
}

fn repository_files() -> (std::path::PathBuf, Vec<String>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .current_dir(&root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .expect("enumerate repository files without a shell");
    assert!(output.status.success(), "git file inventory failed");
    let files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).expect("UTF-8 repository path"))
        .collect();
    (root, files)
}

fn head(root: &Path, path: &str) -> Vec<u8> {
    let full = root.join(path);
    // Deleted-but-tracked files (a staged removal in a working tree) have no head.
    let Ok(metadata) = fs::symlink_metadata(&full) else {
        return Vec::new();
    };
    if !metadata.is_file() {
        return Vec::new();
    }
    let mut bytes = [0_u8; 128];
    let read = fs::File::open(&full)
        .and_then(|mut file| file.read(&mut bytes))
        .unwrap_or(0);
    bytes[..read].to_vec()
}

#[test]
fn repository_contains_no_python_and_only_reviewed_script_test_doubles() {
    let (root, files) = repository_files();
    assert!(!files.is_empty(), "repository inventory must not be empty");
    let mut python = Vec::new();
    let mut scripts = Vec::new();
    for path in &files {
        if !root.join(path).exists() {
            continue;
        }
        match classify(path, &head(&root, path)) {
            Some(Finding::Python) => python.push(path.clone()),
            Some(Finding::Script)
                if !REVIEWED_SCRIPT_TEST_DOUBLES.contains(&path.as_str())
                    && !REVIEWED_ENTRY_POINTS.iter().any(|(p, _)| p == path) =>
            {
                scripts.push(path.clone())
            }
            _ => {}
        }
    }
    assert!(
        python.is_empty(),
        "Python is not part of Admiralty's stack (Rust + Vite/TypeScript/React); port these: {python:?}"
    );
    assert!(
        scripts.is_empty(),
        "repository logic belongs in Rust (`cargo xtask`, `admiralty …`) or TypeScript, not shell: {scripts:?}"
    );
    for allowed in REVIEWED_SCRIPT_TEST_DOUBLES {
        assert!(
            files.iter().any(|path| path == allowed),
            "stale reviewed test double entry: {allowed}"
        );
    }
    for (path, expected) in REVIEWED_ENTRY_POINTS {
        let actual = fs::read_to_string(root.join(path)).expect("reviewed entry point exists");
        assert_eq!(
            &actual, expected,
            "{path} must stay a one-line exec of the Rust implementation"
        );
    }
}

#[test]
fn retired_python_exception_inventory_is_not_reintroduced() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for retired in ["PYTHON_EXCEPTIONS.md", "tests/fixtures/python-baseline.txt"] {
        assert!(
            !root.join(retired).exists(),
            "{retired} was retired with the last Python file; do not restore an exception list"
        );
    }
}

#[test]
fn classifier_finds_python_and_scripts_by_extension_and_shebang() {
    assert_eq!(classify("tools/x.py", b""), Some(Finding::Python));
    assert_eq!(classify("Notebook.IPYNB", b"{}"), Some(Finding::Python));
    assert_eq!(
        classify("tools/bin/kick", b"#!/usr/bin/env python3\nprint(1)\n"),
        Some(Finding::Python)
    );
    assert_eq!(
        classify("tools/bin/kick", b"#!/usr/bin/env bash\nset -e\n"),
        Some(Finding::Script)
    );
    assert_eq!(
        classify("scripts/x.sh", b"echo hi\n"),
        Some(Finding::Script)
    );
    assert_eq!(classify("hook", b"#! /bin/sh\n"), Some(Finding::Script));
    // Node entry points in the TypeScript tree are in stack; a Node shebang on
    // an extensionless or shell file is still an executable script.
    assert_eq!(
        classify("scripts/demos/capture.ts", b"#!/usr/bin/env node\n"),
        None
    );
    assert_eq!(
        classify("tools/run", b"#!/usr/bin/env node\n"),
        Some(Finding::Script)
    );
    assert_eq!(
        classify("tools/x.sh", b"#!/usr/bin/env node\n"),
        Some(Finding::Script)
    );
    assert_eq!(
        classify("tools/x.ts", b"#!/usr/bin/env bash\n"),
        Some(Finding::Script)
    );
    // Rust inner attributes and ordinary sources are not scripts.
    assert_eq!(classify("tests/integration/a.rs", b"#![cfg(unix)]\n"), None);
    assert_eq!(
        classify("scripts/contracts/generate.ts", b"import x from 'y';\n"),
        None
    );
    assert_eq!(classify("README.md", b"# Admiralty\n"), None);
}
