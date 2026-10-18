//! Native execution boundary. Managed provider execution is not enabled by scaffolding.
pub const MANAGED_EXECUTION_QUALIFIED: bool = false;

pub mod session;
pub mod execution;
pub mod trusted_checks;

pub mod models;

pub mod providers;
pub mod recovery;
#[cfg(any(target_os = "linux", test))]
pub mod resource_gate;
pub mod reservations;
pub mod strategies;
pub mod workspace;

pub mod audits;
