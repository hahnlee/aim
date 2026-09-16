//! Resolver-issued mount identity, not permission or image-authenticity policy.
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct MountOrigin {
    pub(super) namespace: Arc<()>,
    pub(super) mount: Option<Vec<u8>>,
}

impl PartialEq for MountOrigin {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.namespace, &other.namespace) && self.mount == other.mount
    }
}
impl Eq for MountOrigin {}

impl MountOrigin {
    /// Identifies the capability selected during lookup, not the requested path.
    /// Root origin alone does not establish immutable-image authority.
    pub fn mount_path(&self) -> &[u8] {
        self.mount.as_deref().unwrap_or(b"/")
    }
}
