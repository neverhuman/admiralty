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

fn prepare_state(path: &std::path::Path) -> CliResult<PathBuf> {
    crate::ops::init_preview_state(path)
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
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize()?)?;
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
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize()?)?;
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
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize()?)?;
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
