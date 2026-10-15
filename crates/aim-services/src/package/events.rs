//! Native package handler and monitor owners. android-16.0.0_r1 PMS and
//! PackageMonitorCallbackHelper, AOSP Apache-2.0.
use aim_binder_host::{
    local::{LocalProcess, LocalService, Strong},
    parcel::{Binder, Exception, Parcel},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};
pub type Task = Box<dyn FnOnce() + Send + 'static>;
pub type ResultPayload = Arc<dyn Fn(i32) -> Result<Option<Parcel>, Exception> + Send + Sync>;
enum Job {
    Task(Task),
    Barrier(mpsc::SyncSender<()>),
    Stop,
}
#[derive(Clone)]
enum Callback { Remote(Arc<Strong>), Local(Arc<LocalService>) }
impl Callback {
    fn clear_death(&self,process:&LocalProcess,cookie:u64){if let Self::Remote(node)=self{process.clear_death(node,cookie);}}
    fn send(&self,request:&Parcel)->Result<(),i32>{match self{
        Self::Remote(node)=>node.transact(aim_service_aidl::android_os_iremotecallback::SEND_RESULT,request,true).map(|_|()),
        Self::Local(node)=>node.transact(aim_service_aidl::android_os_iremotecallback::SEND_RESULT,request,true).map(|_|()),
    }}
}
#[derive(Clone,Copy,PartialEq,Eq,PartialOrd,Ord)]
enum CallbackKey{Local(u64),Remote(u32)}
impl From<Binder> for CallbackKey{fn from(binder:Binder)->Self{match binder{Binder::Local(ptr)=>Self::Local(ptr),Binder::Handle(handle)=>Self::Remote(handle)}}}
struct Listener {
    node: Callback,
    user: i32,
    uid: i32,
    death: u64,
    generation: u64,
}
pub struct Owner {
    process: Arc<LocalProcess>,
    closed: AtomicBool,
    main: Sender<Job>,
    background: Sender<Job>,
    listeners: Mutex<BTreeMap<CallbackKey, Listener>>,
    next_listener: AtomicU64,
}
pub struct Workers {
    owner: Weak<Owner>,
    threads: Vec<JoinHandle<()>>,
}
impl Owner {
    pub fn start(process: Arc<LocalProcess>) -> Result<(Arc<Self>, Workers), Exception> {
        let (main, mrx) = mpsc::channel();
        let (background, brx) = mpsc::channel();
        let owner = Arc::new(Self {
            process,
            closed: AtomicBool::new(false),
            main,
            background,
            listeners: Mutex::new(BTreeMap::new()),
            next_listener: AtomicU64::new(1),
        });
        let mut threads = Vec::new();
        for (name, rx) in [("package-handler", mrx), ("package-background", brx)] {
            let weak = Arc::downgrade(&owner);
            match thread::Builder::new().name(name.into()).spawn(move || {
                while let Ok(job) = rx.recv() {
                    let Some(owner) = weak.upgrade() else {
                        break;
                    };
                    if owner.closed.load(Ordering::Acquire) {
                        break;
                    }
                    match job {
                        Job::Task(task) => task(),
                        Job::Barrier(done) => {
                            let _ = done.send(());
                        }
                        Job::Stop => break,
                    }
                }
            }) {
                Ok(worker) => threads.push(worker),
                Err(error) => {
                    owner.close();
                    for worker in threads {
                        let _ = worker.join();
                    }
                    return Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("package handler start: {error}"),
                    ));
                }
            }
        }
        Ok((
            owner.clone(),
            Workers {
                owner: Arc::downgrade(&owner),
                threads,
            },
        ))
    }
    fn ensure_open(&self) -> Result<(), Exception> {
        if self.closed.load(Ordering::Acquire) {
            Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package events owner closed",
            ))
        } else {
            Ok(())
        }
    }
    pub fn post(&self, background: bool, task: Task) -> Result<(), Exception> {
        self.ensure_open()?;
        self.sender(background).send(Job::Task(task)).map_err(|_| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package handler stopped",
            )
        })
    }
    fn sender(&self, background: bool) -> &Sender<Job> {
        if background {
            &self.background
        } else {
            &self.main
        }
    }
    pub fn wait(&self, timeout: i64, background: bool) -> Result<bool, Exception> {
        self.ensure_open()?;
        let (done, rx) = mpsc::sync_channel(1);
        self.sender(background)
            .send(Job::Barrier(done))
            .map_err(|_| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package handler stopped",
                )
            })?;
        // CountDownLatch's already-finished fast path precedes timeout checking.
        if rx.try_recv().is_ok() {
            return Ok(true);
        }
        if timeout <= 0 {
            return Ok(false);
        }
        let end = SystemTime::now()
            .checked_add(Duration::from_millis(timeout as u64))
            .ok_or_else(|| Exception::illegal_argument("handler timeout overflow"))?;
        let remaining = end.duration_since(SystemTime::now()).unwrap_or_default();
        match rx.recv_timeout(remaining) {
            Ok(()) => Ok(true),
            Err(RecvTimeoutError::Timeout) => Ok(false),
            Err(RecvTimeoutError::Disconnected) => Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package handler stopped",
            )),
        }
    }
    /// target_user comes from actual ActivityManager.handleIncomingUser;
    /// caller UID comes from the authenticated IPM Binder transaction.
    pub fn register(
        self: &Arc<Self>,
        callback: Option<Binder>,
        target_user: i32,
        uid: i32,
    ) -> Result<(), Exception> {
        self.ensure_open()?;
        let callback = callback.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "package monitor callback is null",
            )
        })?;
        let node=match callback{
            Binder::Handle(handle)=>Callback::Remote(Arc::new(self.process.strong(handle))),
            Binder::Local(pointer)=>Callback::Local(Arc::new(self.process.local_service(pointer).ok_or_else(||Exception::illegal_argument("package monitor local capability unavailable"))?)),
        };
        let callback=CallbackKey::from(callback);
        let generation = self.next_listener.fetch_add(1, Ordering::Relaxed);
        let weak = Arc::downgrade(self);
        let mut listeners = self.listeners.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            drop(listeners);
            return self.ensure_open();
        }
        let death = if let Callback::Remote(remote)=&node{self.process.link_to_death(
            remote,
            Box::new(move || {
                if let Some(owner) = weak.upgrade() {
                    let mut listeners = owner.listeners.lock().unwrap();
                    if listeners
                        .get(&callback)
                        .is_some_and(|listener| listener.generation == generation)
                    {
                        listeners.remove(&callback);
                    }
                }
            }),
        )}else{0};
        let old = listeners.insert(
            callback,
            Listener {
                node,
                user: target_user,
                uid,
                death,
                generation,
            },
        );
        drop(listeners);
        if let Some(old) = old {
            old.node.clear_death(&self.process, old.death);
        }
        Ok(())
    }
    pub fn unregister(&self, callback: Option<Binder>) -> Result<(), Exception> {
        self.ensure_open()?;
        let callback = callback.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "package monitor callback is null",
            )
        })?;
        let old = self.listeners.lock().unwrap().remove(&CallbackKey::from(callback));
        if let Some(old) = old {
            old.node.clear_death(&self.process, old.death);
        }
        Ok(())
    }
    pub fn remove_user(&self, user: i32) {
        let removed = {
            let mut listeners = self.listeners.lock().unwrap();
            let keys = listeners
                .iter()
                .filter_map(|(handle, listener)| (listener.user == user).then_some(*handle))
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| listeners.remove(&key))
                .collect::<Vec<_>>()
        };
        for listener in removed {
            listener.node.clear_death(&self.process, listener.death);
        }
    }
    /// Payload creates the original Bundle containing the Intent and applies
    /// native visibility/extras filtering for this listener UID. None means
    /// the listener cannot observe the event, just as filterExtrasFunction.
    pub fn notify(
        self: &Arc<Self>,
        action: &str,
        user: i32,
        allow_uids: Option<Vec<i32>>,
        payload: ResultPayload,
    ) -> Result<(), Exception> {
        self.ensure_open()?;
        if !matches!(
            action,
            "android.intent.action.PACKAGE_ADDED"
                | "android.intent.action.PACKAGE_REMOVED"
                | "android.intent.action.PACKAGE_CHANGED"
                | "android.intent.action.UID_REMOVED"
                | "android.intent.action.PACKAGES_SUSPENDED"
                | "android.intent.action.PACKAGES_UNSUSPENDED"
                | "android.intent.action.EXTERNAL_APPLICATIONS_AVAILABLE"
                | "android.intent.action.EXTERNAL_APPLICATIONS_UNAVAILABLE"
                | "android.intent.action.PACKAGE_DATA_CLEARED"
                | "android.intent.action.PACKAGE_RESTARTED"
                | "android.intent.action.PACKAGE_UNSTOPPED"
        ) {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        self.post(
            false,
            Box::new(move || {
                let Some(owner) = weak.upgrade() else {
                    return;
                };
                if owner.ensure_open().is_err() {
                    return;
                }
                let listeners = owner
                    .listeners
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|listener| {
                        (listener.user == -1 || listener.user == user)
                            && allow_uids.as_ref().is_none_or(|uids| {
                                super::apps_filter::app_id(listener.uid) == 1000
                                    || uids.contains(&listener.uid)
                            })
                    })
                    .map(|listener| (listener.node.clone(), listener.uid))
                    .collect::<Vec<_>>();
                for (node, uid) in listeners {
                    match payload(uid) {
                        Ok(Some(bundle)) => {
                            let mut request = Parcel::new();
                            request.write_interface_token("android.os.IRemoteCallback");
                            request.write_i32(1);
                            request.write_raw(bundle.data(), bundle.objects());
                            if let Err(status) = node.send(&request) {
                                eprintln!("package monitor callback transport: {status}");
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            eprintln!("package monitor payload/filter: {}", error.message)
                        }
                    }
                }
            }),
        )
    }
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let listeners = std::mem::take(&mut *self.listeners.lock().unwrap());
        for (_, listener) in listeners {
            listener.node.clear_death(&self.process, listener.death);
        }
        let _ = self.main.send(Job::Stop);
        let _ = self.background.send(Job::Stop);
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.close();
        }
        for worker in self.threads.drain(..) {
            if worker.join().is_err() {
                eprintln!("package handler worker panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Driver,Device,Credentials};
    use aim_binder_host::local::{Service,Call,Reply};
    struct Callback(Arc<Mutex<Vec<i32>>>);
    impl Service for Callback {
        fn descriptor(&self)->&str{aim_service_aidl::android_os_iremotecallback::DESCRIPTOR}
        fn transact(&self,call:&mut Call<'_>)->Reply{
            assert_eq!(call.code,aim_service_aidl::android_os_iremotecallback::SEND_RESULT);assert!(call.is_oneway());
            call.data.enforce_interface(self.descriptor())?;assert_eq!(call.data.read_i32()?,1);
            self.0.lock().unwrap().push(call.data.read_i32()?);assert_eq!(call.data.remaining(),0);Ok(Parcel::new())
        }
    }
    #[test]
    fn local_monitor_capability_replaces_delivery_cookie_and_retires_on_unregister(){
        let driver=Driver::new();let process=LocalProcess::open(&driver,Device::Binder,Credentials{pid:98811,euid:1000,security_context:None});
        let deliveries=Arc::new(Mutex::new(Vec::new()));let binder=process.add_service(Arc::new(Callback(deliveries.clone())));
        let(owner,workers)=Owner::start(process.clone()).unwrap();
        owner.register(Some(binder),0,1000).unwrap();owner.register(Some(binder),10,1000).unwrap();
        assert_eq!(owner.listeners.lock().unwrap().len(),1);
        let payload:ResultPayload=Arc::new(|uid|{assert_eq!(uid,1000);let mut p=Parcel::new();p.write_i32(71);Ok(Some(p))});
        owner.notify("android.intent.action.PACKAGE_ADDED",0,None,payload.clone()).unwrap();
        owner.notify("android.intent.action.PACKAGE_ADDED",10,None,payload.clone()).unwrap();
        assert!(owner.wait(2000,false).unwrap());assert_eq!(*deliveries.lock().unwrap(),[71]);
        owner.unregister(Some(binder)).unwrap();owner.notify("android.intent.action.PACKAGE_ADDED",10,None,payload).unwrap();
        assert!(owner.wait(2000,false).unwrap());assert_eq!(*deliveries.lock().unwrap(),[71]);
        owner.close();drop(workers);driver.release(process.proc_handle());
    }
}
