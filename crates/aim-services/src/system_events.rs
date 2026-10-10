//! Native package handlers are retained with the bootstrap; guards join outside locks.
use super::*;
impl System {
    pub(crate) fn package_move_primary(&self, volume: Option<String>) -> Result<i32> {
        let bridge = self.package_bootstrap()?;
        let storage = bridge.package_moves().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(exception) => exception,
            error => Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("storage move owner: {error:?}"),
            ),
        })?;
        self.check_package_bootstrap(&bridge)?;
        let id = self.package_moves()?.move_primary(&storage, volume)?;
        self.check_package_bootstrap(&bridge)?;
        Ok(id)
    }
    pub(crate) fn configure_package_events(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Option<crate::package::events::Workers>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .is_some_and(|current| current.events.is_some())
            {
                return Ok(None);
            }
        }
        let (owner, workers) = crate::package::events::Owner::start(self.process.clone())?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package event bootstrap changed",
                )
            })?;
        if current.events.is_some() {
            drop(state);
            drop(workers);
            return Ok(None);
        }
        current.moves = Some(crate::package::moves::Owner::new(
            self.process.clone(),
            owner.clone(),
        ));
        current.events = Some(owner);
        Ok(Some(workers))
    }
    pub(crate) fn package_events(&self) -> Result<Arc<crate::package::events::Owner>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.events.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native package event owner unavailable",
                )
            })
    }

    pub(crate) fn package_moves(&self) -> Result<Arc<crate::package::moves::Owner>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.moves.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native move owner unavailable",
                )
            })
    }
    pub(crate) fn package_register_monitor(
        &self,
        callback: Option<Binder>,
        uid: i32,
        pid: i32,
        user: i32,
    ) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        let target = bridge
            .package_monitor_user(pid, uid, user)
            .map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(error) => error,
                error => Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package monitor incoming user: {error:?}"),
                ),
            })?;
        self.check_package_bootstrap(&bridge)?;
        self.package_events()?.register(callback, target, uid)
    }
    pub(crate) fn package_unregister_monitor(&self, callback: Option<Binder>) -> Result<()> {
        self.package_events()?.unregister(callback)
    }
    pub(crate) fn package_wait_for_handler(&self, timeout: i64, background: bool) -> Result<bool> {
        let start = std::time::Instant::now();
        if !self.package_events()?.wait(timeout, background)? {
            return Ok(false);
        }
        if !background {
            return Ok(true);
        }
        let elapsed = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);
        let remaining = timeout.saturating_sub(elapsed);
        let bridge = self.package_bootstrap()?;
        let finished = bridge
            .wait_package_background_handler(remaining)
            .map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(error) => error,
                error => Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package original background barrier: {error:?}"),
                ),
            })?;
        self.check_package_bootstrap(&bridge)?;
        Ok(finished)
    }
}
impl System {
    pub(crate) fn package_notify_use(
        &self,
        uid: i32,
        name: Option<String>,
        reason: i32,
    ) -> Result<()> {
        use crate::package::{apps_filter, info};
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package usage bootstrap unavailable",
            )
        })?;
        let capture = current.queries.clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package usage capture unavailable",
            )
        })?;
        let fail = |error: apps_filter::NotModelled| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
        };
        let notify = if apps_filter::instant_app_package_name(capture.state(), uid)
            .map_err(fail)?
            .is_some()
        {
            apps_filter::is_caller_same_app(capture.state(), name.as_deref(), uid).map_err(fail)?
        } else {
            !name
                .as_deref()
                .and_then(|name| capture.state().packages.get(name))
                .is_some_and(|package| {
                    info::user_state(package, apps_filter::user_id(uid)).instant_app
                })
        };
        if !notify {
            return Ok(());
        }
        let Some(name) = name.filter(|name| {
            capture
                .scan()
                .owner()
                .settings
                .packages
                .iter()
                .any(|package| package.name == *name)
        }) else {
            return Ok(());
        };
        if reason < 0 || reason as usize >= crate::package::owner::usage::REASONS {
            return Ok(());
        }
        let time = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()),
            Err(before) => i64::try_from(before.duration().as_millis()).map(|millis| -millis),
        }
        .map_err(|_| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package usage clock overflow",
            )
        })?;
        drop(state);
        self.publish_package_usage(Some(&name), reason, time)
    }
}
impl System {
    pub(crate) fn package_log_process_start(
        &self,
        caller: i32,
        args: aim_service_aidl::android_content_pm_ipackagemanager::LogAppProcessStartIfNeeded,
    ) -> Result<()> {
        let capture = self.capture_package_queries()?;
        if crate::package::apps_filter::instant_app_package_name(capture.state(), caller)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?
            .is_some()
        {
            return Ok(());
        }
        let bridge = self.package_bootstrap()?;
        bridge
            .log_package_process_start(
                args.package_name,
                args.process_name,
                args.uid,
                args.seinfo,
                args.apk_file,
                args.pid,
            )
            .map_err(|error| match error {
                crate::package::bootstrap::OwnerError::Owner(error) => error,
                error => Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package process log owner: {error:?}"),
                ),
            })?;
        self.check_package_bootstrap(&bridge)
    }
}
impl System {
    pub(crate) fn package_install_monitor_events(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        before: &Arc<crate::package::scan_snapshot::Snapshot>,
        after: &Arc<crate::package::scan_snapshot::Snapshot>,
        receipt: &crate::package::installer::pipeline::PublishedInstall,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        crate::package::installer::install_events::validate_monitor_publication(after,capture.scan(),receipt)
            .map_err(|error|Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,error))?;
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state()).map_err(|error| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("install monitor visibility: {error:?}"),
            )
        })?;
        let events = self.package_events()?;
        let changes = {
            let state = self.package_bootstrap.lock().unwrap();
            state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .map(|current| current.changes.clone())
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "install changes bootstrap owner changed",
                    )
                })?
        };
        let mut changed_users: Vec<(String, Vec<i32>)> = Vec::new();
        for package in &receipt.packages {
            let user = i32::try_from(package.user)
                .map_err(|_| Exception::illegal_argument("invalid install changes user"))?;
            if let Some((_, users)) = changed_users
                .iter_mut()
                .find(|(name, _)| name == &package.name)
            {
                if !users.contains(&user) {
                    users.push(user);
                }
            } else {
                changed_users.push((package.name.clone(), vec![user]));
            }
        }
        let observers = self.package_observer_owner(bridge)?;
        for (name, users) in changed_users {
            changes.update(&name, &users);
            let setting = after.owner().settings.packages.iter().find(|setting| setting.name == name)
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "install observer package unavailable"))?;
            let update = before.owner().settings.packages.iter().any(|setting| setting.name == name);
            let all_new = before.owner().scanned_user_states(&name)
                .is_none_or(|states| users.iter().all(|user| states.get(user).is_some_and(|state| !state.installed)));
            if update || !all_new {
                observers.changed(&name, setting.app_id)?;
            } else { observers.added(&name, setting.app_id)?; }
        }
        for package in &receipt.packages {
            let user = i32::try_from(package.user)
                .map_err(|_| Exception::illegal_argument("invalid install monitor user"))?;
            let setting = after
                .owner()
                .settings
                .packages
                .iter()
                .find(|setting| setting.name == package.name)
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "install monitor setting unavailable",
                    )
                })?;
            if setting.version_code != package.version_code
                || !after
                    .owner()
                    .scanned_user_states(&package.name)
                    .and_then(|users| users.get(&user))
                    .is_some_and(|state| state.installed)
            {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "install monitor receipt package differs",
                ));
            }
            let replacing = before
                .owner()
                .settings
                .packages
                .iter()
                .any(|old| old.name == package.name)
                && before
                    .owner()
                    .scanned_user_states(&package.name)
                    .and_then(|users| users.get(&user))
                    .is_some_and(|state| state.installed);
            let uid = crate::package::info::uid(user, setting.app_id);
            let actions: &[&str] = if replacing {
                &[
                    "android.intent.action.PACKAGE_REMOVED",
                    "android.intent.action.PACKAGE_ADDED",
                ]
            } else {
                &["android.intent.action.PACKAGE_ADDED"]
            };
            for action in actions {
                let payload = bridge
                    .package_monitor_result(action, &package.name, user, uid, replacing)
                    .map_err(|error| match error {
                        crate::package::bootstrap::OwnerError::Owner(error) => error,
                        error => Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("install monitor result owner: {error:?}"),
                        ),
                    })?;
                let bytes = payload.data().to_vec();
                let objects = payload.objects().to_vec();
                let query_capture = capture.clone();
                let resolution = resolution.clone();
                let name = package.name.clone();
                events.notify(
                    action,
                    user,
                    None,
                    Arc::new(move |listener_uid| {
                        let query = crate::package::query::Query {
                            state: query_capture.state(),
                            filter: &resolution.apps_filter,
                            calling_uid: listener_uid,
                        };
                        if query
                            .filtered_including_uninstalled(query.state.packages.get(&name), user)
                            .map_err(|error| {
                                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
                            })?
                        {
                            return Ok(None);
                        }
                        let mut result = Parcel::new();
                        result.write_raw(&bytes, &objects);
                        Ok(Some(result))
                    }),
                )?;
            }
        }
        self.check_package_bootstrap(bridge)
    }
}
impl System {
    /// Root's install factory passes this retained environment directly to
    /// pipeline::Native; every successful receipt then drives real callbacks.
    pub(crate) fn package_install_event_environment(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        environment: Arc<dyn crate::package::installer::pipeline::Environment>,
    ) -> Result<Arc<dyn crate::package::installer::pipeline::Environment>> {
        self.check_package_bootstrap(bridge)?;
        self.package_events()?;
        Ok(crate::package::installer::install_events::WithEvents::new(
            environment,
            self,
            bridge.clone(),
        ))
    }
}
