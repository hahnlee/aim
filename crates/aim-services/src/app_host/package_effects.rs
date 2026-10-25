//! `IPackageMutationBridge` for the app host: the native package writer's
//! effects, answered as system_server's `NativePackageEffectsBridge`
//! answers them (`android-16.0.0_r1`), to the system uid only. The
//! packages' protection is `ProtectedPackages`' as the app host keeps it
//! (`data_protected`). Install effects retain the session capture and use
//! the activity, broadcast and package-monitor owners.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::broadcasts::TempAllowlist;
use super::kept::RawIntent;
use crate::bundle::Value;
use crate::package::apps_filter::{uid, user_id};
use crate::package::info::user_state;
use crate::package::query::Query;
use crate::package::uri::Uri;
use aim_service_aidl::WriteParcelable;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagemutationbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct PackageEffects {
    host: Arc<AppHost>,
}

impl PackageEffects {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageEffects { host }
    }

    /// `ProtectedPackages.isPackageStateProtected` and
    /// `isPackageDataProtected`, which are the same: no package is
    /// protected without a name.
    fn protected(&self, package: Option<&str>) -> Result<bool, Exception> {
        package.map_or(Ok(false), |package| self.host.data_protected(package))
    }
    fn writes(&self) -> &Arc<super::package_writes::Writes> {
        self.host
            .package_writes
            .get_or_init(|| super::package_writes::Writes::new(self.host.clone()))
    }

    fn require_installer_permission(&self, calling_uid: i32) -> Result<bool, Exception> {
        // EventLog.writeEvent is the only omitted effect: no EventLog writer (#2188).
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let names = query.packages_for_uid(calling_uid).map_err(unavailable)?;
        let compat = &self.host.platform()?.compat;
        let mut required = true;
        for name in names.into_iter().flatten().flatten() {
            let info = query
                .application_info(&name, 0, user_id(calling_uid))
                .map_err(unavailable)??;
            required &= info.as_ref().map_or_else(
                || compat.default_value(150857253),
                |info| compat.is_enabled(150857253, Some(&name), info.target_sdk_version),
            );
        }
        Ok(required)
    }

    fn capture_visibility(
        &self,
        a: bridge::CapturePostInstallVisibility,
    ) -> Result<Vec<i32>, Exception> {
        let capture = self.host.package_capture()?;
        let name = required(a.name.as_deref(), "name")?;
        let previous = capture
            .state()
            .packages
            .get(name)
            .ok_or_else(|| state_error("prior visibility package absent"))?;
        if previous.app_id != a.app_id
            || previous.version_code != a.version
            || Some(previous.path.as_str()) != a.code_path.as_deref()
        {
            return Err(state_error("prior visibility code/UID changed"));
        }
        let resolution = capture
            .resolution()
            .map_err(|e| state_error(format!("package resolution: {e:?}")))?;
        let metadata = capture.scan().metadata_revision();
        let mut encoded = vec![0x56495332, (metadata >> 32) as i32, metadata as i32];
        for user in required_array(a.users, "users")? {
            let list =
                resolution
                    .apps_filter
                    .visibility_allow_list(capture.state(), previous, user);
            encoded.extend([user, list.as_ref().map_or(-1, |v| v.len() as i32)]);
            if let Some(list) = list {
                encoded.extend(list);
            }
        }
        Ok(encoded)
    }

    fn safety_labels_enabled(
        &self,
        state: &crate::package::model::State,
    ) -> Result<bool, Exception> {
        let enabled = self
            .host
            .inputs()?
            .settings
            .get(
                super::settings::Table::Config,
                "privacy/safety_label_change_notifications_enabled",
            )
            .is_none_or(|v| v.eq_ignore_ascii_case("true"));
        Ok(enabled
            && ![
                "android.hardware.type.automotive",
                "android.software.leanback",
                "android.hardware.type.watch",
            ]
            .iter()
            .any(|name| {
                state
                    .system
                    .features
                    .iter()
                    .any(|feature| feature.0 == *name)
            }))
    }

    /// One handler job retains the capture's lists and delivers each untargeted
    /// event once to callbacks. Native discovery observes the final package once
    /// per user, independent of targeted copies and replacement broadcasts.
    fn post(&self, events: Vec<Event>, discoveries: Vec<(String, i32)>) -> Result<(), Exception> {
        let host = self.host.clone();
        let writes = self.writes().clone();
        self.writes().post(move || {
            for (package, user) in discoveries {
                if let Err(error) =
                    host.notify_package_discoveries(&super::package_writes::PackageChanged {
                        package: &package,
                        user,
                        components: &[package.clone()],
                        removed: false,
                    })
                {
                    eprintln!("app host: package addition monitors: {}", error.message);
                }
            }
            for event in events {
                if let Err(error) = host.broadcasts.try_send_package_effect(
                    &host,
                    event.intent.clone(),
                    event.user,
                    if event.instant {
                        None
                    } else {
                        event.allow.as_deref()
                    },
                    &event.permissions,
                    event.temp_allowlist,
                ) {
                    eprintln!("app host: package effect broadcast: {}", error.message);
                }
                if event.notify {
                    if let Err(error) = writes.invoke_callbacks(
                        &{
                            let mut intent = event.intent.clone();
                            intent.set_flags(0, 0);
                            intent
                        },
                        event.user,
                        event.allow.as_deref(),
                    ) {
                        eprintln!("app host: package effect callbacks: {}", error.message);
                    }
                }
            }
        })
    }

    fn first_launch(&self, a: bridge::FirstLaunch) -> Result<(), Exception> {
        let name = required(a.package_name.as_deref(), "packageName")?;
        let (state, _) = self.host.packages()?;
        let instant = state
            .packages
            .get(name)
            .is_some_and(|ps| user_state(ps, a.user_id).instant_app);
        let mut events = Vec::new();
        deliver(
            &mut events,
            "android.intent.action.PACKAGE_FIRST_LAUNCH",
            Some(name),
            &[],
            0,
            a.installer_package.as_deref(),
            &[a.user_id],
            instant,
            None,
            false,
            None,
        );
        self.post(events, Vec::new())
    }

    fn post_install(&self, a: bridge::PostInstallPackage) -> Result<(), Exception> {
        let (events, discoveries) = self.post_install_events(a)?;
        self.post(events, discoveries)
    }

    fn post_install_events(
        &self,
        a: bridge::PostInstallPackage,
    ) -> Result<(Vec<Event>, Vec<(String, i32)>), Exception> {
        let name = required(a.name.as_deref(), "name")?;
        let (state, resolution) = self.host.packages()?;
        let ps = state
            .packages
            .get(name)
            .ok_or_else(|| state_error("post-install package absent"))?;
        if ps.app_id != a.app_id {
            return Err(state_error("post-install UID changed"));
        }
        let first = required_array(a.first_users, "firstUsers")?;
        let first_instant = required_array(a.first_instant_users, "firstInstantUsers")?;
        let updates = required_array(a.update_users, "updateUsers")?;
        let update_instant = required_array(a.update_instant_users, "updateInstantUsers")?;
        let removed = required_array(a.removed_users, "removedUsers")?;
        let removed_instant = required_array(a.removed_instant_users, "removedInstantUsers")?;
        let prior = decode_visibility(&required_array(a.prior_visibility, "priorVisibility")?)?;
        let target = a.installer.as_deref().or(a.old_installer.as_deref());
        let allow = |users: &[i32]| -> AllowLists {
            users
                .iter()
                .map(|user| {
                    (
                        *user,
                        resolution
                            .apps_filter
                            .visibility_allow_list(&state, ps, *user),
                    )
                })
                .collect()
        };
        let mut events = Vec::new();
        if a.replacing {
            let extras = vec![
                (UID, Value::Int(a.app_id)),
                ("android.intent.extra.DATA_REMOVED", Value::Bool(false)),
                (
                    "android.intent.extra.SYSTEM_UPDATE_UNINSTALL",
                    Value::Bool(false),
                ),
                (
                    "android.intent.extra.DONT_KILL_APP",
                    Value::Bool(a.dont_kill),
                ),
                ("android.intent.extra.USER_INITIATED", Value::Bool(true)),
                (REPLACING, Value::Bool(true)),
                (
                    "android.intent.extra.REMOVED_FOR_ALL_USERS",
                    Value::Bool(false),
                ),
            ];
            if let Some(installer) = a.old_installer.as_deref() {
                cohorts(
                    &mut events,
                    REMOVED,
                    Some(name),
                    &extras,
                    0,
                    Some(installer),
                    &removed,
                    &removed_instant,
                    None,
                    None,
                );
            }
            if !a.static_library {
                cohorts(
                    &mut events,
                    REMOVED,
                    Some(name),
                    &extras,
                    0,
                    None,
                    &removed,
                    &removed_instant,
                    Some(&prior),
                    None,
                );
                cohorts(
                    &mut events,
                    "android.intent.action.PACKAGE_REMOVED_INTERNAL",
                    Some(name),
                    &extras,
                    0,
                    Some("android"),
                    &removed,
                    &removed_instant,
                    Some(&prior),
                    None,
                );
            }
        }
        let mut extras = vec![
            (UID, Value::Int(a.app_id)),
            (DATA_LOADER, Value::Int(a.data_loader_type)),
        ];
        if a.replacing {
            extras.push((REPLACING, Value::Bool(true)));
        }
        if a.static_library {
            if let Some(installer) = target {
                let all: Vec<_> = first.iter().chain(&updates).copied().collect();
                cohorts(
                    &mut events,
                    ADDED,
                    Some(name),
                    &extras,
                    0,
                    Some(installer),
                    &all,
                    &[],
                    None,
                    None,
                );
            }
            return Ok((events, Vec::new()));
        }
        let controller = if self.safety_labels_enabled(&state)? {
            state
                .system
                .permission_controller_package
                .as_ref()
                .ok_or_else(|| state_error("permission controller selection unavailable"))?
                .as_deref()
        } else {
            None
        };
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let roles = state
            .system
            .roles
            .as_ref()
            .ok_or_else(|| state_error("KnownPackages owner unavailable"))?;
        let verifiers = roles.known_packages(&query, 4, 0).map_err(unavailable)?;
        let installers = roles.known_packages(&query, 2, 0).map_err(unavailable)?;
        let first_allow = allow(&first);
        let first_extras = [
            (UID, Value::Int(a.app_id)),
            (DATA_LOADER, Value::Int(a.data_loader_type)),
        ];
        cohorts(
            &mut events,
            ADDED,
            Some(name),
            &first_extras,
            0,
            None,
            &first,
            &first_instant,
            Some(&first_allow),
            None,
        );
        if let Some(controller) = controller {
            cohorts(
                &mut events,
                ADDED,
                Some(name),
                &first_extras,
                0,
                Some(controller),
                &first,
                &first_instant,
                Some(&first_allow),
                None,
            );
        }
        if a.system || a.virtual_preload {
            for user in &first {
                self.boot_completed(&mut events, &state, name, *user, a.virtual_preload)?;
            }
        }
        let update_allow = allow(&updates);
        cohorts(
            &mut events,
            ADDED,
            Some(name),
            &extras,
            0,
            None,
            &updates,
            &update_instant,
            Some(&update_allow),
            None,
        );
        if let Some(installer) = target {
            cohorts(
                &mut events,
                ADDED,
                Some(name),
                &extras,
                0,
                Some(installer),
                &updates,
                &update_instant,
                None,
                None,
            );
        }
        if let Some(controller) = controller {
            cohorts(
                &mut events,
                ADDED,
                Some(name),
                &extras,
                0,
                Some(controller),
                &updates,
                &update_instant,
                None,
                None,
            );
        }
        for verifier in verifiers
            .iter()
            .flatten()
            .filter(|v| Some(v.as_str()) != target)
        {
            cohorts(
                &mut events,
                ADDED,
                Some(name),
                &extras,
                0,
                Some(verifier),
                &updates,
                &update_instant,
                None,
                None,
            );
        }
        for installer in installers.iter().flatten() {
            cohorts(
                &mut events,
                ADDED,
                Some(name),
                &extras,
                0x01000000,
                Some(installer),
                &first,
                &update_instant,
                None,
                None,
            );
        }
        if a.replacing {
            cohorts(
                &mut events,
                REPLACED,
                Some(name),
                &extras,
                0,
                None,
                &updates,
                &update_instant,
                Some(&prior),
                None,
            );
            if let Some(installer) = target {
                cohorts(
                    &mut events,
                    REPLACED,
                    Some(name),
                    &extras,
                    0,
                    Some(installer),
                    &updates,
                    &update_instant,
                    None,
                    None,
                );
            }
            for verifier in verifiers
                .iter()
                .flatten()
                .filter(|v| Some(v.as_str()) != target)
            {
                cohorts(
                    &mut events,
                    REPLACED,
                    Some(name),
                    &extras,
                    0,
                    Some(verifier),
                    &updates,
                    &update_instant,
                    None,
                    None,
                );
            }
            cohorts(
                &mut events,
                "android.intent.action.MY_PACKAGE_REPLACED",
                None,
                &[],
                0,
                Some(name),
                &updates,
                &update_instant,
                None,
                Some(
                    self.host
                        .boot_time_temp_allowlist(311 /* REASON_PACKAGE_REPLACED */),
                ),
            );
        }
        let discoveries = (if first_instant.is_empty() {
            &first
        } else {
            &first_instant
        })
        .iter()
        .chain(if update_instant.is_empty() {
            &updates
        } else {
            &update_instant
        })
        .map(|user| (name.to_owned(), *user))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
        Ok((events, discoveries))
    }

    fn package_added(&self, a: bridge::PackageAdded) -> Result<(), Exception> {
        let name = required(a.package_name.as_deref(), "packageName")?;
        let (state, resolution) = self.host.packages()?;
        let ps = state
            .packages
            .get(name)
            .ok_or_else(|| state_error("package absent"))?;
        let user = a.user_id;
        if user != 0 {
            return Err(self
                .host
                .unimplemented_named("packageAdded", "profile user properties (#2015)"));
        }
        let instant = user_state(ps, user).instant_app;
        let allow = if instant {
            None
        } else {
            resolution
                .apps_filter
                .visibility_allow_list(&state, ps, user)
        };
        let mut extras = vec![
            (UID, Value::Int(uid(user, ps.app_id))),
            (DATA_LOADER, Value::Int(a.data_loader_type)),
        ];
        if a.archived {
            extras.push(("android.intent.extra.ARCHIVAL", Value::Bool(true)));
        }
        let mut events = Vec::new();
        deliver(
            &mut events,
            ADDED,
            Some(name),
            &extras,
            0,
            None,
            &[user],
            instant,
            allow.as_deref(),
            true,
            None,
        );
        if self.safety_labels_enabled(&state)? {
            let controller = state
                .system
                .permission_controller_package
                .as_ref()
                .ok_or_else(|| state_error("permission controller selection unavailable"))?
                .as_deref();
            deliver(
                &mut events,
                ADDED,
                Some(name),
                &extras,
                0,
                controller,
                &[user],
                instant,
                allow.as_deref(),
                false,
                None,
            );
        }
        if ps.is.system && !instant {
            self.boot_completed(&mut events, &state, name, user, false)?;
        }
        let launcher_user = super::user_service::profile_parent(&self.host, user)?.unwrap_or(user);
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let launcher =
            crate::package::query::preferred::default_home_for_instant(&query, launcher_user)
                .map_err(unavailable)?;
        let private_access = self
            .host
            .flag("android.multiuser.enable_private_space_features")?;
        let hidden = private_access
            && self
                .host
                .user_properties()?
                .iter()
                .any(|(name, value)| name == "profileApiVisibility" && *value == 1);
        let access = if !hidden {
            true
        } else if let Some(launcher) = &launcher {
            if let Some(ps) = state.packages.get(&launcher.package) {
                let launcher_uid = uid(launcher_user, ps.app_id);
                self.host.check_permission(
                    Some("android.permission.ACCESS_HIDDEN_PROFILES_FULL"),
                    launcher_uid,
                )? == 0
                    || self.host.check_permission(
                        Some("android.permission.ACCESS_HIDDEN_PROFILES"),
                        launcher_uid,
                    )? == 0
            } else {
                false
            }
        } else {
            false
        };
        let session = crate::package::installer::codec::SessionInfo {
            install_reason: user_state(ps, user).install_reason,
            app_package_name: Some(name.into()),
            // SessionInfo's field initializers (not Rust's zero defaults).
            parent_session_id: -1,
            child_session_ids: Some(Vec::new()),
            auto_revoke_permissions_mode: 3,
            ..Default::default()
        };
        let mut data = Parcel::new();
        session.write_to(&mut data);
        let mut handle = Parcel::new();
        handle.write_i32(user);
        let extras = [
            (
                "android.content.pm.extra.SESSION",
                Value::Parcelable {
                    class: "android.content.pm.PackageInstaller$SessionInfo".into(),
                    data: data.data().to_vec(),
                },
            ),
            (
                "android.intent.extra.USER",
                Value::Parcelable {
                    class: "android.os.UserHandle".into(),
                    data: handle.data().to_vec(),
                },
            ),
        ];
        for target in launcher
            .as_ref()
            .filter(|_| access)
            .map(|v| v.package.as_str())
            .into_iter()
            .chain(a.prediction_package.as_deref())
        {
            let intent = RawIntent::with_data(
                Some("android.content.pm.action.SESSION_COMMITTED"),
                None,
                Some(target),
                0,
                &extras,
            );
            events.push(Event {
                intent,
                user: launcher_user,
                allow: None,
                permissions: Vec::new(),
                instant: false,
                notify: false,
                temp_allowlist: None,
            });
        }
        self.post(events, vec![(name.into(), user)])
    }

    fn boot_completed(
        &self,
        events: &mut Vec<Event>,
        state: &crate::package::model::State,
        name: &str,
        user: i32,
        include_stopped: bool,
    ) -> Result<(), Exception> {
        if user != 0 {
            return Err(self
                .host
                .unimplemented_named("package boot completed", "running profile users (#2015)"));
        }
        let unlocked = state
            .users
            .get(&user)
            .is_some_and(|user| user.unlocking_or_unlocked);
        // The app host's activity user owner runs user 0.
        for action in std::iter::once("android.intent.action.LOCKED_BOOT_COMPLETED")
            .chain(unlocked.then_some("android.intent.action.BOOT_COMPLETED"))
        {
            let intent = RawIntent::with_data(
                Some(action),
                None,
                Some(name),
                if include_stopped { 0x20 } else { 0 },
                &[(USER, Value::Int(user))],
            );
            events.push(Event {
                intent,
                user,
                allow: None,
                permissions: vec!["android.permission.RECEIVE_BOOT_COMPLETED".into()],
                instant: false,
                notify: false,
                temp_allowlist: Some(self.host.boot_time_temp_allowlist(202)),
            });
        }
        Ok(())
    }
}

