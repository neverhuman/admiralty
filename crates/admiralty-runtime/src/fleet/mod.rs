//! Authenticated node observations and transfer. Receipt is never execution authority.
pub mod enrollment;
mod operator;
pub mod transfer;
pub mod transport;
pub use operator::{NodeArgs, run};

use sha2::{Digest, Sha256};
use std::io;

pub(crate) fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(crate) fn json<T: serde::Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(invalid)
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(value: &[u8]) -> io::Result<T> {
    serde_json::from_slice(value).map_err(invalid)
}
