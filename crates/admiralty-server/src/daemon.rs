//! `admiraltyd` command line, shared by the standalone daemon binary and the
//! single-binary `admiralty serve` subcommand so both start identically.
use std::{net::SocketAddr, path::PathBuf};

#[derive(clap::Args, Debug, Clone)]
pub struct DaemonArgs {
    #[arg(long)]
    pub database: PathBuf,
    #[arg(long, default_value = "127.0.0.1:18891")]
    pub listen: SocketAddr,
    #[arg(long)]
    pub token_file: PathBuf,
    /// Built dashboard directory read per request (UI-only overlays). Omit it to
    /// serve the dashboard embedded at build time, when this binary has one.
    #[arg(long)]
    pub assets: Option<PathBuf>,
    /// Producer `status.json` behind `/v1/legacy*` and the React detailed view.
    #[arg(long)]
    pub legacy_status_file: Option<PathBuf>,
    /// Optional retained static HTML for `/legacy/`. Without it, `/legacy/`
    /// redirects to the React detailed view.
    #[arg(long, requires = "legacy_status_file")]
    pub legacy_index_file: Option<PathBuf>,
    #[arg(long, requires = "legacy_status_file")]
    pub legacy_public_read: bool,
    #[arg(long)]
    pub models_file: Option<PathBuf>,
    /// Private, owner-controlled native operator and exact agent grant configuration.
    #[arg(long)]
    pub native_config: Option<PathBuf>,
    /// Optional private intake credentials, scoped grants, policies and local origins.
    #[arg(long, requires = "native_config")]
    pub intake_config: Option<PathBuf>,
    /// Protected project-to-role inbox routes. Commands are agent instructions, never shell input.
    #[arg(long)]
    pub operator_chat_config: Option<PathBuf>,
    /// Optional old-port compatibility redirect served by this Rust binary.
    #[arg(long)]
    pub redirect_listen: Option<SocketAddr>,
}

/// Validate private inputs, open the Store and serve until SIGINT/SIGTERM.
pub async fn run(args: DaemonArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    admiralty_telemetry::init_from_env("admiraltyd");
    let assets = if args.assets.is_some() || crate::embedded::embedded_dashboard().is_none() {
        crate::startup::resolve_assets(args.assets, std::env::current_exe().ok().as_deref())
    } else {
        None
    };
    let token = crate::startup::read_private_token(&args.token_file)?;
    let chat = args
        .operator_chat_config
        .as_deref()
        .map(crate::operator_chat::load)
        .transpose()?;
    // Validate the complete independent intake configuration before any Store
    // opens. No grants are installed when this option is absent.
    let intake = args
        .intake_config
        .as_deref()
        .map(crate::intake_configuration::load_private_intake_configuration)
        .transpose()?;
    let (store, native_routes, intake_routes) = match args.native_config {
        Some(path) => {
            let configured = crate::startup::open_native_services_with_intake(
                &args.database,
                &path,
                &token,
                intake,
            )?;
            (
                configured.native.store,
                Some(configured.native.routes),
                configured.intake,
            )
        }
        None if intake.is_some() => {
            return Err("intake configuration requires native startup configuration".into());
        }
        None => (admiralty_store::Store::open(args.database)?, None, None),
    };
    let legacy = args
        .legacy_status_file
        .map(|status_file| crate::legacy::LegacyConfig {
            status_file,
            index_file: args.legacy_index_file,
            public_read: args.legacy_public_read,
        });
    if assets.is_none()
        && let Some(summary) = crate::embedded::embedded_dashboard()
    {
        eprintln!(
            "Admiralty dashboard: embedded {} files, {} bytes, tree sha256 {}",
            summary.files, summary.bytes, summary.tree_sha256
        );
    }
    let primary = crate::serve_with_operator_chat(
        store,
        crate::ServerConfig {
            listen: args.listen,
            simulated: false,
            token: Some(token),
            assets,
            legacy,
            models_file: args.models_file,
        },
        native_routes,
        intake_routes,
        chat,
    );
    match args.redirect_listen {
        Some(listen) => tokio::select! {
            result = primary => result,
            result = crate::serve_redirect(listen, args.listen) => result,
        },
        None => primary.await,
    }
}
