//! Hoppy core: discovery and file transfer, shared by every desktop client.
//!
//! Start here: `docs/architecture.md`. Wire format: `docs/protocol.md`.

pub mod discovery;
pub mod protocol;
pub mod sanitize;
pub mod transfer;

pub use discovery::{Discovery, DiscoveryConfig, Peer};
pub use protocol::{DeviceInfo, Offer};
pub use transfer::{Progress, ReceiveHandler, SendOutcome, send_files};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("{0}")]
    Input(String),
    #[error("remote error: {0}")]
    Remote(String),
}
