//! `package`: PackageManager's queries for the app host, answered by the
//! native model (`package::query`, `package::resolve`) from the packages
//! the app host joined (`super::packages`), and its enabled settings
//! (`super::package_writes`). What the model does not answer fails with
//! the reason, logged once.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::AppHost;
use super::package_writes::Writes;
use super::settings_service::{java_exception, parse_user_arg};
use crate::package::apps_filter::NotModelled;
use crate::package::intent::ComponentName;
use crate::package::query::Query;
use crate::package::write::enabled::{Batch, Setting};
use crate::shadow::{Answer, ShadowCall};
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};

const PER_USER_RANGE: i32 = 100_000;
/// `Process.ROOT_UID`, `SYSTEM_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const MANAGE_DEVICE_ADMINS: &str = "android.permission.MANAGE_DEVICE_ADMINS";
const SUSPEND_APPS: &str = "android.permission.SUSPEND_APPS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";

pub struct PackageService {
    host: Arc<AppHost>,
    /// The installer `getPackageInstaller` hands out (`mInstallerService`).
    installer: aim_binder_host::parcel::Binder,
    reported: Mutex<HashSet<(u32, &'static str)>>,
    writes: Arc<Writes>,
}

impl PackageService {
    pub fn new(host: Arc<AppHost>) -> Self {
        let writes = host
            .package_writes
            .get_or_init(|| Writes::new(host.clone()))
            .clone();
        let installer =
            host.process
                .add_service(Arc::new(super::package_installer::PackageInstaller::new(
                    host.clone(),
                )));
        PackageService {
            host,
            installer,
            reported: Mutex::new(HashSet::new()),
            writes,
        }
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
                let Ok(a) = pm::RegisterPackageMonitorCallback::read(&mut call.data) else {
                    return Some(Err(BAD_VALUE));
                };
                reply(
                    self.writes.register_monitor(call, a.callback, a.user_id),
                    pm::write_register_package_monitor_callback_reply,
                )
            }
            pm::UNREGISTER_PACKAGE_MONITOR_CALLBACK => {
                let Ok(a) = pm::UnregisterPackageMonitorCallback::read(&mut call.data) else {
                    return Some(Err(BAD_VALUE));
                };
                self.writes.unregister_monitor(a.callback);
                reply(Ok(()), pm::write_unregister_package_monitor_callback_reply)
            }
            pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING => {
                let setting = match single(call.code, uid, &mut call.data) {
                    Ok(setting) => setting,
                    Err(status) => return Some(Err(status)),
                };
                let write = match call.code {
                    pm::SET_COMPONENT_ENABLED_SETTING => {
                        pm::write_set_component_enabled_setting_reply
                    }
                    _ => pm::write_set_application_enabled_setting_reply,
                };
                reply(
                    setting.and_then(|s| self.writes.set_enabled(call, &[s])),
                    write,
                )
            }
            pm::SET_COMPONENT_ENABLED_SETTINGS => {
                let batch = match Batch::read(uid as i32, &mut call.data) {
                    Ok(batch) => batch,
                    Err(status) => return Some(Err(status)),
                };
                // No settings: the user does not exist.
                let result = batch
                    .settings(query)
                    .and_then(|settings| match settings.is_empty() {
                        true => Ok(()),
                        false => self.writes.set_enabled(call, &settings),
                    });
                reply(result, pm::write_set_component_enabled_settings_reply)
            }
            pm::CLEAR_APPLICATION_USER_DATA => {
                let Ok(a) = pm::ClearApplicationUserData::read(&mut call.data) else {
                    return Some(Err(BAD_VALUE));
                };
                let package = a.package_name.as_deref();
                let done = super::clear_data::observer_of(&self.host, package, a.observer);
                reply(
                    self.host
                        .clear_package_data(uid as i32, package, a.user_id, done),
                    pm::write_clear_application_user_data_reply,
                )
            }
            pm::IS_PACKAGE_STATE_PROTECTED => {
                let Ok(a) = pm::IsPackageStateProtected::read(&mut call.data) else {
                    return Some(Err(BAD_VALUE));
                };
                let mut p = Parcel::new();
                match self.package_state_protected(query, a.package_name.as_deref(), a.user_id) {
                    Ok(Ok(protected)) => {
                        pm::write_is_package_state_protected_reply(&mut p, protected)
                    }
                    Ok(Err(e)) => p.write_exception(&e),
                    Err(NotModelled(reason)) => return Some(self.unmodelled(call.code, reason)),
                }
                Ok(p)
            }
            pm::GET_UNSUSPENDABLE_PACKAGES_FOR_USER => {
                let Ok(a) = pm::GetUnsuspendablePackagesForUser::read(&mut call.data) else {
                    return Some(Err(BAD_VALUE));
                };
                reply(
                    self.unsuspendable_checks(uid as i32, a.package_names.is_some(), a.user_id)
                        .and_then(|()| {
                            Err(self.host.unimplemented_named(
                                "package",
                                "getUnsuspendablePackagesForUser: package suspension (#1987)",
                            ))
                        }),
                    |_| {},
                )
            }
            // One-way; ART Service's `DexUseManagerLocal` records the use.
            pm::NOTIFY_DEX_LOAD => reply(
                Err(self.host.unimplemented_named(
                    "package",
                    "notifyDexLoad: ART Service's dex use (#1425)",
                )),
                |_| {},
            ),
            _ => return None,
        })
    }
}

