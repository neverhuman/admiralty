use clap::Parser;

#[derive(Parser)]
#[command(about = "Admiralty coordinator and dashboard", version)]
struct Cli {
    #[command(flatten)]
    args: admiralty_server::daemon::DaemonArgs,
}

// Control-plane concurrency is bounded independently of host CPU count.
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    admiralty_server::daemon::run(Cli::parse().args).await
}
