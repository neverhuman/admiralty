//! Native execution boundary. Managed provider execution is not enabled by scaffolding.
pub const MANAGED_EXECUTION_QUALIFIED: bool = false;

pub mod execution;
pub mod integration;
pub mod session;
pub mod trusted_checks;

pub mod models;

pub mod providers;
pub mod recovery;
pub mod reservations;
#[cfg(unix)]
pub mod resource_gate;
pub mod strategies;
pub mod workspace;

pub mod audits;
