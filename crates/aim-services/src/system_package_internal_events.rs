//! Original PackageManagerInternal event contracts over current native owners.
use super::*;
fn event_error(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
impl System {
    pub(crate) fn internal_add_isolated_uid(
        &self,
        isolated_uid: i32,
        owner_uid: i32,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        self.internal_isolated_owner_change(isolated_uid, Some(owner_uid))
    }
    pub(crate) fn internal_remove_isolated_uid(
        &self,
        isolated_uid: i32,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        self.internal_isolated_owner_change(isolated_uid, None)
    }
    fn internal_isolated_owner_change(&self, isolated: i32, owner: Option<i32>) -> Result<()> {
        use crate::package::scan_snapshot::Error;
        loop {
            let (bridge,snapshots,capture,canonical)={
                let state=self.package_bootstrap.lock().unwrap();
                let current=state.current.as_ref().ok_or_else(||event_error("isolated owner bootstrap unavailable"))?;
                let snapshots=current.snapshots.clone().ok_or_else(||event_error("isolated canonical store unavailable"))?;
                let canonical=snapshots.capture();
                (current.bridge.clone(),snapshots,current.queries.clone().ok_or_else(||event_error("isolated owner capture unavailable"))?,canonical)
            };
            self.check_package_bootstrap(&bridge)?;
            let coherent=Arc::ptr_eq(capture.scan(),&canonical);
            let delta=if coherent{None}else{Some(capture.with_isolated_owner_delta(isolated,owner).map_err(event_error)?)};
            let prepared=if coherent{Some(capture.prepare_isolated_owner_delta(isolated,owner).map_err(event_error)?)}else{None};
            let mut state=self.package_bootstrap.lock().unwrap();
            let current=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,&bridge))
                .ok_or_else(||event_error("isolated owner bootstrap retired"))?;
            if !current.snapshots.as_ref().is_some_and(|latest|Arc::ptr_eq(latest,&snapshots))
                || !current.queries.as_ref().is_some_and(|latest|Arc::ptr_eq(latest,&capture)){continue;}
            if let Some(update)=prepared{
                match snapshots.publish_validated_store_after(&canonical,&update.store){Err(Error::Stale)=>continue,
                    Err(error)=>return Err(event_error(format!("isolated canonical publication: {error:?}"))),Ok(_)=>{}}
                let version=update.capture.scan().version();current.queries=Some(update.capture);
                if let Some(page)=&current.version_page{page.publish(version);}state.version=version;
            }else{
                // Context-only delta: preserve the actual durable/new-code scan.
                // The install Context builder sees this changed query base and
                // must retry its projection before publishing the full capture.
                if !Arc::ptr_eq(&snapshots.capture(),&canonical){continue;}
                current.queries=delta;
            }
            return Ok(());
        }
    }
    /// Internal overload bypasses the public IPM instant-caller policy, exactly
    /// as PackageManagerInternalImpl -> notifyPackageUseInternal does.
    pub(crate) fn internal_notify_package_use(
        &self,
        package_name: Option<String>,
        reason: i32,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        let time = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()),
            Err(before) => i64::try_from(before.duration().as_millis()).map(|time| -time),
        }.map_err(|_| event_error("usage clock overflow"))?;
        self.publish_package_usage(package_name.as_deref(), reason, time)
    }

    pub(super) fn publish_package_usage(&self, package_name: Option<&str>, reason: i32, time: i64) -> Result<()> {
        use crate::package::scan_snapshot::{Error, Store};
        loop {
            let (bridge, snapshots, capture, base) = {
                let state = self.package_bootstrap.lock().unwrap();
                let current = state.current.as_ref().ok_or_else(|| event_error("usage bootstrap unavailable"))?;
                let snapshots = current.snapshots.clone().ok_or_else(|| event_error("usage scan owner unavailable"))?;
                let base = snapshots.capture();
                (current.bridge.clone(), snapshots, current.queries.clone().ok_or_else(|| event_error("usage capture unavailable"))?, base)
            };
            self.check_package_bootstrap(&bridge)?;
            let Some(name) = package_name.filter(|name| base.usage().times(name).is_some()) else { return Ok(()); };
            let mut usage = base.usage().clone();
            usage.notify(name, reason, time);
            if usage == *base.usage() { return Ok(()); }
            // AMS holds its own locks here. Prepare outside the coordinator and
            // never wait for the install gate. During query lag, update only the
            // canonical owner; the installer projects this latest usage later.
            let projected = if Arc::ptr_eq(capture.scan(), &base) {
                Some(capture.prepare_usage_update(usage.clone()).map_err(event_error)?)
            } else { None };
            let candidate = if projected.is_some() {
                None
            } else {
                Some(Store::prepare_usage_store(&base, usage)
                    .map_err(|error| event_error(format!("usage canonical replica: {error:?}")))?)
            };
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or_else(|| event_error("usage bootstrap retired during preparation"))?;
            if !current.snapshots.as_ref().is_some_and(|latest| Arc::ptr_eq(latest, &snapshots)) { continue; }
            if !current.queries.as_ref().is_some_and(|latest| Arc::ptr_eq(latest, &capture)) { continue; }
            let candidate = projected.as_ref().map(|update| &update.store).unwrap_or_else(|| candidate.as_ref().unwrap());
            match snapshots.publish_validated_store_after(&base, candidate) {
                Err(Error::Stale) => continue,
                Err(error) => return Err(event_error(format!("usage canonical publication: {error:?}"))),
                Ok(_) => {}
            }
            if let Some(update) = projected {
                let version = update.capture.scan().version();
                current.queries = Some(update.capture);
                if let Some(page) = &current.version_page { page.publish(version); }
                state.version = version;
            }
            return Ok(());
        }
    }

    pub(crate) fn internal_notify_component_used(
        &self,
        package_name: Option<String>,
        user_id: i32,
        caller: Option<String>,
        debug_info: Option<String>,
        calling_uid: i32,
        calling_pid: i32,
    ) -> Result<()> {
        let install = self.package_install_guard();
        let (bridge, capture, dependencies) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_ref().ok_or_else(||event_error("component usage bootstrap unavailable"))?;
            (current.bridge.clone(), current.queries.clone().ok_or_else(||event_error("component usage capture unavailable"))?,
                current.mutations.clone().ok_or_else(||event_error("component usage mutation owner unavailable"))?)
        };
        self.check_package_bootstrap(&bridge)?;
        let Some(name) = package_name.filter(|name| {
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
        if capture
            .state()
            .packages
            .get(&name)
            .is_some_and(|package| crate::package::info::user_state(package, user_id).quarantined)
        {
            eprintln!(
                "Component is quarantined+suspended but being used: {} by {:?}, debugInfo: {:?}",
                name, caller, debug_info
            );
        }
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver
            .resolution(capture.state())
            .map_err(|error| event_error(format!("component usage resolution: {error:?}")))?;
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid,
        };
        let mut parcel = Parcel::new();
        aim_service_aidl::android_content_pm_ipackagemanager::SetPackageStoppedState {
            package_name: Some(name),
            stopped: false,
            user_id,
        }
        .write(&mut parcel);
        let mut call = Call {
            code: aim_service_aidl::android_content_pm_ipackagemanager::SET_PACKAGE_STOPPED_STATE,
            flags: 0,
            sender_pid: calling_pid,
            sender_euid: calling_uid as u32,
            data: Reader::new(parcel.data(), parcel.objects()),
        };
        let prepared = crate::package::mutation_dispatch::prepare(call.code, &mut call.data, &query, calling_pid, &dependencies)
            .ok_or_else(||event_error("component usage stopped-state owner unavailable"))??;
        let reply = self.publish_prepared_package_mutation_guarded(&bridge, &capture, prepared, &dependencies, calling_uid, install)?;
        let mut reader = Reader::new(reply.data(), reply.objects());
        reader
            .read_exception()
            .map_err(|status| event_error(format!("component usage reply {status}")))??;
        Ok(())
    }
}
impl System {
    pub(crate) fn internal_prune_instant_apps(
        self: &Arc<Self>,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        let actual = bridge
            .package_maintenance()
            .map_err(|error| event_error(format!("instant maintenance owner: {error:?}")))?;
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let current = Arc::new(move || {
            let system = weak
                .upgrade()
                .ok_or_else(|| event_error("instant prune system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            system.capture_package_queries()
        });
        let installer = {
            let state = self.package_bootstrap.lock().unwrap();
            state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .and_then(|current| current.installer.as_ref().map(|(owner, _)| owner.clone()))
                .ok_or_else(|| event_error("instant prune installer unavailable"))?
        };
        let removal = self.public_package_removal()?.controller.clone();
        let owner = crate::package::diagnostics_storage::Owner::new(
            Arc::new(actual),
            current,
            removal,
            installer,
        );
        if let Err(error) = owner.prune_instant_apps() {
            // The source entry point logs IOException and preserves completed
            // removals; failures from other owners are propagated explicitly.
            match error {
                crate::package::diagnostics_storage::Error::Io(error) => {
                    eprintln!("Error pruning installed and uninstalled instant apps: {error}")
                }
                crate::package::diagnostics_storage::Error::Owner(error) => return Err(error),
            }
        }
        self.check_package_bootstrap(&bridge)
    }
}
impl System {
    fn internal_event_transport(
        &self,
    ) -> Result<(Arc<crate::package::bootstrap::Bridge>, Arc<Strong>)> {
        let bridge = self.package_bootstrap()?;
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        let mut request = Parcel::new();
        api::GetPackageInternalEventsBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(api::GET_PACKAGE_INTERNAL_EVENTS_BRIDGE, &request, false)
            .map_err(|status| event_error(format!("internal event owner transport {status}")))?;
        let mut reader = reply.reader();
        let binder = api::read_get_package_internal_events_bridge_reply(&mut reader)
            .map_err(|status| event_error(format!("internal event owner reply {status}")))??;
        if reader.remaining() != 0 {
            return Err(event_error("internal event owner reply tail"));
        }
        let node = reply
            .retain_remote_binder(binder.ok_or_else(|| event_error("internal event owner absent"))?)
            .map_err(|status| event_error(format!("internal event owner capability {status}")))?;
        self.check_package_bootstrap(&bridge)?;
        Ok((bridge, Arc::new(node)))
    }
    fn internal_event_allowlist(&self, name: &str, user: i32) -> Result<Option<Vec<i32>>> {
        let capture = self.capture_package_queries()?;
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver
            .resolution(capture.state())
            .map_err(|error| event_error(format!("event visibility owner {error:?}")))?;
        let Some(target) = capture.state().packages.get(name) else {
            return Ok(None);
        };
        if resolution.apps_filter.is_force_queryable(target.app_id) {
            return Ok(None);
        }
        let mut ids = std::collections::BTreeSet::new();
        for package in capture.state().packages.values() {
            if package.app_id < 10000 {
                continue;
            }
            let uid = crate::package::info::uid(user, package.app_id);
            let query = crate::package::query::Query {
                state: capture.state(),
                filter: &resolution.apps_filter,
                calling_uid: uid,
            };
            if !query
                .filtered_including_uninstalled(Some(target), user)
                .map_err(|error| event_error(error.0))?
            {
                ids.insert(package.app_id);
            }
        }
        Ok(Some(ids.into_iter().collect()))
    }
    pub(crate) fn internal_send_package_restarted_broadcast(
        self: &Arc<Self>,
        package_name: Option<String>,
        uid: i32,
        flags: i32,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackageinternaleventsbridge as api;
        let name = package_name.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "packageName is null",
            )
        })?;
        let (bridge, node) = self.internal_event_transport()?;
        let mut request = Parcel::new();
        api::PrepareRestarted {
            package_name: Some(name.clone()),
            uid,
            flags,
        }
        .write(&mut request);
        let reply = node
            .transact(api::PREPARE_RESTARTED, &request, false)
            .map_err(|status| event_error(format!("restart event prepare {status}")))?;
        let mut reader = reply.reader();
        let record = api::read_prepare_restarted_reply(&mut reader)
            .map_err(|status| event_error(format!("restart event record {status}")))??
            .ok_or_else(|| event_error("restart event null record"))?;
        if reader.remaining() != 0 {
            return Err(event_error("restart event reply tail"));
        }
        self.internal_deliver_event(
            bridge,
            node,
            "android.intent.action.PACKAGE_RESTARTED",
            name,
            crate::package::apps_filter::user_id(uid),
            false,
            record,
        )
    }
    pub(crate) fn internal_send_package_data_cleared_broadcast(
        self: &Arc<Self>,
        package_name: Option<String>,
        uid: i32,
        user_id: i32,
        restore: bool,
        instant: bool,
        _calling_uid: i32,
        _calling_pid: i32,
    ) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackageinternaleventsbridge as api;
        let name = package_name.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "packageName is null",
            )
        })?;
        let (bridge, node) = self.internal_event_transport()?;
        let mut request = Parcel::new();
        api::PrepareDataCleared {
            package_name: Some(name.clone()),
            uid,
            user_id,
            restore,
            instant,
        }
        .write(&mut request);
        let reply = node
            .transact(api::PREPARE_DATA_CLEARED, &request, false)
            .map_err(|status| event_error(format!("data cleared event prepare {status}")))?;
        let mut reader = reply.reader();
        let record = api::read_prepare_data_cleared_reply(&mut reader)
            .map_err(|status| event_error(format!("data cleared event record {status}")))??
            .ok_or_else(|| event_error("data cleared event null record"))?;
        if reader.remaining() != 0 {
            return Err(event_error("data cleared event reply tail"));
        }
        self.internal_deliver_event(
            bridge,
            node,
            "android.intent.action.PACKAGE_DATA_CLEARED",
            name,
            user_id,
            instant,
            record,
        )
    }
    fn internal_deliver_event(
        self: &Arc<Self>,
        bridge: Arc<crate::package::bootstrap::Bridge>,
        node: Arc<Strong>,
        action: &'static str,
        name: String,
        user: i32,
        instant: bool,
        record: Vec<u8>,
    ) -> Result<()> {
        let mut reader = Reader::new(&record, &[]);
        let asynchronous = reader
            .read_bool()
            .map_err(|status| event_error(format!("event mode {status}")))?;
        let intent = aim_service_aidl::read_byte_array(&mut reader)
            .map_err(|status| event_error(format!("event Intent {status}")))?
            .ok_or_else(|| event_error("event Intent absent"))?;
        let result = aim_service_aidl::read_byte_array(&mut reader)
            .map_err(|status| event_error(format!("event monitor result {status}")))?
            .ok_or_else(|| event_error("event monitor result absent"))?;
        if reader.remaining() != 0 {
            return Err(event_error("event record tail"));
        }
        let allow = self.internal_event_allowlist(&name, user)?;
        let events = self.package_events()?;
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let callback_events = events.clone();
        let deliver = move || -> Result<()> {
            let system = weak
                .upgrade()
                .ok_or_else(|| event_error("internal event system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            use aim_service_aidl::dev_aim_server_ipackageinternaleventsbridge as api;
            let mut request = Parcel::new();
            api::Broadcast {
                intent: Some(intent),
                user_id: user,
                instant,
                allow_list: allow.clone(),
            }
            .write(&mut request);
            let reply = node
                .transact(api::BROADCAST, &request, false)
                .map_err(|status| event_error(format!("actual AM package broadcast {status}")))?;
            let mut reader = reply.reader();
            api::read_broadcast_reply(&mut reader).map_err(|status| {
                event_error(format!("actual AM package broadcast reply {status}"))
            })??;
            if reader.remaining() != 0 {
                return Err(event_error("actual AM package broadcast reply tail"));
            }
            callback_events.notify(
                action,
                user,
                allow,
                Arc::new(move |_| {
                    let mut payload = Parcel::new();
                    payload.write_raw(&result, &[]);
                    Ok(Some(payload))
                }),
            )?;
            system.check_package_bootstrap(&retained)
        };
        if asynchronous {
            events.post(
                false,
                Box::new(move || {
                    if let Err(error) = deliver() {
                        eprintln!("package restarted broadcast: {}", error.message);
                    }
                }),
            )
        } else {
            deliver()
        }
    }
}
impl System {
    pub(crate) fn internal_shutdown(&self, _calling_uid: i32, _calling_pid: i32) -> Result<()> {
        let bridge = self.package_bootstrap()?;
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
        let mut request = Parcel::new();
        bootstrap::GetPackageShutdownBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(bootstrap::GET_PACKAGE_SHUTDOWN_BRIDGE, &request, false)
            .map_err(|status| event_error(format!("shutdown owner {status}")))?;
        let mut reader = reply.reader();
        let binder = bootstrap::read_get_package_shutdown_bridge_reply(&mut reader)
            .map_err(|status| event_error(format!("shutdown owner reply {status}")))??
            .ok_or_else(|| event_error("shutdown statistics owner unavailable"))?;
        if reader.remaining() != 0 {
            return Err(event_error("shutdown owner reply tail"));
        }
        let owner = reply
            .retain_remote_binder(binder)
            .map_err(|status| event_error(format!("shutdown statistics capability {status}")))?;
        use aim_service_aidl::dev_aim_server_ipackageshutdownbridge as stats;
        let mut request = Parcel::new();
        stats::WriteStatisticsNow {}.write(&mut request);
        let result = owner
            .transact(stats::WRITE_STATISTICS_NOW, &request, false)
            .map_err(|status| event_error(format!("shutdown statistics write {status}")))?;
        let mut reader = result.reader();
        stats::read_write_statistics_now_reply(&mut reader)
            .map_err(|status| event_error(format!("shutdown statistics reply {status}")))??;
        if reader.remaining() != 0 {
            return Err(event_error("shutdown statistics reply tail"));
        }
        self.check_package_bootstrap(&bridge)?;
        let (capture, persistence) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, &bridge))
                .ok_or_else(|| event_error("shutdown bootstrap changed"))?;
            (
                current
                    .queries
                    .clone()
                    .ok_or_else(|| event_error("shutdown usage capture unavailable"))?,
                current
                    .persistence
                    .clone()
                    .ok_or_else(|| event_error("shutdown package writer unavailable"))?,
            )
        };
        let mut disk = persistence.lock().unwrap();
        disk.validate_committed_scan(capture.scan().owner())
            .map_err(|error| event_error(error.to_string()))?;
        if let Err(error) = disk.write_usage_now(capture.scan().usage()) {
            eprintln!("Failed to write package usage times: {}", error.message);
        }
        drop(disk);
        self.flush_pending_internal_settings()?;
        self.check_package_bootstrap(&bridge)
    }
}

