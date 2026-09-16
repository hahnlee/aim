//! Profile-wide Binder routing authority.
//!
//! This owns only authenticated connection/node/call identity and routing.
//! Process-local endpoints continue to own Binder UAPI parsing, guest memory,
//! Parcel contents, installed FDs, binder_thread stacks and delivery queues.

mod state;
mod types;

pub use state::RoutingAuthority;
pub use types::{CloseOutcome, Error, Opened, Outbound, PeerIdentity, RouteOutcome, Session};

#[cfg(test)]
mod tests;
