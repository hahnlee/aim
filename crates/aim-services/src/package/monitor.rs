//! PackageMonitorCallbackHelper registration and delivery, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::handler_queue::Queue;
use aim_binder_host::{
    local::{LocalProcess, LocalService, Strong},
    parcel::{Binder, DEAD_OBJECT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel},
};
use aim_service_aidl::{WriteParcelable, android_os_iremotecallback as aidl};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

/// Retained ActivityManager.handleIncomingUser with allowAll=true, requireFull=true.
pub type ResolveUser = Arc<dyn Fn(i32, i32, i32) -> Result<i32, Exception> + Send + Sync>;
/// Original Bundle payload containing EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT's
/// Intent, after per-registration UID extras filtering. None means filterExtras
/// rejected this registration. Binder/fd objects require a separate retained
/// capability owner and are explicitly rejected here.
pub type BundleSource = Arc<dyn Fn(u32) -> Result<Option<Parcel>, Exception> + Send + Sync>;

pub struct Event {
    pub action: String,
    pub user: i32,
    pub allow_uids: Option<Vec<u32>>,
    pub bundle: BundleSource,
}
enum Target {
    Remote(Arc<Strong>),
    Local(Arc<LocalService>),
}
struct Entry {
    process: Weak<LocalProcess>,
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
    process: Weak<LocalProcess>,
    entries: Mutex<HashMap<Binder, Arc<Entry>>>,
    next: AtomicU64,
    errors: Mutex<Vec<String>>,
    stopped: AtomicBool,
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
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let entries: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(binder, entry)| (*binder, entry.clone()))
            .collect();
        for (binder, entry) in entries {
            if entry.user != -1 && entry.user != event.user {
                continue;
            }
            if event
                .allow_uids
                .as_ref()
                .is_some_and(|uids| entry.uid % 100_000 != 1000 && !uids.contains(&entry.uid))
            {
                continue;
            }
            let bundle = match (event.bundle)(entry.uid) {
                Ok(Some(bundle)) => bundle,
                Ok(None) => continue,
                Err(error) => {
                    self.errors.lock().unwrap().push(error.message);
                    continue;
                }
            };
            if !bundle.objects().is_empty() {
                self.errors
                    .lock()
                    .unwrap()
                    .push("package monitor Bundle contains unowned Binder or fd objects".into());
                continue;
            }
            let mut request = Parcel::new();
            aidl::SendResult {
                data: Some(Bundle(&bundle)),
            }
            .write(&mut request);
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            let result = match &entry.target {
                Target::Remote(strong) => strong.transact(aidl::SEND_RESULT, &request, true),
                Target::Local(local) => local.transact(aidl::SEND_RESULT, &request, true),
            };
            if let Err(status) = result {
                if status == DEAD_OBJECT {
                    self.remove(binder, entry.token);
                } else {
                    self.errors
                        .lock()
                        .unwrap()
                        .push(format!("package monitor callback transport: {status}"));
                }
            }
        }
    }
}
struct Bundle<'a>(&'a Parcel);
impl WriteParcelable for Bundle<'_> {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_raw(self.0.data(), &[]);
    }
}

pub struct Owner {
    core: Arc<Core>,
    handler: Arc<Queue>,
    resolve_user: ResolveUser,
}
impl Owner {
    pub fn new(process: Arc<LocalProcess>, handler: Arc<Queue>, resolve_user: ResolveUser) -> Self {
        Self {
            core: Arc::new(Core {
                process: Arc::downgrade(&process),
                entries: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                errors: Mutex::new(Vec::new()),
                stopped: AtomicBool::new(false),
            }),
            handler,
            resolve_user,
        }
    }
    pub fn register(
        &self,
        callback: Option<Binder>,
        pid: i32,
        uid: i32,
        user: i32,
    ) -> Result<(), Exception> {
        // Original resolves the incoming user before RemoteCallbackList registration.
        let user = (self.resolve_user)(pid, uid, user)?;
        if self.core.stopped.load(Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package monitor owner stopped",
            ));
        }
        let binder =
            callback.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null IRemoteCallback"))?;
        let process = self.core.process.upgrade().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "package monitor Binder process unavailable",
            )
        })?;
        let target = match binder {
            Binder::Handle(handle) => Target::Remote(Arc::new(process.strong(handle))),
            Binder::Local(pointer) => Target::Local(Arc::new(
                process.local_service(pointer).ok_or_else(|| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        "package monitor local callback unavailable",
                    )
                })?,
            )),
        };
        let token = self.core.next.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Entry {
            process: Arc::downgrade(&process),
            target,
            cookie: Mutex::new(None),
            token,
            uid: uid as u32,
            user,
        });
        let old = {
            let mut entries = self.core.entries.lock().unwrap();
            if self.core.stopped.load(Ordering::Acquire) {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "package monitor owner stopped",
                ));
            }
            entries.insert(binder, entry.clone())
        };
        drop(old);
        if let Target::Remote(strong) = &entry.target {
            let weak = Arc::downgrade(&self.core);
            let cookie = process.link_to_death(
                strong,
                Box::new(move || {
                    if let Some(core) = weak.upgrade() {
                        core.remove(binder, token);
                    }
                }),
            );
            *entry.cookie.lock().unwrap() = Some(cookie);
        }
        Ok(())
    }
    pub fn unregister(&self, callback: Option<Binder>) -> Result<(), Exception> {
        let binder =
            callback.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null IRemoteCallback"))?;
        let removed = self.core.entries.lock().unwrap().remove(&binder);
        drop(removed);
        Ok(())
    }
    pub fn user_removed(&self, user: i32) {
        let removed: Vec<_> = {
            let mut entries = self.core.entries.lock().unwrap();
            let binders: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.user == user)
                .map(|(binder, _)| *binder)
                .collect();
            binders
                .into_iter()
                .filter_map(|binder| entries.remove(&binder))
                .collect()
        };
        drop(removed);
    }
    pub fn notify(&self, event: Event) -> Result<(), Exception> {
        if !allowed_action(&event.action) {
            return Ok(());
        }
        if self.core.stopped.load(Ordering::Acquire) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package monitor owner stopped",
            ));
        }
        let core = self.core.clone();
        self.handler.post(move || core.deliver(event))
    }
    pub fn shutdown(&self) {
        self.core.stopped.store(true, Ordering::Release);
        let entries = std::mem::take(&mut *self.core.entries.lock().unwrap());
        drop(entries);
    }
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.core.errors.lock().unwrap())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn allowed_action(action: &str) -> bool {
    matches!(
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
    )
}
