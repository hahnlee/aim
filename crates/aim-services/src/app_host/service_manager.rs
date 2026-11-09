//! Lookups in servicemanager (`android.os.IServiceManager` at
//! `android-16.0.0_r1`) as the original clients make them: libbinder's
//! and `ServiceManager`'s `checkService`, `waitForService` and
//! `waitForDeclaredService`.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, LocalProcess, Received, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, DEAD_OBJECT, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::android_os_iservicecallback as sc;
use aim_service_aidl::android_os_iservicemanager as sm;

/// `Service.serviceWithMetadata`: the union's tag of a binder.
const SERVICE_WITH_METADATA: i32 = 0;

/// The service `name` from servicemanager (`checkService`).
pub(super) fn check_service(process: &LocalProcess, name: &str) -> Result<Strong, String> {
    let mut data = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    match sm::read_check_service_reply(&mut reply.reader()).map_err(|s| format!("{name}: {s}"))? {
        Ok(Some(Binder::Handle(handle))) => Ok(process.strong(handle)),
        _ => Err(format!("no {name}")),
    }
}

/// `android.os.Service` as servicemanager answers `getService2`: the
/// service's binder, if registered; an `IAccessor`, which the services
/// the app host waits for are not served through, is refused.
struct Found(Option<Binder>);

impl ReadParcelable for Found {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        match r.read_i32()? {
            SERVICE_WITH_METADATA => {
                super::display::present(r)?;
                // `isLazyService` only decides libbinder's caching.
                super::display::parcelable(r, |r| r.read_binder()).map(Found)
            }
            // `accessor`.
            _ => Err(BAD_VALUE),
        }
    }
}