impl PackageService {
    /// `isPackageStateProtected`'s checks (the caller across users, the
    /// shell's restrictions, MANAGE_DEVICE_ADMINS for others than root and
    /// the system), then `ProtectedPackages.isPackageStateProtected` of
    /// the app host.
    fn package_state_protected(
        &self,
        q: &Query<'_>,
        name: Option<&str>,
        user: i32,
    ) -> Result<Result<bool, Exception>, NotModelled> {
        let uid = q.calling_uid;
        if let Err(e) =
            q.internal_enforce_cross_user(uid, user, false, true, "isPackageStateProtected")?
        {
            return Ok(Err(e));
        }
        if !matches!(uid % PER_USER_RANGE, ROOT_UID | SYSTEM_UID)
            && !q.uid_has_permission(uid, MANAGE_DEVICE_ADMINS)?
        {
            return Ok(Err(Exception::security(format!(
                "Caller must have the {MANAGE_DEVICE_ADMINS} permission."
            ))));
        }
        let Some(name) = name else {
            return Ok(Ok(false));
        };
        Ok(self.writes.protected(name).and_then(|protected| {
            protected.ok_or_else(|| {
                self.host.unimplemented_named(
                    "package",
                    "isPackageStateProtected of a device or profile owner's packages (#1521)",
                )
            })
        }))
    }

    /// `getUnsuspendablePackagesForUser`'s checks: the names, SUSPEND_APPS,
    /// and INTERACT_ACROSS_USERS_FULL for another user.
    fn unsuspendable_checks(&self, uid: i32, names: bool, user: i32) -> Result<(), Exception> {
        if !names {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "packageNames cannot be null",
            ));
        }
        let enforce = |permission: &str, message: &str| -> Result<(), Exception> {
            match self.host.check_permission(Some(permission), uid)? {
                0 => Ok(()),
                _ => Err(Exception::security(format!(
                    "{message}: Neither user {uid} nor current process has {permission}."
                ))),
            }
        };
        enforce(SUSPEND_APPS, "getUnsuspendablePackagesForUser")?;
        if uid / PER_USER_RANGE != user {
            enforce(
                INTERACT_ACROSS_USERS_FULL,
                &format!(
                    "Calling uid {uid} cannot query getUnsuspendablePackagesForUser for user {user}"
                ),
            )?;
        }
        Ok(())
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

