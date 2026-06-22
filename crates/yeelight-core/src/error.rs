//! Error and result types for the crate.

/// Convenient result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by discovery, control, music mode and firewall helpers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Underlying I/O failure (socket, process, ...).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Writing the device registry to TOML failed.
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    /// Reading the device registry from TOML failed.
    #[error("toml deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    /// The device returned an `error` object in response to a command (spec §4.2).
    #[error("device error {code}: {message}")]
    Protocol {
        /// Device error code.
        code: i64,
        /// Human-readable message from the device.
        message: String,
    },

    /// A command did not receive a response within the timeout.
    #[error("request timed out")]
    Timeout,

    /// A parameter failed local validation before being sent.
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// The target device does not advertise support for this method (spec `support` header).
    #[error("device does not support method: {0}")]
    Unsupported(String),

    /// The control connection is closed.
    #[error("not connected")]
    NotConnected,

    /// A firewall (`ufw`) operation could not be completed.
    #[error("firewall: {0}")]
    Firewall(String),
}
