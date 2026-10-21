use clap::Parser;

fn main() {
    let cli = admiralty_xtask::Cli::parse();
    let code = match admiralty_xtask::run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("xtask: {error}");
            1
        }
    };
    std::process::exit(code);
}
