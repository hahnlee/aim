//! Native preferred state shares the package persistence/publication gate.
use super::*;
use crate::package::preferred::{Selection, registry::{Actions, Commit, Handle, Registry, Stage}};
use crate::package::apps_filter::NotModelled;

impl System {
    pub fn install_native_package_preferred(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>, image: &std::path::Path,
    ) -> Result<()> {
        let actions = crate::package::preferred::actions::SystemActions::new(
            self, bridge.clone(), image.to_path_buf(),
        );
        self.install_package_preferred(bridge, actions)
    }

    pub fn package_preferred_registry(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<Registry>> {
        self.check_package_bootstrap(bridge)?;
        self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.preferred_registry.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred registry unavailable"))
    }

    pub fn set_package_preferred_pending_browser(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, user: i32, package: Option<&str>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred disk owner unavailable"))?;
        let mut disk = persistence.lock().unwrap();
        self.check_package_bootstrap(bridge)?;
        let user = u32::try_from(user).map_err(|_| Exception::illegal_argument("invalid browser user"))?;
        disk.set_pending_default_browser(user, package).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))
    }

    pub fn reset_package_preferred_domains(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, user: i32,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred domain disk owner unavailable"))?;
        let mut disk = persistence.lock().unwrap();
        let capture = self.capture_package_queries()?;
        let mut domains = capture.domains().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "preferred domain owner unavailable"))?
            .owner().clone();
        domains.clear_user(user);
        let update = capture.prepare_domain_update(domains).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        self.commit_package_domains(bridge, update, &mut disk).map(|_| ())
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.to_string()))
    }

    pub fn install_package_preferred(
        self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        actions: Arc<dyn Actions>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred disk owner unavailable"))?;
        let provider = self.package_preferred_identity_provider(bridge)?;
        let disk = persistence.lock().unwrap();
        // Identity allocation can call the original owner; no publication lock is held.
        let registry = Arc::new(Registry::from_store(provider, &disk).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?);
        let mut documents = Vec::new();
        for (user, _) in disk.native_preferred_users().map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))? {
            documents.push((user, disk.preferred_user_document(user).map_err(|error|
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?));
        }
        let system = Arc::downgrade(self);
        let retained_bridge = bridge.clone();
        let commit_registry = registry.clone();
        let commit: Commit = Arc::new(move |user, generation, selection| {
            let system = system.upgrade().ok_or(NotModelled("preferred system stopped"))?;
            system.commit_package_preferred_selection(&retained_bridge, &commit_registry,
                user, generation, selection).map_err(|_| NotModelled("preferred selection publication failed"))
        });
        let handle = Arc::new(Handle::new(Arc::new(registry.capture().map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?), commit.clone(), actions.clone()));
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.persistence.as_ref().is_some_and(|disk| Arc::ptr_eq(disk, &persistence)))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred bootstrap changed"))?;
        if current.preferred_registry.is_some() {
            return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred owner already installed"));
        }
        let capture = current.queries.as_ref().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "preferred query capture unavailable"))?;
        let update = capture.prepare_preferred_update(handle, &documents).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.preferred_registry = Some(registry);
        current.preferred_commit = Some(commit);
        current.preferred_actions = Some(actions);
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        drop(disk);
        bridge.invalidate_package_info_cache().map_err(|error| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("preferred cache owner: {error:?}")))
    }

    pub fn commit_package_preferred_selection(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, registry: &Arc<Registry>,
        user: i32, generation: u64, selection: &Selection,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        // Replacement identities are allocated before acquiring the publication gate.
        let Some(stage) = registry.prepare_selection(user, generation, selection).map_err(|error|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))? else { return Ok(()); };
        self.commit_package_preferred_stage(bridge, registry, stage, false)
    }

    pub fn commit_package_preferred_stage(
        &self, bridge: &Arc<crate::package::bootstrap::Bridge>, registry: &Arc<Registry>,
        stage: Stage, memory_only: bool,
    ) -> Result<()> {
        let fail = |error: String| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error);
        self.check_package_bootstrap(bridge)?;
        let persistence = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone()).ok_or_else(|| fail("preferred disk owner unavailable".into()))?;
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge)
            && current.preferred_registry.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, registry))
            && current.persistence.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &persistence)))
            .ok_or_else(|| fail("preferred publication owner changed".into()))?;
        registry.validate(&stage).map_err(&fail)?;
        let preview = registry.preview(&stage).map_err(&fail)?;
        let bytes = disk.preview_preferred_stage(&stage).map_err(&fail)?;
        let commit = current.preferred_commit.clone().ok_or_else(|| fail("preferred commit owner unavailable".into()))?;
        let actions = current.preferred_actions.clone().ok_or_else(|| fail("preferred effects owner unavailable".into()))?;
        let handle = Arc::new(Handle::new(Arc::new(preview), commit, actions));
        let capture = current.queries.as_ref().ok_or_else(|| fail("preferred query owner unavailable".into()))?;
        let update = capture.prepare_preferred_update(handle, &[(stage.user, bytes)]).map_err(&fail)?;
        let written = if memory_only {
            disk.apply_preferred_stage_in_memory(&stage).map_err(&fail)?;
            Ok(())
        } else { disk.commit_preferred_stage(&stage) };
        if written.as_ref().is_err_and(|error| !error.committed) {
            return Err(fail(written.unwrap_err().to_string()));
        }
        registry.publish(stage).map_err(&fail)?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        drop(disk);
        let invalidated = bridge.invalidate_package_info_cache().map_err(|error| fail(format!("preferred cache owner: {error:?}")));
        written.map_err(|error| fail(error.to_string()))?;
        invalidated
    }
}