#[cfg(test)]
mod component_use_concurrency_tests {
    use super::*;
    use crate::package::{scan::SigningScan, scan_snapshot::Store, owner::usage::Usage,
        settings::{Settings,Package},restrictions::UserState,write::mutation::{Plan,Change}};
    use std::collections::BTreeMap;
    #[test]
    fn component_use_prepares_after_concurrent_app_data_publication() {
        use aim_binder_driver::{Driver,Device,Credentials};
        use aim_binder_host::local::LocalProcess;
        let driver=Driver::new();
        let process=LocalProcess::open(&driver,Device::Binder,Credentials{pid:98001,euid:1000,security_context:None});
        let system=System::new(process,&[]);
        let mut owner=SigningScan::new(&Default::default(),&Settings{packages:vec![Package{name:"p".into(),app_id:10100,..Default::default()}],..Default::default()},36).unwrap();
        owner.capture_user_states(BTreeMap::from([(("p".into(),false),crate::package::scan::CapturedUsers{
            states:BTreeMap::from([(0,UserState{stopped:true,not_launched:true,..Default::default()})]),active_aliases:Default::default(),
        })])).unwrap();
        let store=Arc::new(Store::new(owner,Usage::new(["p"])).unwrap());
        let old=store.capture();
        let app_data_gate=system.package_install_guard();
        let (started,ready)=std::sync::mpsc::channel();
        let component_system=system.clone();let component_store=store.clone();
        let component=std::thread::spawn(move||{
            started.send(()).unwrap();
            let _gate=component_system.package_install_guard();
            let base=component_store.capture();
            assert_eq!(base.version(),2);
            let mut owner=base.owner().clone();
            Plan{package:"p".into(),user:Some(0),change:Change::Stopped{stopped:false,not_launched:false,first_launch_installer:None,was_stopped:true}}.apply_scan(&mut owner).unwrap();
            component_store.publish(&base,owner,base.usage().clone()).unwrap()
        });
        ready.recv().unwrap();
        let mut owner=old.owner().clone();
        let mut user=owner.scanned_user_states("p").unwrap()[&0].clone();user.ce_data_inode=77;user.de_data_inode=88;
        owner.set_user_state("p",0,user).unwrap();store.publish(&old,owner,old.usage().clone()).unwrap();
        drop(app_data_gate);
        let current=component.join().unwrap();
        let state=&current.owner().scanned_user_states("p").unwrap()[&0];
        assert!(!state.stopped);assert!(!state.not_launched);
        assert_eq!(state.ce_data_inode,77);assert_eq!(state.de_data_inode,88);
        assert!(old.owner().scanned_user_states("p").unwrap()[&0].stopped);
        assert_eq!(current.version(),3);
    }
}
