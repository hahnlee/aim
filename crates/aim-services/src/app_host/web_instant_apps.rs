//! `IWebInstantAppsState` for the app host: the users' web instant app
//! policy the native package boot keeps, read from the settings and user
//! owners as `WebInstantAppsState` reads them for PackageManagerService's
//! `mWebInstantAppsDisabled` (`android-16.0.0_r1`): disabled for a user
//! where `enable_ephemeral_feature` or the user's `instant_apps_enabled`
//! is 0, published to the session's callback on each change of either.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_iwebinstantappschanged as changed;
use aim_service_aidl::dev_aim_server_iwebinstantappsstate as leaf;

use super::content_service::LocalObserver;
use super::settings_provider::Caller;
use super::{AppHost, reply};
use crate::bundle::Value;
use crate::package::uri::Uri;
use crate::system::ServiceOwner;

const SYSTEM_UID: u32 = 1000;
/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;
const ENABLE_EPHEMERAL_FEATURE: &str = "enable_ephemeral_feature";
const INSTANT_APPS_ENABLED: &str = "instant_apps_enabled";

pub struct WebInstantApps {
    host: Arc<AppHost>,
    me: Weak<WebInstantApps>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    callback: Option<Arc<ServiceOwner>>,
    epoch: i64,
    ready: bool,
    /// The settings observers' registrations.
    observers: Vec<u64>,
    failure: Option<String>,
}

/// The record: its version, the epoch and each user's disabled state.
fn record(epoch: i64, users: &[(i32, bool)]) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(1);
    p.write_i64(epoch);
    p.write_i32(users.len() as i32);
    for &(user, disabled) in users {
        p.write_i32(user);
        p.write_bool(disabled);
    }
    p.data().to_vec()
}

