#[cfg(unix)]
fn main() -> std::process::ExitCode {
    use clap::Parser;
    #[derive(Parser)]
    #[command(
        about = "Explicit node enrollment, authenticated observations and artifact custody. No provider execution."
    )]
    struct Cli {
        #[command(flatten)]
        args: admiralty_runtime::fleet::NodeArgs,
    }
    match admiralty_runtime::fleet::run(Cli::parse().args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("admiralty-node: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("admiralty-node requires Unix private-file custody");
    std::process::ExitCode::from(78)
}