/// `getService2`: the service `name`, now; servicemanager has init start
/// it first if it is a lazy one.
fn get_service2(process: &LocalProcess, name: &str) -> Result<Option<Strong>, String> {
    let mut data = Parcel::new();
    sm::GetService2 {
        name: Some(name.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::GET_SERVICE2, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    match sm::read_get_service2_reply::<Found>(&mut reply.reader())
        .map_err(|s| format!("{name}: {s}"))?
    {
        Ok(Some(Found(Some(Binder::Handle(handle))))) => Ok(Some(process.strong(handle))),
        Ok(Some(Found(None))) => Ok(None),
        Ok(Some(Found(Some(Binder::Local(_))))) => Err(format!("{name}: a node of this process")),
        Ok(None) => Err(format!("{name}: no Service")),
        Err(e) => Err(format!("{name}: {e:?}")),
    }
}

/// The service `name` as libbinder's `waitForService` finds it: now if
/// registered (`getService2`, which has init start a lazy service), else
/// once servicemanager reports its registration
/// (`registerForNotifications`). As libbinder, it looks again each second
/// it waited, for a report no pool thread was free to take or a lazy
/// service that died before init started it, and waits without a bound.
pub(super) fn wait_for_service(process: &Arc<LocalProcess>, name: &str) -> Result<Strong, String> {
    if let Some(service) = get_service2(process, name)? {
        return Ok(service);
    }
    let waiter = Arc::new(Waiter {
        process: Arc::downgrade(process),
        registered: Mutex::new(None),
        changed: Condvar::new(),
    });
    let callback = process.add_callback(waiter.clone());
    if let Err(e) = notifications(process, name, callback, sm::REGISTER_FOR_NOTIFICATIONS) {
        process.drop_callback(callback);
        return Err(e);
    }
    let _registration = Registration {
        process,
        name,
        callback,
    };
    Ok(loop {
        let registered = waiter.registered.lock().unwrap();
        let (mut registered, _) = waiter
            .changed
            .wait_timeout_while(registered, Duration::from_secs(1), |r| r.is_none())
            .unwrap();
        if let Some(service) = registered.take() {
            break service;
        }
        drop(registered);
        eprintln!("app host: waited one second for {name}");
        if let Some(service) = get_service2(process, name)? {
            break service;
        }
    })
}

/// `waitForDeclaredService`: the service `name` once registered
/// ([`wait_for_service`]) if the device's VINTF manifests declare it
/// (`isDeclared`), else none.
pub(super) fn wait_for_declared_service(
    process: &Arc<LocalProcess>,
    name: &str,
) -> Result<Option<Strong>, String> {
    let mut data = Parcel::new();
    sm::IsDeclared {
        name: Some(name.into()),
    }
    .write(&mut data);
    let reply = process
        .transact(0, sm::IS_DECLARED, &data, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    let declared = sm::read_is_declared_reply(&mut reply.reader())
        .map_err(|s| format!("isDeclared({name}): {s}"))?
        .map_err(|e| format!("isDeclared({name}): {e:?}"))?;
    match declared {
        true => wait_for_service(process, name).map(Some),
        false => Ok(None),
    }
}

/// A [`wait_for_service`]'s registration, unregistered however the wait
/// ends; as libbinder's deferred unregistration, a failure to does not
/// fail the wait.
struct Registration<'a> {
    process: &'a LocalProcess,
    name: &'a str,
    callback: Binder,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let unregistered = notifications(
            self.process,
            self.name,
            self.callback,
            sm::UNREGISTER_FOR_NOTIFICATIONS,
        );
        if let Err(e) = unregistered {
            eprintln!("app host: {e}");
        }
    }
}

/// `registerForNotifications` or `unregisterForNotifications` of
/// `callback` for `name`.
fn notifications(
    process: &LocalProcess,
    name: &str,
    callback: Binder,
    code: u32,
) -> Result<(), String> {
    let mut data = Parcel::new();
    // Both calls' arguments are the same.
    sm::RegisterForNotifications {
        name: Some(name.into()),
        callback: Some(callback),
    }
    .write(&mut data);
    let reply = process
        .transact(0, code, &data, false)
        .map_err(|s| format!("servicemanager#{code}: {s}"))?;
    sm::read_register_for_notifications_reply(&mut reply.reader())
        .map_err(|s| format!("servicemanager#{code} reply: {s}"))?
        .map_err(|e| format!("servicemanager#{code} for {name}: {e:?}"))
}

/// The `IServiceCallback` of a [`wait_for_service`].
struct Waiter {
    process: Weak<LocalProcess>,
    registered: Mutex<Option<Strong>>,
    changed: Condvar,
}

impl Service for Waiter {
    fn descriptor(&self) -> &str {
        sc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != sc::ON_REGISTRATION {
            return Err(UNKNOWN_TRANSACTION);
        }
        let Some(Binder::Handle(handle)) = sc::OnRegistration::read(&mut call.data)?.binder else {
            return Err(BAD_VALUE);
        };
        let process = self.process.upgrade().ok_or(DEAD_OBJECT)?;
        *self.registered.lock().unwrap() = Some(process.strong(handle));
        self.changed.notify_all();
        Ok(Parcel::new())
    }
}

/// A call to the service `name` as `ServiceManager.getService` finds it:
/// another process's, or a node of this process itself.
pub(super) fn transact_service(
    process: &LocalProcess,
    name: &str,
    code: u32,
    data: &Parcel,
) -> Result<Received, String> {
    let mut lookup = Parcel::new();
    sm::CheckService {
        name: Some(name.into()),
    }
    .write(&mut lookup);
    let reply = process
        .transact(0, sm::CHECK_SERVICE, &lookup, false)
        .map_err(|s| format!("servicemanager: {s}"))?;
    let called = match sm::read_check_service_reply(&mut reply.reader())
        .map_err(|s| format!("{name}: {s}"))?
    {
        Ok(Some(Binder::Handle(handle))) => process.transact(handle, code, data, false),
        Ok(Some(Binder::Local(ptr))) => process
            .local_service(ptr)
            .ok_or_else(|| format!("{name}: no local node"))?
            .transact(code, data, false),
        _ => return Err(format!("no {name}")),
    };
    called.map_err(|s| format!("{name}#{code}: {s}"))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
    use aim_service_aidl::WriteParcelable;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    /// What a [`Registry`]'s servicemanager was asked for.
    #[derive(Debug, PartialEq)]
    pub(in super::super) enum Asked {
        /// A lazy service's start (`getService2` before it registered).
        Start(String),
        /// Its registrations (`registerForNotifications`).
        Notifications(String),
    }

    /// servicemanager's naming, declarations, lazy services and
    /// registration notifications.
    struct Manager {
        process: Weak<LocalProcess>,
        names: Mutex<HashMap<String, Strong>>,
        callbacks: Mutex<Vec<(String, Strong)>>,
        declared: Vec<String>,
        lazy: Vec<String>,
        /// Whether a registration is reported to its callbacks.
        notifies: bool,
        /// Lookups fail, as a dead servicemanager's would.
        failing: AtomicBool,
        asked: mpsc::Sender<Asked>,
    }

    /// `Service.serviceWithMetadata`, of a binder that is not lazy.
    struct Metadata(Option<Binder>);

    impl WriteParcelable for Metadata {
        fn write_to(&self, p: &mut Parcel) {
            p.write_i32(SERVICE_WITH_METADATA);
            p.write_i32(1);
            let start = p.position();
            p.write_i32(0);
            p.write_binder(self.0);
            p.write_bool(false);
            p.set_i32_at(start, (p.position() - start) as i32);
        }
    }

    impl Service for Manager {
        fn descriptor(&self) -> &str {
            sm::DESCRIPTOR
        }

        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let process = self.process.upgrade().unwrap();
            let ask = |asked| self.asked.send(asked).unwrap();
            let mut p = Parcel::new();
            match call.code {
                sm::ADD_SERVICE => {
                    let a = sm::AddService::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.service) else {
                        return Err(BAD_VALUE);
                    };
                    let callbacks = self.callbacks.lock().unwrap();
                    for (_, callback) in callbacks.iter().filter(|c| self.notifies && c.0 == name) {
                        let mut data = Parcel::new();
                        sc::OnRegistration {
                            name: Some(name.clone()),
                            binder: Some(Binder::Handle(h)),
                        }
                        .write(&mut data);
                        callback.transact(sc::ON_REGISTRATION, &data, true)?;
                    }
                    self.names.lock().unwrap().insert(name, process.strong(h));
                    sm::write_add_service_reply(&mut p);
                }
                sm::GET_SERVICE2 => {
                    if self.failing.load(Ordering::Relaxed) {
                        return Err(DEAD_OBJECT);
                    }
                    let name = sm::GetService2::read(&mut call.data)?.name.unwrap();
                    let found = self.names.lock().unwrap().get(&name).map(Strong::binder);
                    if found.is_none() && self.lazy.contains(&name) {
                        ask(Asked::Start(name));
                    }
                    sm::write_get_service2_reply(&mut p, Some(&Metadata(found)));
                }
                sm::IS_DECLARED => {
                    let name = sm::IsDeclared::read(&mut call.data)?.name.unwrap();
                    sm::write_is_declared_reply(&mut p, self.declared.contains(&name));
                }
                sm::REGISTER_FOR_NOTIFICATIONS => {
                    let a = sm::RegisterForNotifications::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.callback) else {
                        return Err(BAD_VALUE);
                    };
                    self.callbacks
                        .lock()
                        .unwrap()
                        .push((name.clone(), process.strong(h)));
                    ask(Asked::Notifications(name));
                    sm::write_register_for_notifications_reply(&mut p);
                }
                sm::UNREGISTER_FOR_NOTIFICATIONS => {
                    let a = sm::UnregisterForNotifications::read(&mut call.data)?;
                    let (Some(name), Some(Binder::Handle(h))) = (a.name, a.callback) else {
                        return Err(BAD_VALUE);
                    };
                    self.callbacks
                        .lock()
                        .unwrap()
                        .retain(|c| (c.0.as_str(), c.1.handle) != (name.as_str(), h));
                    sm::write_unregister_for_notifications_reply(&mut p);
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            Ok(p)
        }
    }

    struct NoMemory;

    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn get_file(&mut self, _: u32) -> Result<File, i32> {
            Err(errno::EBADF)
        }
        fn install_file(&mut self, _: File) -> Result<u32, i32> {
            Err(errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            panic!("no files")
        }
    }

    /// servicemanager, a server's process and the app host's.
    pub(in super::super) struct Registry {
        manager: Arc<LocalProcess>,
        server: Arc<LocalProcess>,
        pub host: Arc<LocalProcess>,
        state: Arc<Manager>,
        asked: mpsc::Receiver<Asked>,
    }

    impl Registry {
        /// servicemanager of the services `declared` and the `lazy` ones,
        /// reporting registrations if it `notifies`.
        pub fn start(pid: i32, notifies: bool, declared: &[&str], lazy: &[&str]) -> Self {
            let driver = Driver::new();
            let open = |pid| {
                LocalProcess::open(
                    &driver,
                    Device::Binder,
                    Credentials {
                        pid,
                        euid: 1000,
                        security_context: None,
                    },
                )
            };
            let (manager, server, host) = (open(pid), open(pid + 1), open(pid + 2));
            let (asks, asked) = mpsc::channel();
            let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect();
            let state = Arc::new(Manager {
                process: Arc::downgrade(&manager),
                names: Mutex::new(HashMap::new()),
                callbacks: Mutex::new(Vec::new()),
                declared: names(declared),
                lazy: names(lazy),
                notifies,
                failing: AtomicBool::new(false),
                asked: asks,
            });
            let Binder::Local(ptr) = manager.add_service(state.clone()) else {
                unreachable!()
            };
            let mut object = FlatBinderObject {
                kind: BINDER_TYPE_BINDER,
                flags: 0,
                binder: ptr,
                cookie: ptr,
            }
            .encode();
            driver
                .ioctl(
                    manager.proc_handle(),
                    pid,
                    BINDER_SET_CONTEXT_MGR_EXT,
                    &mut object,
                    &mut NoMemory,
                )
                .unwrap();
            for process in [&manager, &server, &host] {
                process.start();
            }
            Self {
                manager,
                server,
                host,
                state,
                asked,
            }
        }

        /// What servicemanager is asked for next.
        pub fn asked(&self) -> Asked {
            self.asked.recv_timeout(Duration::from_secs(10)).unwrap()
        }

        /// `service` registered as `name` by the server's process.
        pub fn register(&self, name: &str, service: Arc<dyn Service>) {
            let mut p = Parcel::new();
            sm::AddService {
                name: Some(name.into()),
                service: Some(self.server.add_service(service)),
                allow_isolated: false,
                dump_priority: 0,
            }
            .write(&mut p);
            let reply = self.server.transact(0, sm::ADD_SERVICE, &p, false).unwrap();
            sm::read_add_service_reply(&mut reply.reader())
                .unwrap()
                .unwrap();
        }

        /// The processes shut down, once every wait unregistered.
        pub fn finish(self) {
            assert!(self.state.callbacks.lock().unwrap().is_empty());
            self.state.names.lock().unwrap().clear();
            for process in [&self.host, &self.server, &self.manager] {
                process.shutdown().unwrap();
            }
        }
    }

    const NAME: &str = "test.waited";
    const DESCRIPTOR: &str = "test.IWaited";

    /// The waited-for service: its one call answers 7.
    struct Waited;

    impl Service for Waited {
        fn descriptor(&self) -> &str {
            DESCRIPTOR
        }

        fn transact(&self, _: &mut Call<'_>) -> Reply {
            let mut p = Parcel::new();
            p.write_i32(7);
            Ok(p)
        }
    }

    /// A wait of the app host's for [`NAME`], answering its call.
    fn wait(
        registry: &Registry,
        lookup: fn(&Arc<LocalProcess>, &str) -> Result<Option<Strong>, String>,
    ) -> std::thread::JoinHandle<Result<Option<i32>, String>> {
        let host = registry.host.clone();
        std::thread::spawn(move || {
            let Some(service) = lookup(&host, NAME)? else {
                return Ok(None);
            };
            let reply = service.transact(1, &Parcel::new(), false);
            Ok(Some(reply.unwrap().reader().read_i32().unwrap()))
        })
    }

    fn waited(process: &Arc<LocalProcess>, name: &str) -> Result<Option<Strong>, String> {
        wait_for_service(process, name).map(Some)
    }

    #[test]
    fn a_wait_before_the_registration_ends_once_it_is_reported() {
        let registry = Registry::start(97261, true, &[], &[]);
        let waiting = wait(&registry, waited);
        assert_eq!(registry.asked(), Asked::Notifications(NAME.into()));
        registry.register(NAME, Arc::new(Waited));
        assert_eq!(waiting.join().unwrap(), Ok(Some(7)));
        registry.finish();
    }

    /// A registration whose report is never delivered is found by the
    /// lookup each second of waiting.
    #[test]
    fn a_wait_looks_again_each_second() {
        let registry = Registry::start(97271, false, &[], &[]);
        let waiting = wait(&registry, waited);
        assert_eq!(registry.asked(), Asked::Notifications(NAME.into()));
        registry.register(NAME, Arc::new(Waited));
        assert_eq!(waiting.join().unwrap(), Ok(Some(7)));
        registry.finish();
    }

    /// A lookup failing while waiting fails the wait, which unregisters.
    #[test]
    fn a_failed_wait_unregisters() {
        let registry = Registry::start(97281, false, &[], &[]);
        let waiting = wait(&registry, waited);
        assert_eq!(registry.asked(), Asked::Notifications(NAME.into()));
        registry.state.failing.store(true, Ordering::Relaxed);
        let error = waiting.join().unwrap().unwrap_err();
        assert!(error.starts_with("servicemanager: "), "{error}");
        registry.finish();
    }

    /// A lazy service is started by the first lookup, and waited for.
    #[test]
    fn a_wait_starts_a_lazy_service() {
        let registry = Registry::start(97291, true, &[], &[NAME]);
        let waiting = wait(&registry, waited);
        assert_eq!(registry.asked(), Asked::Start(NAME.into()));
        assert_eq!(registry.asked(), Asked::Notifications(NAME.into()));
        registry.register(NAME, Arc::new(Waited));
        assert_eq!(waiting.join().unwrap(), Ok(Some(7)));
        registry.finish();
    }

    /// An undeclared service is none at once; a declared one is waited for.
    #[test]
    fn only_a_declared_service_is_waited_for() {
        let registry = Registry::start(97301, true, &[NAME], &[]);
        assert!(
            wait_for_declared_service(&registry.host, "test.undeclared")
                .unwrap()
                .is_none()
        );
        let waiting = wait(&registry, wait_for_declared_service);
        assert_eq!(registry.asked(), Asked::Notifications(NAME.into()));
        registry.register(NAME, Arc::new(Waited));
        assert_eq!(waiting.join().unwrap(), Ok(Some(7)));
        registry.finish();
    }
}
