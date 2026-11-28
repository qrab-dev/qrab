#![deny(unsafe_code)]

pub mod pod;

/// Crate-level error type for manager.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration or validation error.
    #[error("{0}")]
    Config(String),
    /// Process spawn or lifecycle error.
    #[error("{0}")]
    Spawn(String),
    /// The supervisor is stopping the VM and accepts no further requests.
    #[error("VM is shutting down")]
    ShuttingDown,
    /// QEMU is not running: the VM is powered off and its manager waits for
    /// a power-on request.
    #[error("VM is powered off")]
    PoweredOff,
    /// The guest agent answered, but its answer or the guest's state does
    /// not allow the request, or the answer breaks the manager's bounds on
    /// untrusted guest data.
    #[error("guest agent: {0}")]
    GuestAgent(String),
    /// A guest agent request outlasted the manager's deadline after it was
    /// sent, so it may have run. The manager drops the session and
    /// resynchronizes before the next request.
    #[error("guest agent timed out after the request was sent; it may have run")]
    AgentTimeout,
    /// A value for a QMP request could not be encoded for QEMU's release.
    #[error("cannot encode a QMP request: {0}")]
    Encode(#[from] qapi::EncodeError),
    /// QMP/QGA protocol error.
    #[error(transparent)]
    Protocol(#[from] qapi_protocol::Error),
    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Crate-level Result alias.
pub type Result<T> = std::result::Result<T, Error>;
