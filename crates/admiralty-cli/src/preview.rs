//! Private preview credentials. No service activation, enrollment or grant installation.
use crate::CliResult;
use std::path::Path;

#[cfg(unix)]
pub fn prepare(state: &Path) -> CliResult<serde_json::Value> {
    use rustix::fs::{Mode, OFlags, fstat, mkdirat, open, openat};
    use std::{fs::File, io::Write, path::Component};

    if !state.is_absolute()
        || state
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err("preview state must be an absolute normalized path".into());
    }
    let uid = rustix::process::geteuid().as_raw();
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open("/", flags, Mode::empty())?;
    // Traverse pinned descriptors; never follow a symlink or repair permissions.
    for component in state.components() {
        let metadata = fstat(&directory)?;
        if ![0, uid].contains(&metadata.st_uid)
            || (metadata.st_mode & 0o022 != 0 && metadata.st_mode & 0o1000 == 0)
        {
            return Err("preview state ancestor is not protected".into());
        }
        if let Component::Normal(name) = component {
            match mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            directory = openat(&directory, name, flags, Mode::empty())?;
        }
    }
    let metadata = fstat(&directory)?;
    if metadata.st_uid != uid || metadata.st_mode & 0o077 != 0 {
        return Err("existing preview state must be owned and owner-only".into());
    }
    let created = match openat(
        &directory,
        "dashboard-token",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(descriptor) => {
            let mut file = File::from(descriptor);
            // Two OS-random UUIDs retain 244 random bits after their fixed version bits.
            let mut token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let result = file
                .write_all(token.as_bytes())
                .and_then(|()| file.sync_all());
            token.clear();
            result?;
            File::from(directory).sync_all()?;
            true
        }
        Err(rustix::io::Errno::EXIST) => false,
        Err(error) => return Err(error.into()),
    };
    let token_file = state.join("dashboard-token");
    // Reuse exactly the daemon's size, ownership, ancestry, hard-link and credential checks.
    // Existing invalid files are refused without truncation, chmod or credential rotation.
    let _credential = admiralty_server::startup::read_private_token(&token_file)?;
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
            .tempdir_in(std::env::temp_dir().canonicalize()?)?)
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
