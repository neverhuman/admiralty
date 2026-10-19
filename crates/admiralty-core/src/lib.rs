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
    struct DigestWriter(Sha256);
    impl std::io::Write for DigestWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = DigestWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(format!("sha256:{:x}", writer.0.finalize()))
}
pub mod config;
pub mod conflicts;
pub mod coordination;
pub mod economics;
pub mod excursions;
pub mod governor;
pub mod intake;
pub mod interfaces;
pub mod limits;
pub mod nodes;
pub mod placement;
pub mod publication;
pub mod retirement;
pub mod scheduler;
pub mod trials;
pub mod verification;
pub mod views;
pub mod workplans;
