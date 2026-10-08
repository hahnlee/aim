//! Native pending rollback/install observer Handler ownership.
use super::native::NativeOwners;
use aim_binder_host::{
    local::{LocalProcess, LocalService, Strong},
    parcel::{EX_ILLEGAL_STATE, Exception, Parcel},
};
use aim_service_aidl::{WriteParcelable, android_content_pm_ipackageinstallobserver2 as observer};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};
struct Rollback {
    installer: Arc<NativeOwners>,
    session: i32,
}
pub struct Extras {
    pub bytes: Vec<u8>,
    pub objects: Vec<u64>,
}
impl WriteParcelable for Extras {
    fn write_to(&self, p: &mut Parcel) {
        p.write_raw(&self.bytes, &self.objects);
    }
}
enum Observer {
    Local(LocalService),
    Remote(Strong),
}
impl Observer {
    fn notify(&self, request: &Parcel) -> Result<(), Exception> {
        let reply = match self {
            Self::Local(owner) => owner.transact(observer::ON_PACKAGE_INSTALLED, request, true),
            Self::Remote(owner) => owner.transact(observer::ON_PACKAGE_INSTALLED, request, true),
        };
        reply
            .map(|_| ())
            .map_err(|status| fail(format!("Install observer no longer exists: {status}")))
    }
}
struct PendingObserver {
    observer: Observer,
    package: String,
    status: i32,
    message: Option<String>,
    extras: Option<Extras>,
}
enum Job {
    Rollback(i32, i32),
    Killed(String),
    Stop,
}
pub struct Owner {
    rollback: Mutex<BTreeMap<i32, Rollback>>,
    killed: Mutex<BTreeMap<String, PendingObserver>>,
    queue: mpsc::Sender<Job>,
    errors: Mutex<Vec<String>>,
    pub apex: Strong,
}
pub struct Worker {
    owner: Arc<Owner>,
    thread: Option<JoinHandle<()>>,
}
impl Owner {
    pub fn stop(&self) {
        let _ = self.queue.send(Job::Stop);
    }
    pub fn start(apex: Strong) -> (Arc<Self>, Worker) {
        let (queue, receiver) = mpsc::channel();
        let owner = Arc::new(Self {
            rollback: Mutex::new(BTreeMap::new()),
            killed: Mutex::new(BTreeMap::new()),
            queue,
            errors: Mutex::new(vec![]),
            apex,
        });
        let run = owner.clone();
        let thread = thread::spawn(move || {
            while let Ok(job) = receiver.recv() {
                let result = match job {
                    Job::Stop => break,
                    Job::Rollback(token, code) => {
                        let pending = run.rollback.lock().unwrap().remove(&token);
                        match pending {
                            Some(pending) => {
                                if code != 1 {
                                    run.errors.lock().unwrap().push(format!("Failed to enable rollback for install {}; continuing installation",pending.session));
                                }
                                pending
                                    .installer
                                    .complete_native_package_verification(pending.session, true)
                            }
                            None => {
                                run.errors.lock().unwrap().push(format!(
                                    "Invalid rollback enabled token {token} received"
                                ));
                                Ok(())
                            }
                        }
                    }
                    Job::Killed(package) => {
                        let pending = run.killed.lock().unwrap().remove(&package);
                        match pending {
                            Some(pending) => {
                                let mut request = Parcel::new();
                                observer::OnPackageInstalled {
                                    base_package_name: Some(pending.package),
                                    return_code: pending.status,
                                    msg: pending.message,
                                    extras: pending.extras,
                                }
                                .write(&mut request);
                                pending.observer.notify(&request)
                            }
                            None => Ok(()),
                        }
                    }
                };
                if let Err(error) = result {
                    run.errors.lock().unwrap().push(error.message);
                }
            }
        });
        (
            owner.clone(),
            Worker {
                owner,
                thread: Some(thread),
            },
        )
    }
    pub fn register_rollback(
        &self,
        token: i32,
        installer: Arc<NativeOwners>,
        session: i32,
    ) -> Result<(), Exception> {
        let mut pending = self.rollback.lock().unwrap();
        if pending.contains_key(&token) {
            return Err(fail("Rollback token already pending"));
        }
        pending.insert(token, Rollback { installer, session });
        Ok(())
    }
    pub fn rollback_code(&self, token: i32, code: i32) -> Result<(), Exception> {
        self.queue
            .send(Job::Rollback(token, code))
            .map_err(|_| fail("Rollback handler stopped"))
    }
    pub fn defer_kill_observer(
        &self,
        process: &LocalProcess,
        package: String,
        binder: aim_binder_host::parcel::Binder,
        status: i32,
        message: Option<String>,
        extras: Option<Extras>,
    ) -> Result<(), Exception> {
        let observer = match binder {
            aim_binder_host::parcel::Binder::Local(pointer) => Observer::Local(
                process
                    .local_service(pointer)
                    .ok_or_else(|| fail("Install observer local capability unavailable"))?,
            ),
            aim_binder_host::parcel::Binder::Handle(handle) => {
                Observer::Remote(process.strong(handle))
            }
        };
        self.killed.lock().unwrap().insert(
            package.clone(),
            PendingObserver {
                observer,
                package,
                status,
                message,
                extras,
            },
        );
        Ok(())
    }
    pub fn killed(&self, package: String) -> Result<(), Exception> {
        self.queue
            .send(Job::Killed(package))
            .map_err(|_| fail("Install observer handler stopped"))
    }
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.owner.queue.send(Job::Stop);
        if let Some(thread) = self.thread.take() {
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}
fn fail(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
