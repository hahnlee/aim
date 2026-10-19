//! PackageInstallerService for the app host (`IPackageInstaller` at
//! `android-16.0.0_r1`, `getPackageInstaller`), as far as apps read it: no
//! session exists, as none was persisted (`install_sessions.xml` holds
//! none, else the app host fails) and none can be made, so the session
//! lists are empty and a session's info is none; callbacks are kept
//! until their process dies, with no session event to tell. Making and
//! driving sessions is not implemented (#1757).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_content_pm_ipackageinstaller as pi;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::list_slice::Slice;

const SESSION_INFO: &str = "android.content.pm.PackageInstaller$SessionInfo";

/// No session, of which a list is written.
struct NoSession;

impl aim_service_aidl::WriteParcelable for NoSession {
    fn write_to(&self, _: &mut Parcel) {}
}

pub struct PackageInstaller {
    host: Arc<AppHost>,
    /// `mCallbacks`: each callback, its death link and the user it hears.
    callbacks: Arc<Mutex<HashMap<u32, (Strong, u64, i32)>>>,
}

impl PackageInstaller {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageInstaller { host, callbacks: Default::default() }
    }

    /// `enforceCrossUserPermission(uid, user, true, false, message)`.
    fn enforce_cross_user(&self, uid: i32, user: i32, message: &str) -> Result<(), Exception> {
        if user < 0 {
            return Err(Exception::illegal_argument(format!("Invalid userId {user}")));
        }
        if user != uid / PER_USER_RANGE as i32 && !matches!(uid % PER_USER_RANGE as i32, 0 | 1000) && self.host.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? != 0 {
            return Err(Exception::security(format!("{message} requires android.permission.INTERACT_ACROSS_USERS_FULL")));
        }
        Ok(())
    }

    /// The persisted sessions, of which there must be none.
    fn no_sessions(&self) -> Result<(), Exception> {
        let path = self.host.data()?.join("system/install_sessions.xml");
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
        let Some(bytes) = super::device_policy::read_atomic_bytes(&path).map_err(state)? else { return Ok(()) };
        let root = aim_android_xml::read_next_optional(&bytes).map_err(state)?;
        if root.is_some_and(|r| r.children().next().is_some()) {
            return Err(self.host.unimplemented_named("package_installer", "persisted install sessions (#1757)"));
        }
        Ok(())
    }

    fn empty(&self) -> Slice<NoSession> {
        Slice::new(self.host.process.clone(), SESSION_INFO, Vec::new())
    }

    fn register(&self, uid: i32, callback: Option<Binder>, user: i32) -> Result<(), Exception> {
        self.enforce_cross_user(uid, user, "registerCallback")?;
        let Some(Binder::Handle(handle)) = callback else {
            return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "callback"));
        };
        let strong = self.host.process.strong(handle);
        let callbacks = Arc::downgrade(&self.callbacks);
        let process = self.host.process.clone();
        let death = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(callbacks) = callbacks.upgrade()
                    && let Some((strong, death, _)) = callbacks.lock().unwrap().remove(&handle)
                {
                    process.clear_death(&strong, death);
                }
            }),
        );
        if let Some((old, old_death, _)) = self.callbacks.lock().unwrap().insert(handle, (strong, death, user)) {
            self.host.process.clear_death(&old, old_death);
        }
        Ok(())
    }
}

impl Service for PackageInstaller {
    fn descriptor(&self) -> &str {
        pi::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            pi::GET_ALL_SESSIONS => {
                let a = pi::GetAllSessions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_cross_user(uid, a.user_id, "getAllSessions")
                    .and_then(|()| self.no_sessions())
                    .map(|()| pi::write_get_all_sessions_reply(&mut p, Some(&self.empty())))
            }
            pi::GET_MY_SESSIONS => {
                let a = pi::GetMySessions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_cross_user(uid, a.user_id, "getMySessions")?;
                    // `AppOpsManager.checkPackage`.
                    let package = a.installer_package_name.as_deref();
                    if !package.is_some_and(|p| self.host.owns_package(uid, p).unwrap_or(false)) {
                        return Err(Exception::security(format!("Package {} does not belong to {uid}", package.unwrap_or("null"))));
                    }
                    self.no_sessions()?;
                    pi::write_get_my_sessions_reply(&mut p, Some(&self.empty()));
                    Ok(())
                })()
            }
            pi::GET_STAGED_SESSIONS => {
                pi::GetStagedSessions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.no_sessions().map(|()| pi::write_get_staged_sessions_reply(&mut p, Some(&self.empty())))
            }
            pi::GET_SESSION_INFO => {
                pi::GetSessionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.no_sessions().map(|()| pi::write_get_session_info_reply::<NoSession>(&mut p, None))
            }
            pi::REGISTER_CALLBACK => {
                let a = pi::RegisterCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register(uid, a.callback, a.user_id).map(|()| pi::write_register_callback_reply(&mut p))
            }
            pi::UNREGISTER_CALLBACK => {
                let a = pi::UnregisterCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(h)) = a.callback
                    && let Some((strong, death, _)) = self.callbacks.lock().unwrap().remove(&h)
                {
                    self.host.process.clear_death(&strong, death);
                }
                pi::write_unregister_callback_reply(&mut p);
                Ok(())
            }
            code => {
                let name = pi::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("package_installer", &format!("{name} (#1757)")))
            }
        };
        reply(result.map(|()| p))
    }
}
