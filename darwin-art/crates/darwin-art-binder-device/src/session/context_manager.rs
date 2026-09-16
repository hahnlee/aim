use super::*;
use crate::node_owner::ContextManagerRefs;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Node(node_owner::Error),
    Resource(i32),
}
impl Session {
    pub(crate) fn create_context_manager_refs(
        &mut self,
        object: &Object<'_>,
    ) -> Result<ContextManagerRefs, Error> {
        let node = self.owner.resolve(object).map_err(Error::Node)?;
        self.owner
            .acquire_context_manager_refs(node)
            .map_err(|e| Error::Resource(e.raw_os_error().unwrap_or(libc::EIO)))
    }
}
