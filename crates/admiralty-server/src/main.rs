use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Admiralty coordinator and dashboard")]
struct Args {
    #[arg(long)]
    database: PathBuf,
    #[arg(long, default_value = "127.0.0.1:18891")]
    listen: SocketAddr,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    assets: Option<PathBuf>,
    #[arg(long, requires = "legacy_index_file")]
    legacy_status_file: Option<PathBuf>,
    #[arg(long, requires = "legacy_status_file")]
    legacy_index_file: Option<PathBuf>,
    #[arg(long, requires = "legacy_status_file")]
    legacy_public_read: bool,
    #[arg(long)]
    models_file: Option<PathBuf>,
    /// Private, owner-controlled native operator and exact agent grant configuration.
    #[arg(long)]
    native_config: Option<PathBuf>,
    /// Optional old-port compatibility redirect served by this Rust binary.
    #[arg(long)]
    redirect_listen: Option<SocketAddr>,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let token = admiralty_server::startup::read_private_token(&args.token_file)?;
    let (store, native_routes) = match args.native_config {
        Some(path) => {
            let configured =
                admiralty_server::startup::open_native_services(&args.database, &path, &token)?;
            (configured.store, Some(configured.routes))
        }
        None => (admiralty_store::Store::open(args.database)?, None),
    };
    let legacy =
        args.legacy_status_file
            .zip(args.legacy_index_file)
            .map(
                |(status_file, index_file)| admiralty_server::legacy::LegacyConfig {
                    status_file,
                    index_file,
                    public_read: args.legacy_public_read,
                },
            );
    let primary = admiralty_server::serve_with_native_routes(
        store,
        admiralty_server::ServerConfig {
            listen: args.listen,
            simulated: false,
            token: Some(token),
            assets: args.assets,
            legacy,
            models_file: args.models_file,
        },
        native_routes,
    );
    match args.redirect_listen {
        Some(listen) => tokio::select! {
            result = primary => result,
            result = admiralty_server::serve_redirect(listen, args.listen) => result,
        },
        None => primary.await,
    }
}
