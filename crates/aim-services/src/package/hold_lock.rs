//! PMS test lock capabilities, android-16.0.0_r1. AOSP Apache-2.0.
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service},
    parcel::{Binder, Exception, UNKNOWN_TRANSACTION},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub struct Owner {
    process: Arc<LocalProcess>,
    debuggable: bool,
    closed: AtomicBool,
    tokens: Mutex<BTreeMap<u64, i32>>,
}
impl Owner {
    pub fn new(process: Arc<LocalProcess>, debuggable: bool) -> Arc<Self> {
        Arc::new(Self {
            process,
            debuggable,
            closed: AtomicBool::new(false),
            tokens: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn issue(
        &self,
        query: &super::query::Query<'_>,
    ) -> Result<Result<Binder, Exception>, super::apps_filter::NotModelled> {
        self.available()?;
        if !self.debuggable {
            return Ok(Err(Exception::security(
                "getHoldLockToken requires a debuggable build",
            )));
        }
        let uid = query.calling_uid;
        let app = super::apps_filter::app_id(uid);
        if app != 0
            && app != 1000
            && !query.uid_has_permission(uid, "android.permission.INJECT_EVENTS")?
        {
            return Ok(Err(Exception::security(
                "getHoldLockToken requires INJECT_EVENTS permission",
            )));
        }
        let mut tokens = self.tokens.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "hold lock owner closed",
            )));
        }
        let token = self.process.add_service(Arc::new(Token {
            descriptor: format!("holdLock:{uid}"),
        }));
        let Binder::Local(ptr) = token else {
            unreachable!("native Binder allocation")
        };
        tokens.insert(ptr, uid);
        Ok(Ok(token))
    }
    /// System validates here before sleeping while holding its actual package
    /// publication lock. A separate surrogate mutex cannot stand in for mLock.
    pub fn verify(&self, uid: i32, token: Option<Binder>) -> Result<(), Exception> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "hold lock owner closed",
            ));
        }
        if !self.debuggable {
            return Err(Exception::security("holdLock requires a debuggable build"));
        }
        let Some(token) = token else {
            return Err(Exception::security("null holdLockToken"));
        };
        let Binder::Local(ptr) = token else {
            return Err(Exception::security("Invalid holdLock() token"));
        };
        if self.tokens.lock().unwrap().get(&ptr) != Some(&uid) {
            return Err(Exception::security("Invalid holdLock() token"));
        }
        Ok(())
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.tokens.lock().unwrap().clear();
    }
    fn available(&self) -> Result<(), super::apps_filter::NotModelled> {
        if self.closed.load(Ordering::Acquire) {
            Err(super::apps_filter::NotModelled("hold lock owner closed"))
        } else {
            Ok(())
        }
    }
}
struct Token {
    descriptor: String,
}
impl Service for Token {
    fn descriptor(&self) -> &str {
        &self.descriptor
    }
    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(UNKNOWN_TRANSACTION)
    }
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageHoldLock").finish_non_exhaustive()
    }
}
impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
