//! Hoppy core: discovery and file transfer, shared by every desktop client.
//!
//! Start here: `docs/architecture.md`. Wire format: `docs/protocol.md`.

pub mod discovery;
pub mod protocol;
pub mod sanitize;
pub mod transfer;

pub use discovery::{CallbackRequest, Discovery, DiscoveryConfig, Peer, Sweep};
pub use protocol::{DeviceInfo, Offer};
pub use transfer::{
    Progress, ReceiveHandler, SendOutcome, accept_callbacks, send_files, send_to_peer,
    send_via_callback,
};

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
    /// Neither a direct connection nor a callback (spec §6) reached the peer.
    #[error("peer unreachable: {0}")]
    Unreachable(String),
}
