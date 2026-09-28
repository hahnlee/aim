//! `/data` mount policy. Filesystem operations and retained FD lifetime belong
//! to WritableMount; this owner fixes the Android prefix for existing callers.
use super::writable_mount::WritableMount;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

pub(super) struct PrivateDataRoot(WritableMount);

impl PrivateDataRoot {
    pub(super) fn open(path: PathBuf) -> Result<Self, &'static str> {
        WritableMount::open(path, b"/data").map(Self)
    }
}

impl Deref for PrivateDataRoot {
    type Target = WritableMount;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PrivateDataRoot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
