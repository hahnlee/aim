//! Actual original PackageList/PackageObserverHelper references stay in their
//! Java LocalServices owner; native publication sends only names and real UIDs.
use aim_binder_host::{
    local::Strong,
    parcel::{BAD_VALUE, Exception, Parcel},
};
use aim_service_aidl::dev_aim_server_ipackageobservereventsbridge as api;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub struct Owner {
    target: Strong,
    closed: AtomicBool,
    revoked: AtomicBool,
}
impl Owner {
    pub fn new(target: Strong) -> Arc<Self> {
        Arc::new(Self {
            target,
            closed: AtomicBool::new(false),
            revoked: AtomicBool::new(false),
        })
    }
    fn call(&self, code: u32, request: &Parcel) -> Result<(), Exception> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package observer epoch closed",
            ));
        }
        let reply = self
            .target
            .transact(code, request, false)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package observer transport {status}"),
                )
            })?;
        let mut reader = reply.reader();
        reader.read_exception().map_err(|status| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package observer reply {status}"),
            )
        })??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package observer reply {BAD_VALUE}"),
            ));
        }
        Ok(())
    }
    pub fn added(&self, name: &str, uid: i32) -> Result<(), Exception> {
        let mut p = Parcel::new();
        api::PackageAdded {
            package_name: Some(name.into()),
            uid,
        }
        .write(&mut p);
        self.call(api::PACKAGE_ADDED, &p)
    }
    pub fn changed(&self, name: &str, uid: i32) -> Result<(), Exception> {
        let mut p = Parcel::new();
        api::PackageChanged {
            package_name: Some(name.into()),
            uid,
        }
        .write(&mut p);
        self.call(api::PACKAGE_CHANGED, &p)
    }
    pub fn removed(&self, name: &str, uid: i32) -> Result<(), Exception> {
        let mut p = Parcel::new();
        api::PackageRemoved {
            package_name: Some(name.into()),
            uid,
        }
        .write(&mut p);
        self.call(api::PACKAGE_REMOVED, &p)
    }
    /// Local invalidation is safe under bootstrap publication locks.
    pub fn stop(&self) {
        self.closed.store(true, Ordering::Release);
    }
    /// Invoke outside package publication locks; callbacks can reenter native getters.
    pub fn close(&self) -> Result<(), Exception> {
        self.stop();
        if self.revoked.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut p = Parcel::new();
        api::Revoke {}.write(&mut p);
        let reply = self
            .target
            .transact(api::REVOKE, &p, false)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("observer revoke {status}"),
                )
            })?;
        let mut r = reply.reader();
        api::read_revoke_reply(&mut r).map_err(|status| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("observer revoke reply {status}"),
            )
        })??;
        if r.remaining() != 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "observer revoke reply tail",
            ));
        }
        Ok(())
    }
}
