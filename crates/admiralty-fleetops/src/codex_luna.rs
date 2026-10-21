//! Port of `tools/codex-luna.sh`: validate one existing working directory,
//! then replace this process with
//! `codex -m gpt-6-luna -c model_reasoning_effort=medium -C <canonical dir>`.
//! Exit codes: 64 for usage / directory errors, 127 when `codex` is not on
//! `PATH`. The interactive TUI inherits this process's stdio.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::system::{exec_replace, find_in_path};

pub const EX_USAGE: i32 = 64;
pub const EX_NOT_FOUND: i32 = 127;
const NAME: &str = "admiralty fleet codex-luna";

/// Everything needed to exec codex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LunaLaunch {
    pub codex: PathBuf,
    pub working_dir: PathBuf,
    pub args: Vec<OsString>,
}

/// A refusal: exit code and the stderr line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LunaError {
    pub code: i32,
    pub message: String,
}

fn fail(code: i32, message: String) -> LunaError {
    LunaError { code, message }
}

/// Validate arguments in the script's order and resolve `codex` on `path_var`.
pub fn prepare(
    args: &[OsString],
    path_var: &OsStr,
    model: &str,
    effort: &str,
) -> Result<LunaLaunch, LunaError> {
    if args.len() != 1 {
        return Err(fail(
            EX_USAGE,
            format!("usage: {NAME} EXISTING_WORKING_DIRECTORY"),
        ));
    }
    let dir = &args[0];
    if dir.as_bytes().starts_with(b"-") {
        return Err(fail(EX_USAGE, format!("{NAME}: options are not accepted")));
    }
    let as_path = PathBuf::from(dir);
    if dir.is_empty() || !as_path.is_dir() {
        return Err(fail(
            EX_USAGE,
            format!("{NAME}: working directory must already exist"),
        ));
    }
    // `cd -P DIR && pwd -P`: canonical path, and the directory must be
    // searchable.
    let canonical = std::fs::canonicalize(&as_path)
        .ok()
        .filter(|p| rustix::fs::access(p, rustix::fs::Access::EXEC_OK).is_ok())
        .ok_or_else(|| {
            fail(
                EX_USAGE,
                format!("{NAME}: cannot resolve working directory"),
            )
        })?;
    let codex = find_in_path(OsStr::new("codex"), path_var).ok_or_else(|| {
        fail(
            EX_NOT_FOUND,
            format!("{NAME}: codex executable was not found on PATH"),
        )
    })?;
    let args = vec![
        OsString::from("-m"),
        OsString::from(model),
        OsString::from("-c"),
        OsString::from(format!("model_reasoning_effort={effort}")),
        OsString::from("-C"),
        canonical.as_os_str().to_owned(),
    ];
    Ok(LunaLaunch {
        codex,
        working_dir: canonical,
        args,
    })
}