impl Service for PackageEffects {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native package effects require system UID",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::REQUIRE_INSTALLER_PERMISSION => {
                let a = bridge::RequireInstallerPermission::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.require_installer_permission(a.calling_uid)
                    .map(|v| bridge::write_require_installer_permission_reply(&mut p, v))
            }
            bridge::KILL_PACKAGE => {
                let a = bridge::KillPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.package_name
                    .as_deref()
                    .map_or(Ok(()), |name| {
                        self.host.kill_application(
                            name,
                            a.app_id,
                            a.user_id,
                            a.reason.as_deref().unwrap_or("null"),
                            a.exit_reason,
                        )
                    })
                    .map(|()| bridge::write_kill_package_reply(&mut p))
            }
            bridge::CAPTURE_POST_INSTALL_VISIBILITY => {
                let a = bridge::CapturePostInstallVisibility::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.capture_visibility(a)
                    .map(|v| bridge::write_capture_post_install_visibility_reply(&mut p, &Some(v)))
            }
            bridge::POST_INSTALL_PACKAGE => {
                let a = bridge::PostInstallPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.post_install(a)
                    .map(|()| bridge::write_post_install_package_reply(&mut p))
            }
            bridge::PACKAGE_ADDED => {
                let a = bridge::PackageAdded::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.package_added(a)
                    .map(|()| bridge::write_package_added_reply(&mut p))
            }
            bridge::FIRST_LAUNCH => {
                let a = bridge::FirstLaunch::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.first_launch(a)
                    .map(|()| bridge::write_first_launch_reply(&mut p))
            }
            bridge::IS_STATE_PROTECTED => {
                let a = bridge::IsStateProtected::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.protected(a.package_name.as_deref())
                    .map(|v| bridge::write_is_state_protected_reply(&mut p, v))
            }
            bridge::IS_DATA_PROTECTED => {
                let a = bridge::IsDataProtected::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.protected(a.package_name.as_deref())
                    .map(|v| bridge::write_is_data_protected_reply(&mut p, v))
            }
            bridge::PACKAGE_CHANGED => {
                let a = bridge::PackageChanged::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let name = a.package_name.as_deref().ok_or_else(|| {
                    Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName")
                });
                let components = a
                    .components
                    .ok_or_else(|| {
                        Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "components")
                    })
                    .and_then(|values| {
                        values
                            .into_iter()
                            .map(|name| {
                                name.ok_or_else(|| {
                                    Exception::new(
                                        aim_binder_host::parcel::EX_NULL_POINTER,
                                        "component",
                                    )
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()
                    });
                name.and_then(|name| {
                    components.and_then(|components| {
                        self.host
                            .package_writes
                            .get_or_init(|| super::package_writes::Writes::new(self.host.clone()))
                            .changed(
                                name,
                                a.package_uid,
                                a.dont_kill_app,
                                components,
                                a.delayed,
                                a.reason.as_deref(),
                            )
                    })
                })
                .map(|()| bridge::write_package_changed_reply(&mut p))
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("package effects", &format!("{name} (#2154)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::ServiceOwner;

    /// The writer's effects owner takes the bridge; an effect not ported
    /// names its issue, and no package is protected without a name.
    #[test]
    fn the_writer_takes_the_effects_bridge() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(PackageEffects::new(host)))
        else {
            unreachable!()
        };
        crate::package::effects::Owner::new(ServiceOwner::Local(
            process.local_service(ptr).unwrap(),
        ))
        .unwrap();
        let node = process.local_service(ptr).unwrap();
        let mut p = Parcel::new();
        bridge::IsStateProtected {
            package_name: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = node
            .transact(bridge::IS_STATE_PROTECTED, &p, false)
            .unwrap();
        assert!(
            !bridge::read_is_state_protected_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let mut p = Parcel::new();
        bridge::UserRemoved { user_id: 10 }.write(&mut p);
        let reply = node.transact(bridge::USER_REMOVED, &p, false).unwrap();
        let e = bridge::read_user_removed_reply(&mut reply.reader())
            .unwrap()
            .unwrap_err();
        assert!(e.message.contains("userRemoved (#2154)"), "{}", e.message);
    }
}

const UID: &str = "android.intent.extra.UID";
const USER: &str = "android.intent.extra.user_handle";
const REPLACING: &str = "android.intent.extra.REPLACING";
const DATA_LOADER: &str = "android.content.pm.extra.DATA_LOADER_TYPE";
const ADDED: &str = "android.intent.action.PACKAGE_ADDED";
const REMOVED: &str = "android.intent.action.PACKAGE_REMOVED";
const REPLACED: &str = "android.intent.action.PACKAGE_REPLACED";
type AllowLists = BTreeMap<i32, Option<Vec<i32>>>;

struct Event {
    intent: RawIntent,
    user: i32,
    allow: Option<Vec<i32>>,
    permissions: Vec<String>,
    instant: bool,
    notify: bool,
    temp_allowlist: Option<TempAllowlist>,
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, Exception> {
    value.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, name))
}
fn required_array(value: Option<Vec<i32>>, name: &str) -> Result<Vec<i32>, Exception> {
    value.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, name))
}
fn state_error(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
fn unavailable(error: crate::package::apps_filter::NotModelled) -> Exception {
    state_error(error.0)
}

/// `decodeVisibility`: the caller strips the VIS2 metadata header before
/// postInstallPackage. A null list remains different from an empty list.
fn decode_visibility(encoded: &[i32]) -> Result<AllowLists, Exception> {
    let mut lists = BTreeMap::new();
    let mut i = 0;
    while i < encoded.len() {
        let Some(pair) = encoded.get(i..i + 2) else {
            return Err(Exception::illegal_argument("prior visibility truncated"));
        };
        let (user, count) = (pair[0], pair[1]);
        i += 2;
        if user < 0 || count < -1 || count > (encoded.len() - i) as i32 || lists.contains_key(&user)
        {
            return Err(Exception::illegal_argument("prior visibility invalid"));
        }
        let values = if count < 0 {
            None
        } else {
            let values = encoded[i..i + count as usize].to_vec();
            i += count as usize;
            Some(values)
        };
        lists.insert(user, values);
    }
    Ok(lists)
}

/// `deliver`: instant cohorts take precedence, as in BroadcastHelper.
#[allow(clippy::too_many_arguments)]
fn cohorts(
    events: &mut Vec<Event>,
    action: &str,
    name: Option<&str>,
    extras: &[(&str, Value)],
    flags: i32,
    target: Option<&str>,
    normal: &[i32],
    instant: &[i32],
    lists: Option<&AllowLists>,
    temp: Option<TempAllowlist>,
) {
    let is_instant = !instant.is_empty();
    for user in if is_instant { instant } else { normal } {
        let mut extras = extras.to_vec();
        if target == Some("android") && lists.is_some() {
            let value = lists
                .and_then(|lists| lists.get(user))
                .and_then(|list| list.as_ref());
            extras.push((
                "android.intent.extra.VISIBILITY_ALLOW_LIST",
                value.map_or(Value::Null, |list| Value::IntArray(list.clone())),
            ));
        }
        deliver(
            events,
            action,
            name,
            &extras,
            flags,
            target,
            &[*user],
            is_instant,
            lists
                .and_then(|lists| lists.get(user))
                .and_then(|list| list.as_deref()),
            target.is_none(),
            temp,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn deliver(
    events: &mut Vec<Event>,
    action: &str,
    name: Option<&str>,
    extras: &[(&str, Value)],
    flags: i32,
    target: Option<&str>,
    users: &[i32],
    instant: bool,
    allow: Option<&[i32]>,
    notify: bool,
    temp_allowlist: Option<TempAllowlist>,
) {
    for user in users {
        let mut extras = extras.to_vec();
        for (key, value) in &mut extras {
            if *key == UID {
                if let Value::Int(value) = value {
                    if *value >= 0 {
                        *value = uid(*user, *value % 100000);
                    }
                }
            }
        }
        extras.push((USER, Value::Int(*user)));
        let data = name.map(|name| Uri::preferred_opaque("package", name));
        let intent = RawIntent::with_data(
            Some(action),
            data.as_ref(),
            target,
            flags | 0x04000000,
            &extras,
        );
        events.push(Event {
            intent,
            user: *user,
            allow: allow.map(<[i32]>::to_vec),
            permissions: if instant {
                vec!["android.permission.ACCESS_INSTANT_APPS".into()]
            } else {
                Vec::new()
            },
            instant,
            notify,
            temp_allowlist,
        });
    }
}

#[cfg(test)]
#[path = "package_effects_tests.rs"]
mod effect_tests;