/// `Settings.getInt`'s value: the default where unset or not a number.
fn int_or(value: Option<String>, default: i32) -> i32 {
    value.and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn state_error(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

struct Observer(Weak<WebInstantApps>);

impl LocalObserver for Observer {
    fn on_change(&self, _self_change: bool, _uris: &[Uri], _flags: i32, _user: i32) {
        let Some(state) = self.0.upgrade() else {
            return;
        };
        // On the background handler, as the observer's `onChange` runs.
        std::thread::spawn(move || state.publish_changed());
    }
}

impl WebInstantApps {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        Arc::new_cyclic(|me| WebInstantApps {
            host,
            me: me.clone(),
            state: Default::default(),
        })
    }

    /// `Settings.Global.getString` or `Settings.Secure.getStringForUser`,
    /// as the system.
    fn setting(&self, method: &str, name: &str, user: i32) -> Result<Option<String>, Exception> {
        let caller = Caller {
            uid: SYSTEM_UID as i32,
            package: Some("android".into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(user))]);
        let got =
            self.host
                .settings_provider()?
                .call(&caller, method, Some(name), &extras, None)?;
        Ok(match got.as_deref() {
            Some([(_, Value::String(value)), ..]) => value.clone(),
            _ => None,
        })
    }

    /// The record of `state`: empty with epoch 0 before `start` (the
    /// original's empty `mWebInstantAppsDisabled`), else a new epoch of the
    /// users' states.
    fn read(&self, state: &mut State) -> Result<Vec<u8>, Exception> {
        if let Some(failure) = &state.failure {
            return Err(state_error(failure.clone()));
        }
        if !state.ready {
            return Ok(record(0, &[]));
        }
        let ids = super::user_service::user_ids(&self.host, false)?;
        if ids.is_empty() {
            return Err(state_error("Web instant policy UM inventory absent"));
        }
        let global_disabled =
            int_or(self.setting("GET_global", ENABLE_EPHEMERAL_FEATURE, 0)?, 1) == 0;
        if state.epoch == i64::MAX {
            return Err(state_error("Web instant policy epoch exhausted"));
        }
        let users = ids
            .into_iter()
            .map(|user| {
                if user < 0 {
                    return Err(state_error("Invalid original UM user id"));
                }
                let disabled = global_disabled
                    || int_or(self.setting("GET_secure", INSTANT_APPS_ENABLED, user)?, 1) == 0;
                Ok((user, disabled))
            })
            .collect::<Result<Vec<_>, Exception>>()?;
        state.epoch += 1;
        Ok(record(state.epoch, &users))
    }

    /// The current record to the callback, outside the lock; a failure is
    /// kept, and every later read fails with it.
    fn publish_changed(&self) {
        let (target, record) = {
            let mut state = self.state.lock().unwrap();
            let Some(target) = state.callback.clone() else {
                return;
            };
            match self.read(&mut state) {
                Ok(record) => (target, record),
                Err(e) => {
                    state.failure = Some(format!(
                        "Web instant state publication failed: {}",
                        e.message
                    ));
                    return;
                }
            }
        };
        let mut p = Parcel::new();
        changed::Changed {
            record: Some(record),
        }
        .write(&mut p);
        let result = target
            .transact(changed::CHANGED, &p, false)
            .map_err(|s| state_error(format!("transport: {s}")))
            .and_then(|r| {
                changed::read_changed_reply(&mut r.reader())
                    .map_err(|s| state_error(format!("reply: {s}")))?
            });
        if let Err(e) = result {
            self.state.lock().unwrap().failure = Some(format!(
                "Web instant state publication failed: {}",
                e.message
            ));
        }
    }

    /// `start`: both settings observed for all users, then the first
    /// record published.
    fn start(&self, callback: Option<Binder>) -> Result<(), Exception> {
        let target = match callback {
            Some(Binder::Local(ptr)) => self
                .host
                .process
                .local_service(ptr)
                .map(ServiceOwner::Local),
            Some(Binder::Handle(h)) => {
                Some(ServiceOwner::Remote(Arc::new(self.host.process.strong(h))))
            }
            _ => None,
        }
        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "Web instant policy callback"))?;
        {
            let mut state = self.state.lock().unwrap();
            if state.callback.is_some() {
                return Err(state_error("Web instant policy observer already installed"));
            }
            let content = self
                .host
                .content_service
                .get()
                .cloned()
                .ok_or_else(|| state_error("no content service"))?;
            let observer: Arc<dyn LocalObserver> = Arc::new(Observer(self.me.clone()));
            let global = content.register_local(
                &Uri::parse(&format!(
                    "content://settings/global/{ENABLE_EPHEMERAL_FEATURE}"
                )),
                false,
                USER_ALL,
                observer.clone(),
            )?;
            match content.register_local(
                &Uri::parse(&format!("content://settings/secure/{INSTANT_APPS_ENABLED}")),
                false,
                USER_ALL,
                observer,
            ) {
                Ok(secure) => state.observers = vec![global, secure],
                Err(e) => {
                    content.unregister_local(global);
                    return Err(e);
                }
            }
            state.callback = Some(Arc::new(target));
            state.ready = true;
        }
        self.publish_changed();
        match &self.state.lock().unwrap().failure {
            Some(failure) => Err(state_error(failure.clone())),
            None => Ok(()),
        }
    }

    /// `stop`: the observers gone, no more records published.
    fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(content) = self.host.content_service.get() {
            for id in state.observers.drain(..) {
                content.unregister_local(id);
            }
        }
        state.callback = None;
    }
}

impl Service for WebInstantApps {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Web instant state requires system uid",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::CAPTURE => {
                leaf::Capture::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut state = self.state.lock().unwrap();
                self.read(&mut state)
                    .map(|record| leaf::write_capture_reply(&mut p, &Some(record)))
            }
            leaf::START => {
                let a = leaf::Start::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.start(a.callback)
                    .map(|()| leaf::write_start_reply(&mut p))
            }
            leaf::STOP => {
                leaf::Stop::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.stop();
                leaf::write_stop_reply(&mut p);
                Ok(())
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::web_instant_state::Snapshot;

    #[test]
    fn records_read_as_the_session_reads_them() {
        let before = Snapshot::read(&record(0, &[])).unwrap();
        assert_eq!(before.epoch, 0);
        assert!(before.disabled.is_empty());
        let after = Snapshot::read(&record(3, &[(0, false), (10, true)])).unwrap();
        assert_eq!(after.epoch, 3);
        assert!(!after.is_disabled(0));
        assert!(after.is_disabled(10));
    }

    #[test]
    fn settings_read_as_integers_with_their_default() {
        assert_eq!(int_or(None, 1), 1);
        assert_eq!(int_or(Some("0".into()), 1), 0);
        assert_eq!(int_or(Some("x".into()), 1), 1);
    }
}
