//! Original PermissionManager remains the owner; Binder authenticates forwarding.
use super::*;
impl System {
    pub(crate) fn package_permission_call(
        self: &Arc<Self>,
        code: u32,
        request: &Parcel,
    ) -> Result<aim_binder_host::local::Received> {
        match self.service("permissionmgr")? {
            ServiceOwner::Remote(owner) => self
                .process
                .transact_preserving_inbound(owner.handle, code, request)
                .map_err(|status| unreachable_service("permissionmgr", status)),
            ServiceOwner::Local(owner) => owner
                .transact(code, request, false)
                .map_err(|status| unreachable_service("permissionmgr", status)),
        }
    }
}
impl System {
    pub(crate) fn package_make_provider_visible(
        &self,
        uid: i32,
        request: &crate::package::visibility_mutation::MakeProviderVisible,
        resolver: &crate::package::resolve::Resolver,
    ) -> std::result::Result<std::result::Result<(), Exception>, crate::package::resolve::QueryError>
    {
        use crate::package::{
            apps_filter::NotModelled,
            query::Query,
            resolve::{QueryError, ResolutionError},
        };
        let bridge = self.package_bootstrap().map_err(|_| {
            QueryError::NotModelled(NotModelled("provider visibility bootstrap unavailable"))
        })?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
            .ok_or(QueryError::NotModelled(NotModelled(
                "provider visibility bootstrap changed",
            )))?;
        let capture = current
            .queries
            .clone()
            .ok_or(QueryError::NotModelled(NotModelled(
                "provider visibility capture unavailable",
            )))?;
        let resolution = resolver.resolution(capture.state()).map_err(|_| {
            QueryError::NotModelled(NotModelled("provider visibility resolver unavailable"))
        })?;
        let query = Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let grants = match request.decide(&resolution, &query) {
            Ok(Err(error)) => return Ok(Err(error)),
            Ok(Ok(None)) => return Ok(Ok(())),
            Ok(Ok(Some(grants))) => grants,
            Err(ResolutionError::Original(error)) => return Ok(Err(error)),
            Err(ResolutionError::NotModelled(error)) => return Err(QueryError::NotModelled(error)),
            Err(ResolutionError::UriMatching(error)) => {
                return Ok(Err(error.binder_exception().unwrap_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "provider URI authority owner unavailable",
                    )
                })));
            }
        };
        let update = capture.prepare_visibility_update(grants).map_err(|_| {
            QueryError::NotModelled(NotModelled("provider visibility preparation failed"))
        })?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page {
            page.publish(update.capture.scan().version());
        }
        state.version = update.capture.scan().version();
        drop(state);
        bridge.invalidate_packages_for_uid_cache().map_err(|_| {
            QueryError::NotModelled(NotModelled(
                "provider visibility UID cache invalidation failed",
            ))
        })?;
        Ok(Ok(()))
    }
}
impl System {
    pub(crate) fn package_is_auto_revoke_whitelisted(
        &self,
        uid: i32,
        package: Option<String>,
    ) -> Result<bool> {
        let bridge = self.package_bootstrap()?;
        let result =
            bridge
                .is_auto_revoke_whitelisted(uid, package)
                .map_err(|error| match error {
                    crate::package::bootstrap::OwnerError::Owner(error) => error,
                    error => Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("auto revoke owner: {error:?}"),
                    ),
                })?;
        self.check_package_bootstrap(&bridge)?;
        Ok(result)
    }
}
impl System {
    pub(crate) fn package_hold_lock_token(
        &self,
        query: &crate::package::query::Query<'_>,
    ) -> std::result::Result<
        std::result::Result<aim_binder_host::parcel::Binder, Exception>,
        crate::package::apps_filter::NotModelled,
    > {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_ref()
            .ok_or(crate::package::apps_filter::NotModelled(
                "hold lock bootstrap unavailable",
            ))?;
        if !current
            .queries
            .as_ref()
            .is_some_and(|capture| std::ptr::eq(capture.state().as_ref(), query.state))
        {
            return Err(crate::package::apps_filter::NotModelled(
                "hold lock query generation changed",
            ));
        }
        current.hold_locks.issue(query)
    }
    pub(crate) fn package_hold_lock(
        &self,
        uid: i32,
        token: Option<aim_binder_host::parcel::Binder>,
        duration: i32,
    ) -> Result<()> {
        let state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_ref().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "hold lock bootstrap unavailable",
            )
        })?;
        current.hold_locks.verify(uid, token)?;
        let duration =
            u64::try_from(duration).map_err(|_| Exception::illegal_argument("millis < 0"))?;
        // The package publication lock is PMS's native mLock counterpart.
        // Unlike a host timer callback, sleep retains this actual owner lock.
        std::thread::sleep(std::time::Duration::from_millis(duration));
        drop(state);
        Ok(())
    }
}
impl System {
    pub(crate) fn package_verifier_identity(
        &self,
        uid: i32,
        resolver: &crate::package::resolve::Resolver,
    ) -> std::result::Result<
        std::result::Result<crate::package::verifier::Identity, Exception>,
        crate::package::resolve::QueryError,
    > {
        use crate::package::{
            apps_filter::{self, NotModelled},
            query::Query,
            resolve::QueryError,
            verifier::Identity,
        };
        let bridge = self
            .package_bootstrap()
            .map_err(|_| QueryError::NotModelled(NotModelled("verifier bootstrap unavailable")))?;
        let persistence = {
            let state = self.package_bootstrap.lock().unwrap();
            state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .and_then(|current| current.persistence.clone())
                .ok_or(QueryError::NotModelled(NotModelled(
                    "verifier settings owner unavailable",
                )))?
        };
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| {
                Arc::ptr_eq(&current.bridge, &bridge)
                    && current
                        .persistence
                        .as_ref()
                        .is_some_and(|owner| Arc::ptr_eq(owner, &persistence))
            })
            .ok_or(QueryError::NotModelled(NotModelled(
                "verifier bootstrap owner changed",
            )))?;
        let capture = current
            .queries
            .clone()
            .ok_or(QueryError::NotModelled(NotModelled(
                "verifier query capture unavailable",
            )))?;
        let resolution = resolver.resolution(capture.state()).map_err(|_| {
            QueryError::NotModelled(NotModelled("verifier query resolver unavailable"))
        })?;
        let query = Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        if apps_filter::app_id(uid) != 0
            && apps_filter::app_id(uid) != 1000
            && !query
                .uid_has_permission(uid, "android.permission.PACKAGE_VERIFICATION_AGENT")
                .map_err(QueryError::NotModelled)?
        {
            return Ok(Err(Exception::security(
                "getVerifierDeviceIdentity requires android.permission.PACKAGE_VERIFICATION_AGENT",
            )));
        }
        if let Some(encoded) = capture.scan().owner().settings.verifier.as_deref() {
            return Ok(Identity::parse(encoded).map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
            }));
        }
        if let Err(error) = disk.validate_committed_scan(capture.scan().owner()) {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                error.to_string(),
            )));
        }
        let identity = Identity::generate();
        let mut scan = capture.scan().owner().clone();
        scan.settings.verifier = Some(identity.encoded());
        let update = match capture.prepare_package_update(scan) {
            Ok(update) => update,
            Err(error) => {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    error,
                )));
            }
        };
        // As Settings does, publish the installation-local nonce first. I/O
        // failure logs loss-at-reboot but retains the live verifier identity.
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page {
            page.publish(update.capture.scan().version());
        }
        state.version = update.capture.scan().version();
        let written = disk.commit_verifier_identity(identity);
        drop(state);
        drop(disk);
        if let Err(error) = written {
            eprintln!(
                "Unable to write verifier package settings; committed={}: {}",
                error.committed, error.message
            );
        }
        if let Err(error) = bridge.invalidate_package_info_cache() {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("verifier package cache invalidation: {error:?}"),
            )));
        }
        Ok(Ok(identity))
    }
}
