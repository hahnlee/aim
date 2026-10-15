//! Original PackageInstaller callbacks: FIFO handler delivery, caller visibility,
//! user cookies and RemoteCallbackList lifetime, android-16.0.0_r1 (AOSP, Apache-2.0).
use super::Event;
use aim_binder_host::{
    local::{LocalProcess, LocalService, Strong},
    parcel::{Binder, DEAD_OBJECT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel},
};
use aim_service_aidl::android_content_pm_ipackageinstallercallback as aidl;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
pub type Visibility = Arc<dyn Fn(u32, i32) -> Result<bool, Exception> + Send + Sync>;
pub type VisibilitySource = Arc<dyn Fn() -> Result<Visibility, Exception> + Send + Sync>;
enum Target {
    Remote(Arc<Strong>),
    Local(Arc<LocalService>),
}
struct Entry {
    process: std::sync::Weak<LocalProcess>,
    target: Target,
    cookie: Mutex<Option<u64>>,
    token: u64,
    uid: u32,
    user: i32,
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let (Target::Remote(strong), Some(cookie)) =
            (&self.target, self.cookie.lock().unwrap().take())
        {
            if let Some(process) = self.process.upgrade() {
                process.clear_death(strong, cookie);
            }
        }
    }
}
struct Core {
    process: std::sync::Weak<LocalProcess>,
    entries: Mutex<HashMap<Binder, Arc<Entry>>>,
    visibility: VisibilitySource,
    next: std::sync::atomic::AtomicU64,
    errors: Mutex<Vec<String>>,
}
impl Core {
    fn remove(&self, binder: Binder, token: u64) {
        let removed = {
            let mut entries = self.entries.lock().unwrap();
            if entries
                .get(&binder)
                .is_some_and(|entry| entry.token == token)
            {
                entries.remove(&binder)
            } else {
                None
            }
        };
        drop(removed);
    }
    fn deliver(&self, event: Event) {
        let visibility = match (self.visibility)() {
            Ok(visibility) => visibility,
            Err(error) => {
                self.errors
                    .lock()
                    .unwrap()
                    .push(format!("callback visibility owner: {}", error.message));
                return;
            }
        };
        let entries: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(binder, entry)| (*binder, entry.clone()))
            .collect();
        let (id, user) = event.identity();
        let (code, parcel) = request(&event);
        for (binder, entry) in entries {
            if entry.user != user as i32 {
                continue;
            }
            match visibility(entry.uid, id) {
                Ok(false) => continue,
                Err(error) => {
                    self.errors
                        .lock()
                        .unwrap()
                        .push(format!("callback visibility: {}", error.message));
                    continue;
                }
                Ok(true) => {}
            }
            let result = match &entry.target {
                Target::Remote(strong) => strong.transact(code, &parcel, true),
                Target::Local(local) => local.transact(code, &parcel, true),
            };
            if let Err(status) = result {
                if status == DEAD_OBJECT {
                    self.remove(binder, entry.token);
                } else {
                    self.errors
                        .lock()
                        .unwrap()
                        .push(format!("installer callback transport: {status}"));
                }
            }
        }
    }
}
enum Job {
    Event(Event),
    Barrier(mpsc::SyncSender<()>),
}
/// Keep outside owner locks. Dropping it stops and joins the dispatcher.
pub struct CallbackWorker {
    thread: Option<JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    queue: Arc<Mutex<Option<mpsc::Sender<Job>>>>,
}
impl CallbackWorker {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for CallbackWorker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.queue.lock().unwrap().take();
        if let Some(worker) = self.thread.take() {
            if worker.thread().id() != thread::current().id() {
                worker.join().unwrap();
            }
        }
    }
}
pub struct Registry {
    core: Arc<Core>,
    queue: Arc<Mutex<Option<mpsc::Sender<Job>>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Registry {
    pub fn new(
        process: Arc<LocalProcess>,
        visibility: VisibilitySource,
    ) -> Result<Self, Exception> {
        let core = Arc::new(Core {
            process: Arc::downgrade(&process),
            entries: Mutex::new(HashMap::new()),
            visibility,
            next: std::sync::atomic::AtomicU64::new(1),
            errors: Mutex::new(Vec::new()),
        });
        let (tx, rx) = mpsc::channel();
        let worker_core = core.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("package-installer-callbacks".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    if worker_stop.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    match job {
                        Job::Event(event) => worker_core.deliver(event),
                        Job::Barrier(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
        Ok(Self {
            core,
            queue: Arc::new(Mutex::new(Some(tx))),
            stop,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn register(&self, callback: Option<Binder>, uid: u32, user: i32) -> Result<(), Exception> {
        let binder = callback
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "null IPackageInstallerCallback"))?;
        let process = self.core.process.upgrade().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "installer callback Binder process unavailable",
            )
        })?;
        let target = match binder {
            Binder::Handle(handle) => Target::Remote(Arc::new(process.strong(handle))),
            Binder::Local(pointer) => {
                Target::Local(Arc::new(process.local_service(pointer).ok_or_else(
                    || Exception::new(EX_ILLEGAL_STATE, "local callback node unavailable"),
                )?))
            }
        };
        let token = self
            .core
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let entry = Arc::new(Entry {
            process: Arc::downgrade(&process),
            target,
            cookie: Mutex::new(None),
            token,
            uid,
            user,
        });
        let old = self
            .core
            .entries
            .lock()
            .unwrap()
            .insert(binder, entry.clone());
        drop(old);
        if let Target::Remote(strong) = &entry.target {
            let weak = Arc::downgrade(&self.core);
            let cookie = process.link_to_death(
                strong,
                Box::new(move || {
                    if let Some(core) = weak.upgrade() {
                        core.remove(binder, token)
                    }
                }),
            );
            *entry.cookie.lock().unwrap() = Some(cookie);
        }
        Ok(())
    }
    pub fn unregister(&self, callback: Option<Binder>) -> Result<(), Exception> {
        let callback = callback
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "null IPackageInstallerCallback"))?;
        let removed = self.core.entries.lock().unwrap().remove(&callback);
        drop(removed);
        Ok(())
    }
    pub fn notify(&self, event: Event) -> Result<(), Exception> {
        self.queue
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer callback queue closed"))?
            .send(Job::Event(event))
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))
    }
    pub fn flush(&self, timeout: Duration) -> Result<(), Exception> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.queue
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "installer callback queue closed"))?
            .send(Job::Barrier(tx))
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
        rx.recv_timeout(timeout)
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))
    }
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.core.errors.lock().unwrap())
    }
    /// Nonjoining: safe when releasing a bootstrap owner under its mutex.
    pub fn shutdown(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.queue.lock().unwrap().take();
        let entries = std::mem::take(&mut *self.core.entries.lock().unwrap());
        drop(entries);
    }
    pub fn take_worker(&self) -> Option<CallbackWorker> {
        self.worker
            .lock()
            .unwrap()
            .take()
            .map(|thread| CallbackWorker {
                thread: Some(thread),
                stop: self.stop.clone(),
                queue: self.queue.clone(),
            })
    }
    pub fn len(&self) -> usize {
        self.core.entries.lock().unwrap().len()
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.shutdown();
        self.worker.lock().unwrap().take();
    }
}

