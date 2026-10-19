//! `package`: PackageManager's queries for the app host, answered by the
//! native model (`package::query`, `package::resolve`) from the packages
//! the app host joined (`super::packages`), and its enabled settings
//! (`super::package_writes`). What the model does not answer fails with
//! the reason, logged once.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::AppHost;
use super::package_writes::Writes;
use crate::package::intent::ComponentName;
use crate::package::query::Query;
use crate::package::write::enabled::{Batch, Setting};
use crate::shadow::{Answer, ShadowCall};

pub struct PackageService {
    host: Arc<AppHost>,
    /// The installer `getPackageInstaller` hands out (`mInstallerService`).
    installer: aim_binder_host::parcel::Binder,
    reported: Mutex<HashSet<(u32, &'static str)>>,
    writes: Arc<Writes>,
}

impl PackageService {
    pub fn new(host: Arc<AppHost>) -> Self {
        let writes = Writes::new(host.clone());
        let installer = host.process.add_service(Arc::new(super::package_installer::PackageInstaller::new(host.clone())));
        PackageService { host, installer, reported: Mutex::new(HashSet::new()), writes }
    }

    fn unmodelled(&self, code: u32, reason: &'static str) -> Reply {
        if self.reported.lock().unwrap().insert((code, reason)) {
            eprintln!("app host: package#{code} not modelled: {reason}");
        }
        let mut reply = Parcel::new();
        reply.write_exception(&Exception::new(
            EX_UNSUPPORTED_OPERATION,
            format!("package#{code}: {reason}"),
        ));
        Ok(reply)
    }

    /// The reply of a write, or `None` for another call.
    fn write(&self, call: &mut Call<'_>, query: &Query<'_>) -> Option<Reply> {
        let reply = |result: Result<(), Exception>, write: fn(&mut Parcel)| {
            let mut p = Parcel::new();
            match result {
                Ok(()) => write(&mut p),
                Err(e) => p.write_exception(&e),
            }
            Ok(p)
        };
        let uid = call.sender_euid;
        Some(match call.code {
            pm::REGISTER_PACKAGE_MONITOR_CALLBACK => {
                let Ok(a) = pm::RegisterPackageMonitorCallback::read(&mut call.data) else { return Some(Err(BAD_VALUE)) };
                reply(self.writes.register_monitor(call, a.callback, a.user_id), pm::write_register_package_monitor_callback_reply)
            }
            pm::UNREGISTER_PACKAGE_MONITOR_CALLBACK => {
                let Ok(a) = pm::UnregisterPackageMonitorCallback::read(&mut call.data) else { return Some(Err(BAD_VALUE)) };
                self.writes.unregister_monitor(a.callback);
                reply(Ok(()), pm::write_unregister_package_monitor_callback_reply)
            }
            pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING => {
                let setting = match single(call.code, uid, &mut call.data) {
                    Ok(setting) => setting,
                    Err(status) => return Some(Err(status)),
                };
                let write = match call.code {
                    pm::SET_COMPONENT_ENABLED_SETTING => pm::write_set_component_enabled_setting_reply,
                    _ => pm::write_set_application_enabled_setting_reply,
                };
                reply(setting.and_then(|s| self.writes.set_enabled(call, &[s])), write)
            }
            pm::SET_COMPONENT_ENABLED_SETTINGS => {
                let batch = match Batch::read(uid as i32, &mut call.data) {
                    Ok(batch) => batch,
                    Err(status) => return Some(Err(status)),
                };
                // No settings: the user does not exist.
                let result = batch.settings(query).and_then(|settings| match settings.is_empty() {
                    true => Ok(()),
                    false => self.writes.set_enabled(call, &settings),
                });
                reply(result, pm::write_set_component_enabled_settings_reply)
            }
            // One-way; ART Service's `DexUseManagerLocal` records the use.
            pm::NOTIFY_DEX_LOAD => reply(
                Err(self.host.unimplemented_named("package", "notifyDexLoad: ART Service's dex use (#1425)")),
                |_| {},
            ),
            _ => return None,
        })
    }
}

/// The setting of `setComponentEnabledSetting` or
/// `setApplicationEnabledSetting`; a null component throws as
/// `ComponentEnabledSetting`'s constructor does, and no package is known
/// by a null name.
fn single(code: u32, uid: u32, data: &mut Reader<'_>) -> Result<Result<Setting, Exception>, i32> {
    let mut args = data.clone();
    if let Some(setting) = Setting::read(code, uid, data) {
        return Ok(Ok(setting));
    }
    Ok(Err(if code == pm::SET_COMPONENT_ENABLED_SETTING {
        pm::SetComponentEnabledSetting::<ComponentName>::read(&mut args)?;
        Exception::new(EX_NULL_POINTER, "componentName")
    } else {
        pm::SetApplicationEnabledSetting::read(&mut args)?;
        Exception::illegal_argument("Unknown package: null")
    }))
}

impl Service for PackageService {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == pm::GET_PACKAGE_INSTALLER {
            pm::GetPackageInstaller::read(&mut call.data).map_err(|_| BAD_VALUE)?;
            let mut reply = Parcel::new();
            pm::write_get_package_installer_reply(&mut reply, Some(self.installer));
            return Ok(reply);
        }
        let (state, resolution) = match self.host.packages() {
            Ok(packages) => packages,
            Err(e) => {
                let mut reply = Parcel::new();
                reply.write_exception(&e);
                return Ok(reply);
            }
        };
        let mut shadow = ShadowCall {
            service: "package",
            descriptor: pm::DESCRIPTOR,
            code: call.code,
            flags: call.flags,
            sender_pid: call.sender_pid,
            sender_euid: call.sender_euid,
            seq: 0,
            sent: Instant::now(),
            dropped: 0,
            data: call.data.clone(),
        };
        if let Some(answer) = self.host.resolver.answer(&state, &mut shadow) {
            return match answer {
                Answer::Reply(reply) => Ok(reply),
                Answer::Status(status) => Err(status),
                Answer::NotModelled => self.unmodelled(call.code, "intent query"),
            };
        }
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: call.sender_euid as i32,
        };
        if let Some(reply) = self.write(call, &query) {
            return reply;
        }
        // The call's data from its interface token, which the query reads.
        let mut data = call.data.clone();
        match query.answer(pm::DESCRIPTOR, call.code, &mut data) {
            Ok(reply) => Ok(reply),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                self.unmodelled(call.code, reason)
            }
        }
    }
}
