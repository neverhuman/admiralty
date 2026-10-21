//! `admiralty-canary`: bounded offline smoke and qualification harnesses.
//!
//! The default invocation keeps the retired `canary-smoke.py` interface:
//!
//! ```text
//! admiralty-canary --admiraltyd-binary /abs/admiraltyd --admiralty-binary /abs/admiralty --scratch-root /abs/private
//! admiralty-canary excursions --binary target/debug/admiraltyd --scratch /new/private/dir
//! admiralty-canary package --input-root ... --inputs ... --version ... --target ... --source-commit ... --output ...
//! admiralty-canary qualify-native --binary ... --assets ... --output ... --source-commit ... --target ...
//! admiralty-canary release-evidence --manifest ... --evidence-root ... --expected-revision ...
//! ```

use clap::{Parser, Subcommand};

use admiralty_qualify::{
    EvidenceArgs, ExcursionsArgs, PackageArgs, QualifyArgs, SmokeArgs, canary, excursions,
    native_qualify, release_evidence, source_package,
};

#[derive(Parser)]
#[command(
    name = "admiralty-canary",
    about = "Bounded offline HTTP smoke checks for explicitly supplied Admiralty binaries.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Mode>,
    #[command(flatten)]
    smoke: SmokeArgs,
}

#[derive(Subcommand)]
enum Mode {
    /// The real canary smoke (same as the default invocation).
    Smoke(SmokeArgs),
    /// Isolated incident-journal smoke of one admiraltyd binary, including restart.
    Excursions(ExcursionsArgs),
    /// Build a deterministic source package from an explicit file list.
    Package(PackageArgs),
    /// Qualify a private native archive/install/demo journey offline.
    QualifyNative(QualifyArgs),
    /// Read-only release evidence preflight.
    ReleaseEvidence(EvidenceArgs),
}

fn main() {
    let cli = Cli::parse();
    let outcome = match cli.command {
        None => canary::run(cli.smoke),
        Some(Mode::Smoke(args)) => canary::run(args),
        Some(Mode::Excursions(args)) => excursions::run(args),
        Some(Mode::Package(args)) => source_package::run_package(args),
        Some(Mode::QualifyNative(args)) => native_qualify::run_qualify(args),
        Some(Mode::ReleaseEvidence(args)) => release_evidence::run_evidence(args),
    };
    let code = match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("admiralty-canary: {error}");
            1
        }
    };
    std::process::exit(code);
}
