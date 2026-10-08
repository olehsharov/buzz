//! Error type for the agent-host daemon and its subcommands.

use thiserror::Error;

/// Errors produced by `buzz host` operations.
///
/// Messages never include secrets (agent nsecs, the host key, or auth tags):
/// they may be logged and are sent back to the owner inside `host.ack`.
#[derive(Debug, Error)]
pub enum HostError {
    /// Filesystem access failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done when the error happened (no secrets).
        context: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// JSON (de)serialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// NIP-AB pairing failed.
    #[error("pairing failed: {0}")]
    Pairing(String),
    /// The relay connection failed.
    #[error("relay error: {0}")]
    Relay(String),
    /// Input from the owner, the user, or local state was invalid.
    #[error("{0}")]
    Invalid(String),
    /// The host is not paired yet.
    #[error("this machine is not paired; run `buzz host up` or `buzz host pair <uri>`")]
    NotPaired,
    /// A supervisor command (systemctl, launchctl, process spawn) failed.
    #[error("{0}")]
    Supervisor(String),
}

impl HostError {
    /// Wrap an I/O error with a human-readable context string.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

/// Convenience alias for `Result<T, HostError>`.
pub type Result<T> = std::result::Result<T, HostError>;
