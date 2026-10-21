//! Private preview credentials. No service activation, enrollment or grant installation.
use crate::CliResult;
use std::path::Path;

#[cfg(unix)]
pub fn prepare(state: &Path) -> CliResult<serde_json::Value> {
    let created = !state.join("dashboard-token").exists();
    let token_file = crate::ops::init_preview_state(state)?;
    Ok(
        serde_json::json!({"schema_version":1,"state_directory":state,
        "token_file":token_file,"created":created,"service_started":false}),
    )
}

#[cfg(not(unix))]
pub fn prepare(_: &Path) -> CliResult<serde_json::Value> {
    Err("private preview initialization is not qualified on this platform".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    fn root() -> CliResult<tempfile::TempDir> {
        Ok(tempfile::Builder::new()
            .prefix("adm-preview-")
            // tempfile's default directory mode is 0777 before the caller's
            // umask. Fixtures must be private at creation on shared runners.
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize()?)?)
    }

    #[test]
    fn private_fixture_supports_a_permissive_caller_umask() -> CliResult<()> {
        // Isolate umask in a child: never change this test process's global mask.
        let output = std::process::Command::new("bash")
            .args(["-c", "umask 000; exec \"$@\"", "preview-fixture"])
            .arg(std::env::current_exe()?)
            .args([
                "--exact",
                "preview::tests::private_creation_is_idempotent_and_matches_daemon_reader",
                "--test-threads=1",
            ])
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[test]
    fn private_creation_is_idempotent_and_matches_daemon_reader() -> CliResult<()> {
        let root = root()?;
        let state = root.path().join("nested/state");
        let first = prepare(&state)?;
        let token = admiralty_server::startup::read_private_token(&state.join("dashboard-token"))?;
        assert_eq!(token.len(), 64);
        assert_eq!(fs::metadata(&state)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(state.join("dashboard-token"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(first["created"], true);
        assert_eq!(prepare(&state)?["created"], false);
        assert_eq!(
            admiralty_server::startup::read_private_token(&state.join("dashboard-token"))?,
            token
        );
        assert!(!first.to_string().contains(&token));
        Ok(())
    }

    #[test]
    fn refuses_unsafe_paths_and_preserves_existing_credentials() -> CliResult<()> {
        let root = root()?;
        let state = root.path().join("state");
        prepare(&state)?;
        let token = state.join("dashboard-token");
        let original = fs::read(&token)?;
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644))?;
        assert!(prepare(&state).is_err());
        assert_eq!(fs::read(&token)?, original);
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600))?;
        fs::hard_link(&token, state.join("alias"))?;
        assert!(prepare(&state).is_err());
        let link = root.path().join("link");
        symlink(&state, &link)?;
        assert!(prepare(&link).is_err());
        assert!(prepare(&link.join("new")).is_err());
        assert!(!state.join("new").exists());
        assert!(prepare(Path::new("relative")).is_err());
        assert!(prepare(&state.join("../elsewhere")).is_err());
        Ok(())
    }

    #[test]
    fn refuses_shared_state_and_symlink_token_without_chmod() -> CliResult<()> {
        let root = root()?;
        let shared = root.path().join("shared");
        fs::create_dir(&shared)?;
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755))?;
        assert!(prepare(&shared).is_err());
        assert_eq!(fs::metadata(&shared)?.permissions().mode() & 0o777, 0o755);
        assert!(!shared.join("dashboard-token").exists());
        let state = root.path().join("state");
        fs::create_dir(&state)?;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        let source = root.path().join("source");
        fs::write(&source, b"01234567890123456789012345678901")?;
        symlink(&source, state.join("dashboard-token"))?;
        assert!(prepare(&state).is_err());
        assert_eq!(fs::read(&source)?, b"01234567890123456789012345678901");
        Ok(())
    }
}
