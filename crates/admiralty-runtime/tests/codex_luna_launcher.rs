#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn launcher() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/codex-luna.sh")
        .canonicalize()
        .expect("launcher source path exists")
}

fn mock_codex(dir: &Path) {
    fs::create_dir_all(dir).expect("create private mock bin directory");
    let path = dir.join("codex");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$MOCK_ARGS\"\npwd -P > \"$MOCK_CWD\"\nexit \"${MOCK_EXIT:-0}\"\n",
    )
    .expect("write local mock executable");
    let mut permissions = fs::metadata(&path).expect("read mock mode").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("make mock executable");
}

fn run_launcher(args: &[&str], root: &Path, exit: &str) -> Output {
    let mock_bin = root.join("mock-bin");
    mock_codex(&mock_bin);
    let config_home = root.join("private-codex-home");
    fs::create_dir_all(&config_home).expect("create private config home");
    fs::write(
        config_home.join("config.toml"),
        "model = 'gpt-6.1-sol'\nmodel_reasoning_effort = 'xhigh'\n",
    )
    .expect("write hostile config fixture");

    Command::new(launcher())
        .args(args)
        .env_clear()
        .env("PATH", mock_bin)
        .env("CODEX_HOME", &config_home)
        .env("CODEX_MODEL", "gpt-6.1-sol")
        .env("OPENAI_MODEL", "gpt-6.1-sol")
        .env("MOCK_ARGS", root.join("argv.bin"))
        .env("MOCK_CWD", root.join("cwd.txt"))
        .env("MOCK_EXIT", exit)
        .output()
        .expect("run only the local launcher and mock")
}

#[test]
fn pins_luna_medium_canonical_directory_and_propagates_provider_status() {
    let temp = tempfile::tempdir().expect("private test directory");
    let root = temp.path();
    let target = root.join("-work area");
    fs::create_dir(&target).expect("create path with spaces and leading-hyphen basename");
    let config_home = root.join("private-codex-home");
    fs::create_dir(&config_home).expect("create private config home");
    let config = config_home.join("config.toml");
    let original_config = b"model = 'gpt-6.1-sol'\nmodel_reasoning_effort = 'xhigh'\n";
    fs::write(&config, original_config).expect("write hostile config fixture");

    let output = run_launcher(&[target.to_str().unwrap()], root, "23");
    assert_eq!(output.status.code(), Some(23));
    let recorded = fs::read(root.join("argv.bin")).expect("mock recorded argv");
    let argv: Vec<&[u8]> = recorded
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .collect();
    let canonical = target.canonicalize().expect("canonical target");
    let canonical = canonical.to_str().expect("UTF-8 test path");
    assert_eq!(
        argv,
        [
            b"-m".as_slice(),
            b"gpt-6-luna",
            b"-c",
            b"model_reasoning_effort=medium",
            b"-C",
            canonical.as_bytes(),
        ]
    );
    assert_eq!(
        fs::read_to_string(root.join("cwd.txt")).expect("mock recorded cwd"),
        format!("{canonical}\n")
    );
    assert_eq!(
        fs::read(&config).expect("config remains readable"),
        original_config
    );
}

#[test]
fn rejects_missing_invalid_and_override_arguments_before_mock_invocation() {
    let temp = tempfile::tempdir().expect("private test directory");
    let root = temp.path();
    let invalid = root.join("missing directory");
    for args in [
        vec![],
        vec![invalid.to_str().unwrap()],
        vec!["--model", "gpt-6.1-sol"],
        vec![".", "--"],
        vec!["--resume"],
    ] {
        let output = run_launcher(&args, root, "0");
        assert_eq!(output.status.code(), Some(64));
        assert!(!root.join("argv.bin").exists());
    }
}
