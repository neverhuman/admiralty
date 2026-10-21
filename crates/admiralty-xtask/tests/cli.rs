//! Command-line surface of the `xtask` binary (argument forwarding, stage
//! listing and early validation) without running heavy stages.

use std::process::Command;

fn xtask(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .output()
        .expect("run xtask")
}

#[test]
fn check_list_prints_sixteen_numbered_stages_without_touching_the_environment() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "--list"])
        .env("ADMIRALTY_TARGET_DIR", "relative-is-not-validated-for-list")
        .env("PATH", "/nonexistent")
        .output()
        .expect("run xtask check --list");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 16);
    for (index, line) in lines.iter().enumerate() {
        assert!(line.starts_with(&format!("{:02} ", index + 1)), "{line}");
    }
    assert_eq!(lines[3], "04 cargo test --workspace --locked");
    assert!(
        lines[15]
            .starts_with("16 pnpm --dir apps/dashboard test --run --config e2e/vitest.config.ts")
    );
}

#[test]
fn check_rejects_out_of_range_and_conflicting_selections() {
    for args in [
        &["check", "--from", "0"][..],
        &["check", "--from", "17"],
        &["check", "--only", "17"],
        &["check", "--from", "2", "--only", "3"],
    ] {
        let output = xtask(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    let output = xtask(&["check", "--only", "16", "--skip-browser"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("No local CI stage selected"));
}

#[test]
fn check_refuses_relative_target_dir_before_any_stage() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "--only", "2"])
        .env("ADMIRALTY_TARGET_DIR", "relative/target")
        .output()
        .expect("run xtask check");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "ADMIRALTY_TARGET_DIR must be absolute\n"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn check_requires_the_pinned_node_major_unless_overridden() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "--only", "2"])
        .env_remove("ADMIRALTY_TARGET_DIR")
        .env_remove("ADMIRALTY_ALLOW_NODE_MAJOR")
        .env("PATH", dir.path())
        .output()
        .expect("run xtask check");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Local CI requires Node 24, matching .github/workflows/ci.yml.\n"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check", "--only", "2"])
        .env_remove("ADMIRALTY_TARGET_DIR")
        .env("ADMIRALTY_ALLOW_NODE_MAJOR", "nope")
        .output()
        .expect("run xtask check");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "ADMIRALTY_ALLOW_NODE_MAJOR must be a positive integer\n"
    );
}

#[test]
fn live_forwards_raw_arguments_and_keeps_shell_exit_codes() {
    let help = xtask(&["live", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let usage = String::from_utf8_lossy(&help.stdout);
    assert!(usage.starts_with(
        "Usage: pnpm live --status /absolute/status.json [--index /absolute/index.html]"
    ));
    assert!(help.stderr.is_empty());

    let bogus = xtask(&["live", "--bogus"]);
    assert_eq!(bogus.status.code(), Some(2));
    assert!(bogus.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bogus.stderr).starts_with("Usage: pnpm live"));

    let missing_value = xtask(&["live", "--status"]);
    assert_eq!(missing_value.status.code(), Some(2));

    for args in [
        &["live", "--status", "relative.json"][..],
        &["live", "--", "--status", "relative.json"],
        &["live", "--index", "/nonexistent/index.html"],
    ] {
        let output = xtask(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "--status must name an existing absolute producer file.\n",
            "{args:?}"
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let status = dir.path().join("status.json");
    std::fs::write(&status, "{}").expect("write status");
    let status = status.to_string_lossy().into_owned();
    let output = xtask(&[
        "live",
        "--status",
        &status,
        "--index",
        "/nonexistent/index.html",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "--index must name an existing absolute producer file.\n"
    );
    let output = xtask(&["live", "--status", &status, "--models", "models.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "--models must name an existing absolute private registry file.\n"
    );
}

#[test]
fn live_refuses_a_symlinked_state_dir_before_building_or_printing_a_token() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().expect("tempdir");
    let status = dir.path().join("status.json");
    std::fs::write(&status, "{}").expect("write status");
    let real = dir.path().join("real");
    std::fs::create_dir(&real).expect("mkdir real");
    let link = dir.path().join("link");
    symlink(&real, &link).expect("symlink");
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["live", "--status"])
        .arg(&status)
        .env("ADMIRALTY_STATE_DIR", &link)
        .env("PATH", "/nonexistent")
        .output()
        .expect("run xtask live");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Preview state directory must be absolute and not a symlink\n"
    );
    assert!(!real.join("dashboard-token").exists());
}

#[test]
fn live_creates_a_private_token_and_prints_only_its_path_before_building() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let status = dir.path().join("status.json");
    std::fs::write(&status, "{}").expect("write status");
    let state = dir.path().join("state/preview");
    let empty_path = dir.path().join("empty-bin");
    std::fs::create_dir(&empty_path).expect("mkdir empty-bin");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["live", "--status"])
            .arg(&status)
            .env("ADMIRALTY_STATE_DIR", &state)
            // No pnpm on PATH: the dashboard build step fails after state preparation.
            .env("PATH", &empty_path)
            .output()
            .expect("run xtask live")
    };
    let output = run();
    let token_path = state.join("dashboard-token");
    let token = std::fs::read_to_string(&token_path).expect("token created");
    assert_eq!(token.len(), 64);
    assert_eq!(output.status.code(), Some(127));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stdout,
        format!(
            "Dashboard access token is in {} (value not displayed)\n",
            token_path.display()
        )
    );
    assert!(!stdout.contains(&token) && !stderr.contains(&token));
    assert!(stderr.contains("cannot run pnpm"), "{stderr}");
    assert_eq!(
        std::fs::metadata(&state).expect("state").mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&token_path).expect("token").mode() & 0o777,
        0o600
    );

    let again = run();
    assert_eq!(again.status.code(), Some(127));
    assert_eq!(again.stdout, output.stdout);
    assert_eq!(
        std::fs::read_to_string(&token_path).expect("token kept"),
        token
    );
}
