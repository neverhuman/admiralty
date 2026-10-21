use clap::Parser;

#[derive(Parser)]
#[command(about = "Rust fleet observations and real dashboard history; never launches agents")]
struct Cli {
    #[command(flatten)]
    args: admiralty_observe::cli::ObserveArgs,
}
// Fleet hosts can expose hundreds of CPUs; observation does not need one thread per CPU.
#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match admiralty_observe::cli::run(Cli::parse().args).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "Native observer failed: {}",
                admiralty_observe::io::clean(&error.to_string())
            );
            std::process::ExitCode::FAILURE
        }
    }
}
