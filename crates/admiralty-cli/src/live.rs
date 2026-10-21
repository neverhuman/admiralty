//! Source and installed-bundle entry point for the read-only producer console.
use crate::{CliResult, bounded_absolute_path};
use admiralty_server::{ServerConfig, legacy::LegacyConfig};
use clap::Args;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Args)]
pub struct LiveArgs {
    #[arg(long, value_parser = bounded_absolute_path)]
    state_dir: PathBuf,
    #[arg(long, value_parser = bounded_absolute_path)]
    status: PathBuf,
    #[arg(long, value_parser = bounded_absolute_path)]
    index: PathBuf,
    #[arg(long, value_parser = bounded_absolute_path)]
    assets: PathBuf,
    #[arg(long, default_value = "127.0.0.1:18891")]
    listen: SocketAddr,
    #[arg(long, value_parser = bounded_absolute_path)]
    models: Option<PathBuf>,
}

pub async fn run(args: LiveArgs) -> CliResult<()> {
    // This convenience mode explicitly exposes the producer view. Never let a
    // mistaken bind address publish it to a LAN while creating private state.
    if !args.listen.ip().is_loopback() {
        return Err("live preview requires a loopback listener".into());
    }
    for file in [&args.status, &args.index, &args.assets.join("index.html")] {
        if !std::fs::metadata(file).is_ok_and(|m| m.is_file()) {
            return Err("live preview requires existing producer files and built assets".into());
        }
    }
    let token_file = prepare_state(&args.state_dir)?;
    let token = admiralty_server::startup::read_private_token(&token_file)?;
    eprintln!(
        "Dashboard access token file: {} (value not displayed)",
        token_file.display()
    );
    admiralty_server::serve(
        admiralty_store::Store::open(args.state_dir.join("native.sqlite"))?,
        ServerConfig {
            listen: args.listen,
            simulated: false,
            token: Some(token),
            assets: Some(args.assets),
            legacy: Some(LegacyConfig {
                status_file: args.status,
                index_file: args.index,
                public_read: true,
            }),
            models_file: args.models,
        },
    )
    .await
}

#[cfg(unix)]
fn prepare_state(path: &std::path::Path) -> CliResult<PathBuf> {
    use rustix::fs::{Mode, OFlags, fstat, mkdirat, open, openat};
    use std::{fs::File, io::Write, path::Component};
    let uid = rustix::process::geteuid().as_raw();
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("live state directory must be absolute and normalized".into());
    }
    let protected = |stat: &rustix::fs::Stat| {
        (stat.st_uid == 0 || stat.st_uid == uid)
            && (stat.st_mode & 0o022 == 0 || stat.st_mode & 0o1000 != 0)
    };
    let mut directory = open("/", flags, Mode::empty())?;
    for component in path.components() {
        if !protected(&fstat(&directory)?) {
            return Err("live state ancestor is not protected".into());
        }
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = match openat(&directory, name, flags, Mode::empty()) {
                    Ok(fd) => fd,
                    Err(rustix::io::Errno::NOENT) => {
                        // A concurrent initializer may win; re-open and validate
                        // its directory rather than relaxing existing modes.
                        match mkdirat(&directory, name, Mode::RWXU) {
                            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                            Err(error) => return Err(error.into()),
                        }
                        openat(&directory, name, flags, Mode::empty())?
                    }
                    Err(error) => return Err(error.into()),
                };
            }
            _ => return Err("live state directory must be normalized".into()),
        }
    }
    let stat = fstat(&directory)?;
    if stat.st_uid != uid || stat.st_mode & 0o077 != 0 {
        return Err("existing live state directory must be owned and owner-only".into());
    }
    match openat(
        &directory,
        "dashboard-token",
        OFlags::WRONLY
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => {
            let mut file = File::from(fd);
            // Two independently generated OS-random UUIDs; never echo bytes.
            let mut token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            )
            .into_bytes();
            let result = file.write_all(&token).and_then(|()| file.sync_all());
            token.fill(0);
            result?;
            File::from(directory).sync_all()?;
        }
        Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    let token_file = path.join("dashboard-token");
    // Apply the same bounded, owner-only, no-follow, single-link validation as
    // native daemon startup, including on re-use. Never repair/rotate a token.
    admiralty_server::startup::read_private_token(&token_file)?;
    Ok(token_file)
}

#[cfg(not(unix))]
fn prepare_state(_: &std::path::Path) -> CliResult<PathBuf> {
    Err("private live startup is not qualified on this platform".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    };

    #[test]
    fn initialization_is_private_and_reuses_exact_credential() -> CliResult<()> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("new/preview");
        let token = prepare_state(&state)?;
        let before = fs::read(&token)?;
        assert_eq!(before.len(), 64);
        assert_eq!(fs::metadata(&state)?.mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&token)?.mode() & 0o777, 0o600);
        assert_eq!(prepare_state(&state)?, token);
        assert_eq!(fs::read(&token)?, before);
        Ok(())
    }

    #[test]
    fn refuses_public_state_symlink_ancestors_and_invalid_existing_tokens() -> CliResult<()> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let token = prepare_state(&state)?;
        let before = fs::read(&token)?;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755))?;
        assert!(prepare_state(&state).is_err());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
        let alias = root.path().join("alias");
        symlink(&state, &alias)?;
        assert!(prepare_state(&alias.join("child")).is_err());
        assert!(!state.join("child").exists());
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644))?;
        assert!(prepare_state(&state).is_err());
        assert_eq!(fs::read(&token)?, before);
        fs::remove_file(&token)?;
        symlink(root.path().join("absent"), &token)?;
        assert!(prepare_state(&state).is_err());
        assert!(!root.path().join("absent").exists());
        Ok(())
    }

    #[tokio::test]
    async fn non_loopback_is_refused_before_creating_any_state() -> CliResult<()> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("absent");
        let result = run(LiveArgs {
            state_dir: state.clone(),
            status: root.path().join("status.json"),
            index: root.path().join("index.html"),
            assets: root.path().join("assets"),
            listen: "0.0.0.0:18891".parse()?,
            models: None,
        })
        .await;
        assert!(result.is_err());
        assert!(!state.exists());
        Ok(())
    }
}
