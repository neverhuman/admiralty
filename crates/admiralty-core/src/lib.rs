//! Deterministic work and coordination contracts. No filesystem, clock or network I/O.
pub mod context;
pub mod contracts;
pub mod events;
pub mod ids;
pub mod messages;
pub mod reducer;

pub use context::*;
pub use contracts::*;
pub use events::*;
pub use ids::*;
pub use messages::*;
pub use reducer::*;

pub fn content_digest<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(value)?)
    ))
}
