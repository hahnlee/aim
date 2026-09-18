//! Narrow Darwin host FD delivery ownership.

mod guardian;
mod owner;
pub(crate) mod transport;
mod types;
mod wire;

pub use owner::HostFdDeliveryOwner;
pub use types::*;
pub use wire::*;

#[cfg(test)]
mod tests;
