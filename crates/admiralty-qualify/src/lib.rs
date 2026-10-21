//! Release and smoke qualification harnesses.
//!
//! Rust ports of the retired Python qualification scripts:
//!
//! | Retired script | Module | Entry point |
//! | --- | --- | --- |
//! | `scripts/release/package.py` | [`source_package`] | [`source_package::run_package`] |
//! | `scripts/check/canary-smoke.py` | [`canary`] | [`canary::run`] (`admiralty-canary`) |
//! | `scripts/check/excursions-smoke.py` | [`excursions`] | [`excursions::run`] (`admiralty-canary excursions`) |
//! | `scripts/release/qualify_native.py` | [`native_qualify`] | [`native_qualify::run_qualify`] |
//! | `tests/qualification/release_evidence.py` | [`release_evidence`] | [`release_evidence::run_evidence`] |
//!
//! Every entry point returns the process exit code the Python script used.
//! None of them grants acceptance, deployment authority or fleet rollout.

pub mod canary;
pub mod excursions;
pub mod http;
pub mod native_qualify;
pub mod process;
pub mod py;
pub mod pynet;
pub mod release_evidence;
pub mod source_package;

/// Result of a CLI entry point: the process exit code.
pub type RunResult = Result<i32, Box<dyn std::error::Error + Send + Sync>>;

pub use canary::SmokeArgs;
pub use excursions::ExcursionsArgs;
pub use native_qualify::QualifyArgs;
pub use release_evidence::EvidenceArgs;
pub use source_package::{PackageArgs, SelfTestReport};
