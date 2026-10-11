//! The crate-wide error and result types for transport-free voice contracts.

use serde::{Deserialize, Serialize};

/// A classified failure returned by a host-supplied voice provider.
///
/// Providers must remove credential echoes from messages before returning them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Error {
    /// The caller supplied invalid voice input.
    #[error("{message}")]
    InvalidInput {
        /// Human-readable detail with credentials removed.
        message: String,
    },
    /// No provider or usable backend credential is available.
    #[error("{message}")]
    Unavailable {
        /// Human-readable detail with credentials removed.
        message: String,
    },
    /// A signed-in session credential has expired or been rejected.
    #[error("{message}")]
    SessionExpired {
        /// Human-readable detail with credentials removed.
        message: String,
    },
    /// A backend API key has been rejected.
    #[error("{message}")]
    ApiKeyRejected {
        /// Human-readable detail with credentials removed.
        message: String,
    },
    /// The backend request could not be completed.
    #[error("{message}")]
    Transport {
        /// Human-readable detail with credentials removed.
        message: String,
    },
    /// The backend returned an invalid or oversized response.
    #[error("{message}")]
    InvalidResponse {
        /// Human-readable detail with credentials removed.
        message: String,
    },
}

/// The result returned by fallible operations in this crate.
pub type Result<T> = core::result::Result<T, Error>;
