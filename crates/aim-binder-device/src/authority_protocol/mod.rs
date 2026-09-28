//! Bounded control-plane contract between one authenticated process-local
//! Binder endpoint and the profile-wide routing authority.
//!
//! The stream's verified peer credentials own the connection. Consequently no
//! request carries a sender PID, UID or ConnectionId. Parcel bytes, object
//! offsets, file descriptors, guest addresses, receive buffers and binder_thread
//! stacks are deliberately not representable by this protocol; a separate
//! endpoint-owned data plane must carry transactions after authority routing.

mod codec;
mod types;

pub use codec::{DecodeError, MAX_PAYLOAD_BYTES, decode, encode};
pub use types::{
    CallToken, ConnectionToken, LocalNodeToken, Message, NodeToken, TransactionFailure,
    TransferToken,
};

#[cfg(test)]
mod tests;
