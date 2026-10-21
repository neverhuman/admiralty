//! Shared error type mirroring the Python exception classes the tools caught.

use std::fmt;
use std::io;

/// Error raised by the ported tools.
///
/// `Invalid` corresponds to the Python tools' `Invalid(ValueError)`; `Io` to
/// `OSError`; `Key` to `KeyError` (displayed like Python, quoted); `Type` to
/// `TypeError`/`AttributeError`/`ValueError` raised by malformed data.
#[derive(Debug)]
pub enum ToolError {
    /// Validation failure; no external effect is authorized.
    Invalid(String),
    /// Operating-system error.
    Io(io::Error),
    /// Missing mapping key (Python `KeyError`).
    Key(String),
    /// Malformed value shape (Python `TypeError`, `AttributeError`, `ValueError`).
    Type(String),
    /// Child process exceeded its deadline (Python `TimeoutExpired`).
    Timeout(String),
}

impl ToolError {
    /// Build an `Invalid` error.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Build a `Type` error.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self::Type(message.into())
    }

    /// True when this is a validation (`Invalid`) error.
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }

    /// True when this is an I/O "not found" error.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Type(message) | Self::Timeout(message) => {
                formatter.write_str(message)
            }
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Key(key) => write!(formatter, "'{key}'"),
        }
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ToolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rustix::io::Errno> for ToolError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

/// Return `Invalid(message)` unless `condition` holds (Python `require`).
pub fn require(condition: bool, message: impl Into<String>) -> Result<(), ToolError> {
    if condition {
        Ok(())
    } else {
        Err(ToolError::Invalid(message.into()))
    }
}

/// Result alias used throughout the crate.
pub type ToolResult<T> = Result<T, ToolError>;