fn request(event: &Event) -> (u32, Parcel) {
    let mut parcel = Parcel::new();
    let code = match *event {
        Event::Created { id, .. } => {
            aidl::OnSessionCreated { session_id: id }.write(&mut parcel);
            aidl::ON_SESSION_CREATED
        }
        Event::Badging { id, .. } => {
            aidl::OnSessionBadgingChanged { session_id: id }.write(&mut parcel);
            aidl::ON_SESSION_BADGING_CHANGED
        }
        Event::Active { id, active, .. } => {
            aidl::OnSessionActiveChanged {
                session_id: id,
                active,
            }
            .write(&mut parcel);
            aidl::ON_SESSION_ACTIVE_CHANGED
        }
        Event::Progress { id, progress, .. } => {
            aidl::OnSessionProgressChanged {
                session_id: id,
                progress,
            }
            .write(&mut parcel);
            aidl::ON_SESSION_PROGRESS_CHANGED
        }
        Event::Finished { id, success, .. } => {
            aidl::OnSessionFinished {
                session_id: id,
                success,
            }
            .write(&mut parcel);
            aidl::ON_SESSION_FINISHED
        }
    };
    (code, parcel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{
        Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*,
    };
    use aim_binder_host::{
        local::{Call, Reply, Service},
        parcel::UNKNOWN_TRANSACTION,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
            Err(errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
            Err(errno::EFAULT)
        }
        fn get_file(&mut self, _: u32) -> Result<File, Errno> {
            Err(errno::EBADF)
        }
        fn install_file(&mut self, _: File) -> Result<u32, Errno> {
            Err(errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            panic!("unexpected fd")
        }
    }
    struct Listener {
        events: Arc<Mutex<Vec<Event>>>,
    }
    impl Service for Listener {
        fn descriptor(&self) -> &str {
            aidl::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert!(call.is_oneway());
            assert_eq!(call.sender_euid, 1000);
            let event = match call.code {
                aidl::ON_SESSION_CREATED => {
                    let a = aidl::OnSessionCreated::read(&mut call.data)?;
                    Event::Created {
                        id: a.session_id,
                        user: 0,
                    }
                }
                aidl::ON_SESSION_BADGING_CHANGED => {
                    let a = aidl::OnSessionBadgingChanged::read(&mut call.data)?;
                    Event::Badging {
                        id: a.session_id,
                        user: 0,
                    }
                }
                aidl::ON_SESSION_ACTIVE_CHANGED => {
                    let a = aidl::OnSessionActiveChanged::read(&mut call.data)?;
                    Event::Active {
                        id: a.session_id,
                        user: 0,
                        active: a.active,
                    }
                }
                aidl::ON_SESSION_PROGRESS_CHANGED => {
                    let a = aidl::OnSessionProgressChanged::read(&mut call.data)?;
                    Event::Progress {
                        id: a.session_id,
                        user: 0,
                        progress: a.progress,
                    }
                }
                aidl::ON_SESSION_FINISHED => {
                    let a = aidl::OnSessionFinished::read(&mut call.data)?;
                    Event::Finished {
                        id: a.session_id,
                        user: 0,
                        success: a.success,
                    }
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            };
            assert_eq!(call.data.remaining(), 0);
            self.events.lock().unwrap().push(event);
            Ok(Parcel::new())
        }
    }
    fn open(driver: &Arc<Driver>, pid: i32, uid: u32) -> Arc<LocalProcess> {
        LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid,
                euid: uid,
                security_context: None,
            },
        )
    }
    struct Processes {
        driver: Arc<Driver>,
        processes: Vec<Arc<LocalProcess>>,
    }
    impl Drop for Processes {
        fn drop(&mut self) {
            for process in &self.processes {
                self.driver.release(process.proc_handle());
            }
        }
    }
    #[test]
    fn callbacks_queue_generated_oneway_preserve_user_visibility_and_replacement_cookie() {
        let driver = Driver::new();
        let process = open(&driver, 88001, 1000);
        let _processes = Processes {
            driver,
            processes: vec![process.clone()],
        };
        let allowed = Arc::new(AtomicBool::new(true));
        let current = allowed.clone();
        let registry = Registry::new(
            process.clone(),
            Arc::new(move || {
                let enabled = current.load(Ordering::SeqCst);
                Ok(Arc::new(move |uid: u32, _: i32| Ok(uid == 10100 && enabled)) as Visibility)
            }),
        )
        .unwrap();
        let _worker = registry.take_worker().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = process.add_service(Arc::new(Listener {
            events: events.clone(),
        }));
        registry.register(Some(callback), 10100, 0).unwrap();
        let sequence = vec![
            Event::Created { id: 7, user: 0 },
            Event::Badging { id: 7, user: 0 },
            Event::Active {
                id: 7,
                user: 0,
                active: true,
            },
            Event::Progress {
                id: 7,
                user: 0,
                progress: 0.4,
            },
            Event::Finished {
                id: 7,
                user: 0,
                success: false,
            },
        ];
        for event in &sequence {
            registry.notify(event.clone()).unwrap();
        }
        registry.flush(Duration::from_secs(2)).unwrap();
        assert_eq!(*events.lock().unwrap(), sequence);
        registry.notify(Event::Created { id: 8, user: 10 }).unwrap();
        allowed.store(false, Ordering::SeqCst);
        registry.notify(Event::Created { id: 9, user: 0 }).unwrap();
        registry.flush(Duration::from_secs(2)).unwrap();
        assert_eq!(events.lock().unwrap().len(), 5);
        allowed.store(true, Ordering::SeqCst);
        registry.register(Some(callback), 10101, 0).unwrap();
        assert_eq!(registry.len(), 1);
        registry.notify(Event::Created { id: 10, user: 0 }).unwrap();
        registry.flush(Duration::from_secs(2)).unwrap();
        assert_eq!(events.lock().unwrap().len(), 5);
        registry.unregister(Some(callback)).unwrap();
        registry.notify(Event::Created { id: 11, user: 0 }).unwrap();
        registry.flush(Duration::from_secs(2)).unwrap();
        assert_eq!(registry.len(), 0);
        assert_eq!(events.lock().unwrap().len(), 5);
        assert!(registry.take_errors().is_empty());
    }
    struct Registrar {
        registry: std::sync::Weak<Registry>,
    }
    impl Service for Registrar {
        fn descriptor(&self) -> &str {
            "fixture.CallbackRegistrar"
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let binder = call.data.read_binder()?;
            assert_eq!(call.data.remaining(), 0);
            self.registry
                .upgrade()
                .unwrap()
                .register(binder, call.sender_euid, 0)
                .unwrap();
            let mut reply = Parcel::new();
            reply.write_no_exception();
            Ok(reply)
        }
    }
    #[test]
    fn remote_callback_process_death_releases_strong_registration() {
        let driver = Driver::new();
        let native = open(&driver, 88101, 1000);
        let client = open(&driver, 88102, 10100);
        let _processes = Processes {
            driver: driver.clone(),
            processes: vec![native.clone(), client.clone()],
        };
        let registry = Arc::new(
            Registry::new(
                native.clone(),
                Arc::new(|| Ok(Arc::new(|_, _| Ok(true)) as Visibility)),
            )
            .unwrap(),
        );
        let _worker = registry.take_worker().unwrap();
        let Binder::Local(pointer) = native.add_service(Arc::new(Registrar {
            registry: Arc::downgrade(&registry),
        })) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: pointer,
            cookie: pointer,
        }
        .encode();
        driver
            .ioctl(
                native.proc_handle(),
                88103,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        native.start();
        client.start();
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = client.add_service(Arc::new(Listener {
            events: events.clone(),
        }));
        let mut parcel = Parcel::new();
        parcel.write_binder(Some(callback));
        client
            .strong(0)
            .transact(1, &parcel, false)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(registry.len(), 1);
        registry.notify(Event::Created { id: 77, user: 0 }).unwrap();
        registry.flush(Duration::from_secs(2)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while events.lock().unwrap().is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        driver.release(client.proc_handle());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while registry.len() != 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "dead registration remains"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        registry.notify(Event::Created { id: 78, user: 0 }).unwrap();
        registry.flush(Duration::from_secs(2)).unwrap();
        assert_eq!(events.lock().unwrap().len(), 1);
    }
    #[test]
    fn dropping_untaken_worker_under_owner_lock_signals_without_joining() {
        let driver = Driver::new();
        let process = open(&driver, 88201, 1000);
        let _processes = Processes {
            driver,
            processes: vec![process.clone()],
        };
        let lock = Arc::new(Mutex::new(()));
        let held = lock.lock().unwrap();
        let captured = lock.clone();
        let (started, ready) = mpsc::sync_channel(1);
        let (done, ended) = mpsc::sync_channel(1);
        struct End(mpsc::SyncSender<()>);
        impl Drop for End {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let end = End(done);
        let registry = Registry::new(
            process,
            Arc::new(move || {
                let _ = &end;
                started.send(()).unwrap();
                let _held = captured.lock().unwrap();
                Ok(Arc::new(|_, _| Ok(true)) as Visibility)
            }),
        )
        .unwrap();
        registry.notify(Event::Created { id: 7, user: 0 }).unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(registry);
        drop(held);
        ended.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    #[test]
    fn last_owner_drop_on_dispatcher_thread_does_not_join_itself() {
        let driver = Driver::new();
        let process = open(&driver, 88301, 1000);
        let _processes = Processes {
            driver,
            processes: vec![process.clone()],
        };
        let weak_owner = Arc::new(Mutex::new(std::sync::Weak::<Owner>::new()));
        let weak = weak_owner.clone();
        let (started, ready) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let released = Mutex::new(released);
        let (done, ended) = mpsc::sync_channel(1);
        struct Owner {
            registry: Registry,
            done: mpsc::SyncSender<std::thread::ThreadId>,
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                self.done.send(std::thread::current().id()).unwrap();
            }
        }
        let registry = Registry::new(
            process,
            Arc::new(move || {
                let owner = weak.lock().unwrap().upgrade().unwrap();
                started.send(()).unwrap();
                released.lock().unwrap().recv().unwrap();
                drop(owner);
                Ok(Arc::new(|_, _| Ok(true)) as Visibility)
            }),
        )
        .unwrap();
        let owner = Arc::new(Owner { registry, done });
        *weak_owner.lock().unwrap() = Arc::downgrade(&owner);
        let worker = owner.registry.take_worker().unwrap();
        owner
            .registry
            .notify(Event::Created { id: 7, user: 0 })
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(owner);
        release.send(()).unwrap();
        let thread = ended.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_ne!(thread, std::thread::current().id());
        drop(worker);
    }
}
