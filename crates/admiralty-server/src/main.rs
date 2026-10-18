use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Read-only Admiralty coordinator projection server")]
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
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let token = std::fs::read_to_string(args.token_file)?.trim().to_string();
    let store = admiralty_store::Store::open(args.database)?;
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
    admiralty_server::serve(
        store,
        admiralty_server::ServerConfig {
            listen: args.listen,
            simulated: false,
            token: Some(token),
            assets: args.assets,
            legacy,
            models_file: args.models_file,
        },
    )
    .await
}
