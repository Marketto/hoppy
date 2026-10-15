//! Hoppy core: discovery and file transfer, shared by every desktop client and (through
//! `hoppy-ffi`) the iPhone/iPad app. Protocol v1 encrypts every session with Noise XX and
//! tracks device keys (trust on first use + optional verification).
//!
//! Start here: `docs/architecture.md`. Wire format: `docs/protocol.md` (v1: §7).

pub mod crypto;
pub mod discovery;
pub mod protocol;
pub mod sanitize;
pub mod session;
pub mod transfer;
pub mod trust;

pub use crypto::{DeviceKey, QrPayload, SAS_EMOJI, Sas, SasEmoji};
pub use discovery::{CallbackRequest, Discovery, DiscoveryConfig, Peer, Sweep};
pub use protocol::{DeviceInfo, Offer};
pub use transfer::{
    ALLOW_PLAINTEXT_DEFAULT, CryptoPolicy, Local, Progress, ReceiveHandler, SendOutcome,
    accept_callbacks, confirm_qr, send_files, send_to_peer, send_via_callback,
};
pub use trust::{PeerKey, SessionInfo, Trust, TrustEntry, TrustState, TrustStore};

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
    /// Handshake or decryption failed (tampered data, wrong keys, broken peer).
    #[error("encryption error: {0}")]
    Crypto(String),
    /// The device answered with a different key than the one remembered for it (spec §7.5):
    /// possible impersonation. Nothing was sent. The user may "forget" the device if it was
    /// reinstalled. Carries the device name.
    #[error("the security key of {0} has changed: not sending (forget the device to re-trust it)")]
    KeyChanged(String),
    /// A QR / code verification did not match (spec §7.7).
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    /// The peer cannot encrypt (old app) and plaintext is not allowed here, or a device
    /// known to encrypt answered without encryption (downgrade).
    #[error("not encrypted: {0}")]
    Unencrypted(String),
}