/// Validate and exec. Only returns on refusal or exec failure.
pub fn run(args: &[OsString], model: &str, effort: &str, err: &mut dyn Write) -> i32 {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    match prepare(args, &path_var, model, effort) {
        Err(e) => {
            let _ = writeln!(err, "{}", e.message);
            e.code
        }
        Ok(launch) => {
            let _ = err.flush();
            let _ = std::io::stdout().flush();
            let e = exec_replace(&launch.codex, "codex", &launch.args, &launch.working_dir);
            let _ = writeln!(err, "{NAME}: cannot execute codex: {e}");
            if e.kind() == std::io::ErrorKind::NotFound {
                EX_NOT_FOUND
            } else {
                126
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output};

    use super::*;

    const CHILD_ENV: &str = "ADMIRALTY_FLEETOPS_CODEX_LUNA_CHILD_ARGS";

    /// Re-exec target: when the parent test sets `CHILD_ENV`, run the real
    /// launcher (which execs the mock codex) and exit with its status.
    #[test]
    fn exec_child_helper() {
        let Some(raw) = std::env::var_os(CHILD_ENV) else {
            return;
        };
        let args: Vec<OsString> = raw
            .as_bytes()
            .split(|b| *b == 0x1f)
            .filter(|a| !a.is_empty())
            .map(|a| OsStr::from_bytes(a).to_owned())
            .collect();
        let args = if raw.as_bytes() == b"\x1e" {
            Vec::new()
        } else {
            args
        };
        let code = run(&args, "gpt-6-luna", "medium", &mut std::io::stderr());
        std::process::exit(code);
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
        let encoded = if args.is_empty() {
            "\x1e".to_string()
        } else {
            args.join("\x1f")
        };
        Command::new(std::env::current_exe().expect("test binary path"))
            .args([
                "--exact",
                "codex_luna::tests::exec_child_helper",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(CHILD_ENV, encoded)
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

    /// Ported from crates/admiralty-runtime/tests/codex_luna_launcher.rs::pins_luna_medium_canonical_directory_and_propagates_provider_status
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

        let output = run_launcher(
            &[target.to_str().expect("UTF-8 target fixture")],
            root,
            "23",
        );
        assert_eq!(output.status.code(), Some(23), "{output:?}");
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

    /// Ported from crates/admiralty-runtime/tests/codex_luna_launcher.rs::rejects_missing_invalid_and_override_arguments_before_mock_invocation
    #[test]
    fn rejects_missing_invalid_and_override_arguments_before_mock_invocation() {
        let temp = tempfile::tempdir().expect("private test directory");
        let root = temp.path();
        let invalid = root.join("missing directory");
        for args in [
            vec![],
            vec![invalid.to_str().expect("UTF-8 invalid directory fixture")],
            vec!["--model", "gpt-6.1-sol"],
            vec![".", "--"],
            vec!["--resume"],
        ] {
            let output = run_launcher(&args, root, "0");
            assert_eq!(output.status.code(), Some(64), "{args:?}: {output:?}");
            assert!(!root.join("argv.bin").exists());
        }
    }

    #[test]
    fn prepare_validation_order_and_messages() {
        let temp = tempfile::tempdir().expect("tempdir");
        let empty_path = OsString::new();
        let os = |s: &str| OsString::from(s);
        let err = prepare(&[], &empty_path, "m", "e").expect_err("no args");
        assert_eq!(err.code, 64);
        assert_eq!(
            err.message,
            "usage: admiralty fleet codex-luna EXISTING_WORKING_DIRECTORY"
        );
        let err = prepare(&[os("-x")], &empty_path, "m", "e").expect_err("option");
        assert_eq!(
            err.message,
            "admiralty fleet codex-luna: options are not accepted"
        );
        let err =
            prepare(&[os("/definitely/missing")], &empty_path, "m", "e").expect_err("missing");
        assert_eq!(
            err.message,
            "admiralty fleet codex-luna: working directory must already exist"
        );
        let file = temp.path().join("file");
        fs::write(&file, "x").expect("write");
        let err = prepare(&[file.into_os_string()], &empty_path, "m", "e").expect_err("file");
        assert_eq!(err.code, 64);
        let dir = temp.path().as_os_str().to_owned();
        let err = prepare(std::slice::from_ref(&dir), &empty_path, "m", "e").expect_err("no codex");
        assert_eq!(err.code, 127);
        assert_eq!(
            err.message,
            "admiralty fleet codex-luna: codex executable was not found on PATH"
        );
        let bin = temp.path().join("bin");
        mock_codex(&bin);
        let ok = prepare(
            std::slice::from_ref(&dir),
            bin.as_os_str(),
            "gpt-6-luna",
            "medium",
        )
        .expect("valid");
        let canonical = temp.path().canonicalize().expect("canonical");
        assert_eq!(ok.codex, bin.join("codex"));
        assert_eq!(ok.working_dir, canonical);
        assert_eq!(
            ok.args,
            [
                os("-m"),
                os("gpt-6-luna"),
                os("-c"),
                os("model_reasoning_effort=medium"),
                os("-C"),
                canonical.into_os_string()
            ]
        );
    }
}
