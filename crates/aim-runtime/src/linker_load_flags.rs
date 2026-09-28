//! Effective per-image load flags, independent of Darwin RTLD bit values.
//! This is not a reference counter or permission to unmap an image.
use std::sync::atomic::{AtomicU8, Ordering};
const GLOBAL: u8 = 1;
const NODELETE: u8 = 2;
#[derive(Debug, Default)]
pub(crate) struct LoadFlags(AtomicU8);
impl LoadFlags {
    pub fn new(global: bool, nodelete: bool) -> Self {
        Self(AtomicU8::new(
            u8::from(global) * GLOBAL | u8::from(nodelete) * NODELETE,
        ))
    }
    pub fn promote_global(&self) {
        self.0.fetch_or(GLOBAL, Ordering::AcqRel);
    }
    pub fn promote_nodelete(&self) {
        self.0.fetch_or(NODELETE, Ordering::AcqRel);
    }
    pub fn snapshot(&self) -> (bool, bool) {
        let flags = self.0.load(Ordering::Acquire);
        (flags & GLOBAL != 0, flags & NODELETE != 0)
    }
}
