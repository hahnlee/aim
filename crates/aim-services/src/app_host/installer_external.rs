//! `IInstallerExternalBridge` for the app host: the native installer's
//! outside owners, answered as system_server's `InstallerExternalBridge`
//! answers them (`android-16.0.0_r1`), to root and the system uid: the
//! uids' importance changes it watches for install constraints, and the
//! guest's install environment. Its other owners are not ported (#2157).

use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::android_os_iremotecallback as remote_callback;
use aim_service_aidl::dev_aim_server_iinstallerexternalbridge as bridge;

use super::{AppHost, reply, system_boolean};
use crate::bundle::{self, Value};
use crate::package::query::Query;
use crate::system::ServiceOwner;

const ROOT_UID: u32 = 0;
const SYSTEM_UID: u32 = 1000;
/// `watchAppState`'s cutpoints: `IMPORTANCE_FOREGROUND` and
/// `IMPORTANCE_FOREGROUND_SERVICE`, a listener each.
const CUTPOINTS: usize = 2;

pub struct InstallerExternal {
    host: Arc<AppHost>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    closed: bool,
    /// The importance listeners' registrations.
    listeners: Vec<u64>,
}

/// `getNativeInstallEnvironment`'s record: the page size, whether the 16
/// KiB app compat is disabled, the headless system user mode, and the
/// default zone's id.
fn install_environment(
    page_size: i64,
    compat_16kb_disabled: bool,
    headless: bool,
    zone: &str,
) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i64(page_size);
    p.write_bool(compat_16kb_disabled);
    p.write_bool(headless);
    p.write_string16(Some(zone));
    p.data().to_vec()
}

/// The listener's event to `callback`: the uid's package
/// (`getNameForUid`, as the system).
fn send_package(host: &Weak<AppHost>, callback: &ServiceOwner, uid: i32) {
    let Some(host) = host.upgrade() else { return };
    let name = match host.packages() {
        Ok((state, resolution)) => Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID as i32,
        }
        .name_for_uid(uid)
        .map_err(|e| host.unimplemented_named("installer external", e.0)),
        Err(e) => Err(e),
    };
    let name = match name {
        Ok(name) => name,
        Err(e) => return eprintln!("app host: constraint state callback: {}", e.message),
    };
    let mut p = Parcel::new();
    p.write_interface_token(remote_callback::DESCRIPTOR);
    p.write_i32(1);
    bundle::write(&mut p, &[("package", Value::String(name))]);
    if let Err(status) = callback.transact(remote_callback::SEND_RESULT, &p, true) {
        eprintln!("app host: constraint state callback failed: {status}");
    }
}

impl InstallerExternal {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerExternal {
            host,
            state: Default::default(),
        }
    }

    fn open(&self) -> Result<std::sync::MutexGuard<'_, State>, Exception> {
        let state = self.state.lock().unwrap();
        if state.closed {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer external owner closed",
            ));
        }
        Ok(state)
    }

    /// `watchAppState`: `ActivityManager.addOnUidImportanceListener` at
    /// each cutpoint, each change sent on its own thread, as
    /// ActivityManager's handler sends it.
    fn watch_app_state(&self, callback: Option<Binder>) -> Result<(), Exception> {
        let mut state = self.open()?;
        let callback = match callback {
            Some(Binder::Local(ptr)) => self
                .host
                .process
                .local_service(ptr)
                .map(ServiceOwner::Local),
            Some(Binder::Handle(h)) => {
                Some(ServiceOwner::Remote(Arc::new(self.host.process.strong(h))))
            }
            None => None,
        }
        .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let callback = Arc::new(callback);
        for _ in 0..CUTPOINTS {
            let host = Arc::downgrade(&self.host);
            let callback = callback.clone();
            let id = self
                .host
                .add_uid_importance_listener(Arc::new(move |uid, _change| {
                    let (host, callback) = (host.clone(), callback.clone());
                    std::thread::spawn(move || send_package(&host, &callback, uid));
                }));
            state.listeners.push(id);
        }
        Ok(())
    }

    /// `close`: the listeners removed, once.
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        for id in state.listeners.drain(..) {
            self.host.remove_uid_importance_listener(id);
        }
    }

    /// `getNativeInstallEnvironment`: `TimeZone.getDefault()` is
    /// `persist.sys.timezone`, GMT where unset or not in the image's
    /// tzdata.
    fn install_environment(&self) -> Result<Vec<u8>, Exception> {
        self.open()?;
        let root = self.host.root()?;
        let tzdata = std::fs::read(root.join(crate::tzdata::PATH.trim_start_matches('/')))
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("tzdata: {e}")))?;
        let zone = self
            .host
            .property("persist.sys.timezone")
            .map(|z| z.trim().to_string())
            .filter(|z| crate::tzdata::has(&tzdata, z))
            .unwrap_or_else(|| "GMT".into());
        // `UserManager.isHeadlessSystemUserMode`: the system user is not
        // full.
        let headless = self.host.user_flags()? & super::user_service::FLAG_FULL == 0;
        Ok(install_environment(
            super::package_initial_context::PAGE_SIZE,
            system_boolean(
                self.host.property("pm.16kb.app_compat.disabled").as_deref(),
                false,
            ),
            headless,
            &zone,
        ))
    }
}

impl Service for InstallerExternal {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID && call.sender_euid != ROOT_UID {
            return reply(Err(Exception::security(
                "untrusted installer bridge caller",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::WATCH_APP_STATE => {
                let a = bridge::WatchAppState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.watch_app_state(a.callback)
            }
            bridge::CLOSE => {
                bridge::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.close();
                bridge::write_close_reply(&mut p);
                Ok(())
            }
            bridge::GET_NATIVE_INSTALL_ENVIRONMENT => {
                bridge::GetNativeInstallEnvironment::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.install_environment().map(|record| {
                    bridge::write_get_native_install_environment_reply(&mut p, &Some(record))
                })
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("installer external", &format!("{name} (#2157)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record reads as the native installer reads it.
    #[test]
    fn the_install_environment_is_the_originals_record() {
        let record = install_environment(16_384, false, true, "Asia/Seoul");
        let mut r = aim_binder_host::parcel::Reader::new(&record, &[]);
        assert_eq!(r.read_i64().unwrap(), 16_384);
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("Asia/Seoul"));
        assert_eq!(r.remaining(), 0);
    }

    /// The uids' changes reach the importance listeners until they are
    /// removed; a closed owner refuses to watch.
    #[test]
    fn importance_listeners_hear_the_uids_changes() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: None,
            },
        );
        let host = AppHost::new(process);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        let id = host.add_uid_importance_listener(Arc::new(move |uid, change| {
            into.lock().unwrap().push((
                uid,
                matches!(change, super::super::uid_observers::Change::Gone),
            ));
        }));
        host.uid_changed(10_100, super::super::uid_observers::Change::Active);
        host.uid_changed(10_100, super::super::uid_observers::Change::Gone);
        host.remove_uid_importance_listener(id);
        host.uid_changed(10_101, super::super::uid_observers::Change::Active);
        assert_eq!(*heard.lock().unwrap(), [(10_100, false), (10_100, true)]);
        let external = InstallerExternal::new(host);
        external.close();
        let e = external.watch_app_state(None).unwrap_err();
        assert_eq!(e.message, "installer external owner closed");
    }
}