impl PackageService {
    /// `PackageManagerShellCommand.onCommand` of `uid` (of `pid`): `clear`;
    /// the installation commands run in `transact`; the other commands are
    /// not implemented (#1939).
    fn shell_command(&self, uid: i32, pid: i32, c: &mut ShellCommand) -> i32 {
        let cmd = c.command().unwrap_or_default().to_string();
        let result = match cmd.as_str() {
            "clear" => self.run_clear(uid, pid, c),
            _ => Err(java_exception(&self.host.unimplemented_named(
                "package",
                &format!("the shell command {cmd:?} (#1939)"),
            ))),
        };
        result.unwrap_or_else(|exception| {
            c.exception(&exception);
            -1
        })
    }

    /// `runClear`: the package's data cleared through ActivityManager, as
    /// the shell, and waited for.
    fn run_clear(&self, uid: i32, pid: i32, c: &mut ShellCommand) -> Result<i32, String> {
        const USER_SYSTEM: i32 = 0;
        let mut user = USER_SYSTEM;
        let mut cache_only = false;
        while let Some(opt) = c.next_option()? {
            match opt.as_str() {
                "--user" => user = parse_user_arg(&c.next_arg_required()?)?,
                "--cache-only" => cache_only = true,
                _ => {
                    c.println(&format!("Error: Unknown option: {opt}"));
                    return Ok(1);
                }
            }
        }
        let Some(package) = c.next_arg() else {
            c.eprintln("Error: no package specified");
            return Ok(1);
        };
        let java = |e: Exception| java_exception(&e);
        // `translateUserId(user, USER_NULL, "runClear")`.
        let user = self.host.incoming_user(uid, user, false).map_err(java)?;
        if cache_only {
            return Err(java(self.host.unimplemented_named(
                "package",
                "the shell command \"clear --cache-only\" (#1939)",
            )));
        }
        let (sender, outcome) = std::sync::mpsc::channel();
        let done: super::clear_data::Done = Box::new(move |succeeded| {
            let _ = sender.send(succeeded);
        });
        self.host
            .clear_application_user_data(uid, pid, Some(&package), false, done, user)
            .map_err(java)?;
        if outcome.recv().unwrap_or(false) {
            c.println("Success");
            Ok(0)
        } else {
            c.eprintln("Failed");
            Ok(1)
        }
    }
}

impl Service for PackageService {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == SHELL_COMMAND_TRANSACTION {
            let mut command =
                ShellCommand::read(&self.host.process, &mut call.data).map_err(|_| BAD_VALUE)?;
            if crate::package::shell_install::claims(command.command().unwrap_or_default()) {
                // The native installation commands, over this package
                // manager and the app host's installer.
                let context = crate::package::shell::Context::with_command(
                    Arc::new(super::package_shell::ShellHost(Arc::downgrade(&self.host))),
                    self.host.process.clone(),
                    self,
                    call.sender_euid,
                    call.sender_pid,
                    command,
                );
                crate::package::shell::dispatch_by(context, crate::package::shell_install::run);
            } else if command.has_output() {
                let result =
                    self.shell_command(call.sender_euid as i32, call.sender_pid, &mut command);
                command.finish(result);
            }
            let mut p = Parcel::new();
            p.write_no_exception();
            return Ok(p);
        }
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
        if call.code == pm::REQUEST_PACKAGE_CHECKSUMS {
            let request = crate::package::query::checksums::Request::read(&mut call.data)?;
            let mut reply = Parcel::new();
            match request.prepare(&query) {
                Err(crate::package::apps_filter::NotModelled(reason)) => {
                    return self.unmodelled(call.code, reason);
                }
                Ok(Err(e)) => reply.write_exception(&e),
                Ok(Ok(prepared)) => match super::package_checksums::request(&self.host, prepared) {
                    Ok(()) => pm::write_request_package_checksums_reply(&mut reply),
                    Err(super::package_checksums::Refused::NotModelled(reason)) => {
                        return self.unmodelled(call.code, reason);
                    }
                    Err(super::package_checksums::Refused::Exception(e)) => {
                        reply.write_exception(&e)
                    }
                },
            }
            return Ok(reply);
        }
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
