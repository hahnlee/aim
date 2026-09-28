//! Authority node capabilities installed into one Android binder_proc handle
//! namespace. Transport never chooses numeric handles.

use super::*;

impl OpenConnection {
    pub fn remote_node_for_handle(
        &self,
        handle: u32,
    ) -> Result<crate::authority_protocol::NodeToken, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .remote_node_for_handle(key, handle)
            .map_err(Error::Registry)
    }

    pub fn enqueue_remote_dead_binder(
        &self,
        cookie: u64,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .enqueue_dead_binder(key, cookie)
            .map_err(Error::Registry)
    }

    pub fn enqueue_clear_death_done(
        &self,
        cookie: u64,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .enqueue_clear_death_done(key, cookie)
            .map_err(Error::Registry)
    }

    pub fn acknowledge_dead_binder(
        &self,
        cookie: u64,
    ) -> Result<(), Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .acknowledge_dead_binder(key, cookie)
            .map_err(Error::Registry)
    }

    /// Keep an exported local node alive while a remote process can hold a
    /// reference to it. The process coordinator owns the corresponding export
    /// table; the device remains the sole owner of BR_INCREFS/BR_ACQUIRE work.
    pub fn hold_exported_node(
        &self,
        local: crate::authority_protocol::LocalNodeToken,
        strength: crate::reference_table::Strength,
    ) -> Result<crate::node_owner::LocalHold, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        let kind = match strength {
            crate::reference_table::Strength::Strong => crate::node_owner::HoldKind::Strong,
            crate::reference_table::Strength::Weak => crate::node_owner::HoldKind::Weak,
        };
        self.registry
            .hold_local_node(key, local, kind)
            .map_err(Error::Registry)
    }

    pub fn resolve_remote_objects(
        &self,
        payload: &crate::transaction_snapshot::TransactionSnapshot,
    ) -> Result<Vec<crate::remote_objects::OutboundObject>, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .resolve_remote_objects(key, payload)
            .map_err(Error::Registry)
    }

    pub fn install_remote_reference(
        &self,
        node: crate::authority_protocol::NodeToken,
        strength: crate::reference_table::Strength,
    ) -> Result<u32, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .install_remote_reference(key, node, strength, false)
            .map_err(Error::Registry)
    }

    pub fn install_remote_context_manager(
        &self,
        node: crate::authority_protocol::NodeToken,
        strength: crate::reference_table::Strength,
    ) -> Result<u32, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .install_remote_reference(key, node, strength, true)
            .map_err(Error::Registry)
    }

    pub fn resolve_remote_transaction_target(
        &self,
        request: &crate::transaction_request::Request,
    ) -> Result<Option<crate::authority_protocol::NodeToken>, Error<std::convert::Infallible>> {
        let key = self.key.as_ref().ok_or(Error::Closed)?;
        self.registry
            .resolve_remote_transaction_target(key, request)
            .map_err(Error::Registry)
    }
}
