//! End-to-end checks of `xtask security` through the built binary.

use std::{fs, path::Path, process::Command};

/// Ported from tests/integration/python_policy.rs::sc186o_security_supply_chain_fails_closed_on_missing_scanner_or_symlink_tmpdir
/// (which drove `scripts/check/security-supply-chain.sh`; it now drives `xtask security`).
#[test]
fn sc186o_security_supply_chain_fails_closed_on_missing_scanner_or_symlink_tmpdir() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let xtask = env!("CARGO_BIN_EXE_xtask");
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
    let missing = Command::new(xtask)
        .arg("security")
        .env("ADMIRALTY_SECURITY_NO_DEFAULT_PATH", "1")
        .env("PATH", &empty_bin)
        .output()
        .expect("run xtask security with missing scanners");
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
    let sym_res = Command::new(xtask)
        .arg("security")
        .env("TMPDIR", &sym_tmp)
        // Scratch validation must not depend on installed scanner availability.
        .env("ADMIRALTY_SECURITY_NO_DEFAULT_PATH", "1")
        .env("PATH", &empty_bin)
        .output()
        .expect("run xtask security with symlink TMPDIR");
    assert!(!sym_res.status.success());
    assert!(
        String::from_utf8_lossy(&sym_res.stderr).contains("must not be a symlink"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&sym_res.stderr)
    );
}

#[test]
fn security_reports_the_first_missing_tool_by_name_and_exits_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("security")
        .env("ADMIRALTY_SECURITY_NO_DEFAULT_PATH", "1")
        .env("PATH", dir.path())
        .env("TMPDIR", dir.path())
        .output()
        .expect("run xtask security");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "[security-supply-chain] FAIL: required tool 'cargo' is missing from PATH (no missing-tool skip allowed)\n"
    );
}
