//! Raw PackageManagerInternal.grantImplicitAccess. No public visibility ACL.
//! AOSP android-16.0.0_r1, Apache License 2.0.
use super::*;

impl System {
    pub(crate) fn internal_grant_implicit_access(
        &self, user_id: i32, intent: Option<crate::package::intent::Intent>,
        recipient_app_id: i32, visible_uid: i32, direct: bool, retain_on_update: bool,
        calling_uid: i32, calling_pid: i32,
    ) -> Result<()> {
        use crate::package::{apps_filter, info, query::Query};
        if calling_uid < 0 || calling_pid < 0 {
            return Err(Exception::illegal_argument("invalid original caller identity"));
        }
        // Transport is constrained to the retained original system UID by the
        // internal host. Original PMInternal forwards arbitrary Binder caller
        // identities; this body intentionally does not add MAKE_UID_VISIBLE or
        // public cross-user gates to that trusted raw owner operation.
        let bridge = self.package_bootstrap()?;
        self.check_package_bootstrap(&bridge)?;
        loop {
            let capture = {
                let state = self.package_bootstrap.lock().unwrap();
                let current = state.current.as_ref().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                    .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "implicit-access bootstrap changed"))?;
                current.queries.clone().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "implicit-access native capture unavailable"))?
            };
            let resolution = capture.resolution().map_err(|error| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("implicit-access resolution: {error:?}")))?;
            // Computer.getPackage(uid) resolves getPackagesForUidInternal using
            // SYSTEM_UID and returns the first member with real loaded code.
            let query = Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: 1000 };
            let package_for_uid = |uid: i32| -> Result<Option<&crate::package::model::PackageState>> {
                let names = query.packages_for_uid(uid).map_err(|error| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?;
                Ok(names.as_ref().into_iter().flatten().filter_map(|name| name.as_deref())
                    .filter_map(|name| capture.state().packages.get(name)).find(|package| package.pkg.is_some()))
            };
            let Some(visible_package) = package_for_uid(visible_uid)? else { return Ok(()); };
            let recipient_uid = apps_filter::uid(user_id, recipient_app_id);
            if package_for_uid(recipient_uid)?.is_none() { return Ok(()); }
            let instant = info::user_state(visible_package, user_id).instant_app;
            let fail = |message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or_else(|| fail("implicit-access bootstrap changed".into()))?;
            if !current.queries.as_ref().is_some_and(|query| Arc::ptr_eq(query, &capture)) { continue; }
            let canonical = current.snapshots.as_ref().ok_or_else(|| fail("implicit-access canonical owner unavailable".into()))?.capture();
            let aligned = Arc::ptr_eq(capture.scan(), &canonical);
            let (view, update) = if instant {
                if !direct { return Ok(()); }
                let owner = current.instant_registry.clone().ok_or_else(|| fail("raw instant visibility owner unavailable".into()))?;
                if !owner.grant_access(user_id, intent.as_ref(), recipient_app_id, apps_filter::app_id(visible_uid)) {
                    return Ok(());
                }
                let view = capture.bind_instant_registry(owner.clone()).map_err(&fail)?;
                let update = if aligned { Some(capture.prepare_instant_registry_update(owner).map_err(&fail)?) } else { None };
                (view, update)
            } else {
                drop(state);
                let mut grants = capture.state().system.implicit_access.clone();
                if !grants.grant(recipient_uid, visible_uid, retain_on_update) { return Ok(()); }
                let prepared = if aligned {
                    let update = capture.prepare_visibility_update(grants).map_err(&fail)?;
                    (update.capture.clone(), Some(update))
                } else {
                    (capture.with_visibility_view(grants).map_err(&fail)?, None)
                };
                state = self.package_bootstrap.lock().unwrap();
                let current = state.current.as_ref().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                    .ok_or_else(|| fail("implicit-access bootstrap changed".into()))?;
                if !current.queries.as_ref().is_some_and(|query| Arc::ptr_eq(query, &capture)) { continue; }
                prepared
            };
            self.publish_internal_visibility_view(&mut state, &canonical, view, update)?;
            drop(state);
            self.check_package_bootstrap(&bridge)?;
            return bridge.invalidate_packages_for_uid_cache().map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(exception) => exception,
                other => Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("implicit-access committed, packages-for-UID cache invalidation failed: {other:?}")),
            });
        }
    }
}

impl System {
    /// Retain an actual visibility delta while canonical installation is ahead
    /// of query projection. No scan ownership is derived from the old query.
    pub(super) fn publish_internal_visibility_view(
        &self, state: &mut PackageBootstrapState,
        base: &Arc<crate::package::scan_snapshot::Snapshot>,
        view: Arc<crate::package::scan_snapshot::query_state::Capture>,
        update: Option<crate::package::scan_snapshot::query_state::PackageUpdate>,
    ) -> Result<()> {
        let fail = |message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        let current = state.current.as_mut().ok_or_else(|| fail("implicit-access bootstrap unavailable".into()))?;
        if let Some(update) = update {
            let snapshots = current.snapshots.as_ref().ok_or_else(|| fail("implicit-access canonical owner unavailable".into()))?;
            match snapshots.publish_validated_store_after(base, &update.store) {
                Ok(_) => {
                    let version = update.capture.scan().version();
                    current.queries = Some(update.capture);
                    if let Some(page) = &current.version_page { page.publish(version); }
                    state.version = version;
                    return Ok(());
                }
                Err(crate::package::scan_snapshot::Error::Stale) => {}
                Err(error) => return Err(fail(format!("implicit-access canonical publication: {error:?}"))),
            }
        }
        current.queries = Some(view);
        Ok(())
    }
}
