//! Retained terminal truth for one authenticated Binder routing connection.
//!
//! Retaining this metadata does not keep the transport/device owner alive.
//! Queries are atomic-only so native endpoint admission can read them without
//! calling transport, framework policy or another resource owner under its lock.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct AuthorityLifetime {
    pub(crate) closed: Arc<AtomicBool>,
}

impl AuthorityLifetime {
    pub fn live(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
}
