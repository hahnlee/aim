//! AppHibernationService (`android-16.0.0_r1`) for the app host. Its states
//! are in memory as the original keeps them, the global ones loaded at
//! `PHASE_BOOT_COMPLETED` and the user's at `onUserUnlocking` from
//! UserLevelHibernationProto and GlobalLevelHibernationProto, then kept by
//! the package additions and removals of its receiver and left by use; each
//! change is written back through AtomicFile. Global hibernation deletes the
//! package's ART artifacts when the image enables it, and keeps the package
//! from the package boot's and ART's dexopt selection; leaving a user's
//! hibernation replays the boot broadcasts. Entering a user's hibernation
//! (its force stop, cache deletion and storage statistics), the statsd atoms
//! and the shell command remain (#2064). The app host's one user is user 0.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex, OnceLock},
};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, Exception, Parcel,
};
use aim_service_aidl::android_apphibernation_iapphibernationservice as ah;

use super::exit_info::{ProtoWriter, Value, fields};
use super::kept::RawIntent;
use super::usage_database::{atomic_read, atomic_write};
use super::{AppHost, activity_user::AllowMode, reply, usage_events};
use crate::package::query::Query;

const MANAGE: &str = "android.permission.MANAGE_APP_HIBERNATION";
const RECEIVE_BOOT_COMPLETED: &str = "android.permission.RECEIVE_BOOT_COMPLETED";
/// The service's own calls (`Process.SYSTEM_UID`).
const SYSTEM: i32 = 1000;

/// `mUserStates` of user 0 and `mGlobalHibernationStates`, absent until
/// read, and the constructor's `mOatArtifactDeletionEnabled`, read once.
#[derive(Default)]
pub(super) struct States(Mutex<Loaded>, OnceLock<bool>);

#[derive(Default)]
struct Loaded {
    user: Option<BTreeMap<String, State>>,
    global: Option<BTreeMap<String, State>>,
}

pub struct AppHibernation(pub Arc<AppHost>);

impl AppHibernation {
    /// PackageManager's unstop (`NativePackageEffectsBridge.unhibernate`):
    /// a package hibernating for the user leaves both hibernations.
    pub(super) fn unhibernate(&self, name: &str, user: i32) -> Result<(), Exception> {
        if self.hibernating(SYSTEM, Some(name), user, false)? {
            self.set_for_user(SYSTEM, Some(name), user, false)?;
            self.set_globally(SYSTEM, Some(name), false)?;
        }
        Ok(())
    }

    /// The usage event listener: user interaction, a resumed activity or a
    /// used component leaves both hibernations.
    pub(super) fn used(&self, name: &str, user: i32, event_type: i32) -> Result<(), Exception> {
        if !self.enabled()?
            || !matches!(
                event_type,
                usage_events::USER_INTERACTION
                    | usage_events::ACTIVITY_RESUMED
                    | usage_events::APP_COMPONENT_USED
            )
        {
            return Ok(());
        }
        self.set_for_user(SYSTEM, Some(name), user, false)?;
        self.set_globally(SYSTEM, Some(name), false)
    }

    /// App restriction levels do not model a package hibernating for the user.
    pub(super) fn restrictions_unmodelled(&self, user: i32) -> Result<(), Exception> {
        if user == 0
            && self.enabled()?
            && (self.0.hibernation.0.lock().unwrap().user.as_ref())
                .is_some_and(|states| states.values().any(|s| s.hibernated))
        {
            return Err(self.0.unimplemented_named(
                "app_hibernation",
                "restriction levels of hibernating packages (#2064)",
            ));
        }
        Ok(())
    }

    /// `onBootPhase(PHASE_BOOT_COMPLETED)`: `initializeGlobalHibernationStates`.
    pub(super) fn boot_completed(&self) -> Result<(), Exception> {
        let states = self.initial(true)?;
        self.0.hibernation.0.lock().unwrap().global = Some(states);
        Ok(())
    }

    /// `onUserUnlocking` of user 0: `initializeUserHibernationStates`, then
    /// the packages the user does not hibernate leave global hibernation.
    pub(super) fn user_unlocking(&self) -> Result<(), Exception> {
        let states = self.initial(false)?;
        let awake: Vec<String> = {
            let mut loaded = self.0.hibernation.0.lock().unwrap();
            let global = loaded.global.as_ref();
            let awake = states
                .iter()
                .filter(|(name, s)| {
                    !s.hibernated
                        && global
                            .and_then(|g| g.get(*name))
                            .is_some_and(|g| g.hibernated)
                })
                .map(|(name, _)| name.clone())
                .collect();
            loaded.user = Some(states);
            awake
        };
        for name in awake {
            self.set_globally(SYSTEM, Some(&name), false)?;
        }
        Ok(())
    }

    /// The receiver's `PACKAGE_ADDED` without `EXTRA_REPLACING`.
    pub(super) fn package_added(&self, name: &str, user: i32) {
        let mut loaded = self.0.hibernation.0.lock().unwrap();
        let Loaded {
            user: states,
            global,
        } = &mut *loaded;
        if let (0, Some(states)) = (user, states) {
            states.insert(name.into(), State::default());
            if let Some(global) = global {
                global.entry(name.into()).or_default();
            }
        }
    }

    /// The receiver's `PACKAGE_REMOVED` without `EXTRA_REPLACING`, with
    /// its `EXTRA_REMOVED_FOR_ALL_USERS`.
    pub(super) fn package_removed(&self, name: &str, user: i32, all_users: bool) {
        let mut loaded = self.0.hibernation.0.lock().unwrap();
        if let (0, Some(states)) = (user, loaded.user.as_mut()) {
            states.remove(name);
        }
        // `onPackageRemovedForAllUsers` does not depend on the user's states.
        if all_users && let Some(global) = loaded.global.as_mut() {
            global.remove(name);
        }
    }

    fn enabled(&self) -> Result<bool, Exception> {
        Ok(self
            .0
            .platform()?
            .settings
            .device_config("app_hibernation", "app_hibernation_enabled")
            .is_none_or(|v| v.eq_ignore_ascii_case("true")))
    }

    fn enforce(&self, uid: i32, method: &str) -> Result<(), Exception> {
        let host = &self.0;
        if host.check_permission(Some(MANAGE), uid)? != 0 {
            let message = if method == "isHibernatingForUser" {
                format!("Caller did not have permission while calling {method}")
            } else {
                "Caller does not have MANAGE_APP_HIBERNATION permission.".into()
            };
            return Err(Exception::security(format!(
                "{message}: Neither user {uid} nor current process has {MANAGE}."
            )));
        }
        Ok(())
    }

    fn user(&self, uid: i32, user: i32, method: &str) -> Result<i32, Exception> {
        self.enforce(uid, method)?;
        self.0
            .handle_incoming_user(uid, user, false, AllowMode::FullOnly, Some(method), None)
    }

    fn path(&self, global: bool) -> Result<std::path::PathBuf, Exception> {
        Ok(self.0.data()?.join(if global {
            "system/hibernation/states"
        } else {
            "system_ce/0/hibernation/states"
        }))
    }

    /// The installed packages' awake states, with their recorded states;
    /// a package's that is not stopped no longer hibernates for the user.
    fn initial(&self, global: bool) -> Result<BTreeMap<String, State>, Exception> {
        let mut states = match atomic_read(&self.path(global)?) {
            Ok(bytes) => decode(&bytes, global).map_err(error)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(error(e)),
        };
        let capture = self.0.package_capture()?;
        let packages = &capture.state().packages;
        states.retain(|name, state| {
            let Some(ps) = packages.get(name) else {
                return false;
            };
            if !global && !crate::package::info::user_state(ps, 0).stopped {
                state.hibernated = false;
            }
            true
        });
        for (name, ps) in packages {
            if ps.pkg.is_some() {
                states.entry(name.clone()).or_default();
            }
        }
        Ok(states)
    }

    /// `HibernationStateDiskStore.writeStateProto`: a failed write is logged.
    fn persist(&self, global: bool, states: &BTreeMap<String, State>) -> Result<(), Exception> {
        let mut out = ProtoWriter::default();
        for (name, state) in states {
            let mut entry = ProtoWriter::default();
            entry.string(1, Some(name));
            entry.varint(2, state.hibernated.into());
            if global {
                entry.varint(3, state.saved);
            }
            out.message(1, &entry.0);
        }
        let path = self.path(global)?;
        let written = (|| {
            let dir = path.parent().unwrap();
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                // AtomicFile.startWrite sets a newly created parent to 0771.
                aim_storage::guest_inode::record(
                    dir,
                    aim_storage::guest_inode::GuestInode {
                        uid: Some(1000),
                        gid: Some(1000),
                        mode: Some(0o771),
                    },
                )?;
            }
            atomic_write(&path, &out.0)
        })();
        if let Err(e) = written {
            eprintln!("app host: HibernationStateDiskStore: Failed to write states protobuf: {e}");
        }
        Ok(())
    }

    fn visible(&self, uid: i32, name: &str) -> Result<bool, Exception> {
        let (state, resolution) = self.0.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        query
            .internal_can_query(uid, Some(name))
            .map_err(|e| self.0.unimplemented_named("app_hibernation", e.0))?
    }

    fn deletion_enabled(&self) -> Result<bool, Exception> {
        let once = &self.0.hibernation.1;
        if let Some(&enabled) = once.get() {
            return Ok(enabled);
        }
        let enabled = self
            .0
            .framework()?
            .framework_bool("config_hibernationDeletesOatArtifactsEnabled")
            .ok_or_else(|| error("missing config_hibernationDeletesOatArtifactsEnabled"))?;
        Ok(*once.get_or_init(|| enabled))
    }

    fn hibernating(
        &self,
        uid: i32,
        name: Option<&str>,
        user: i32,
        global: bool,
    ) -> Result<bool, Exception> {
        if !self.enabled()? {
            return Ok(false);
        }
        let user = if global {
            self.enforce(uid, "isHibernatingGlobally")?;
            0
        } else {
            self.user(uid, user, "isHibernatingForUser")?
        };
        let Some(name) = name else { return Ok(false) };
        let hibernated = {
            let loaded = self.0.hibernation.0.lock().unwrap();
            let states = if global {
                loaded.global.as_ref()
            } else {
                loaded.user.as_ref().filter(|_| user == 0)
            };
            states
                .and_then(|s| s.get(name))
                .is_some_and(|s| s.hibernated)
        };
        Ok(hibernated && self.visible(uid, name)?)
    }

    /// `setHibernatingForUser`.
    fn set_for_user(
        &self,
        uid: i32,
        name: Option<&str>,
        user: i32,
        hibernating: bool,
    ) -> Result<(), Exception> {
        if !self.enabled()? {
            return Ok(());
        }
        let method = "setHibernatingForUser";
        self.enforce(uid, method)?;
        let user = self.0.handle_incoming_user(
            uid,
            user,
            false,
            AllowMode::FullOnly,
            Some(method),
            None,
        )?;
        let visible = name.map(|n| self.visible(uid, n)).transpose()?;
        let name = {
            let mut loaded = self.0.hibernation.0.lock().unwrap();
            let Some(states) = loaded.user.as_mut().filter(|_| user == 0) else {
                eprintln!(
                    "app host: AppHibernationService: Attempt to call setHibernatingForUser on stopped or nonexistent user {user}"
                );
                return Ok(());
            };
            let Some((name, state)) = name
                .filter(|_| visible == Some(true))
                .and_then(|n| Some((n, states.get_mut(n)?)))
            else {
                eprintln!(
                    "app host: AppHibernationService: Package {name:?} is not installed for user {user}"
                );
                return Ok(());
            };
            if state.hibernated == hibernating {
                return Ok(());
            }
            if hibernating {
                return Err(self.0.unimplemented_named(
                    "app_hibernation",
                    "user hibernation: force stop, cache deletion, storage stats (#2064)",
                ));
            }
            state.hibernated = false;
            self.persist(false, states)?;
            name
        };
        // `unhibernatePackageForUser`: the package registers its alarms and
        // jobs again.
        for action in [
            "android.intent.action.LOCKED_BOOT_COMPLETED",
            "android.intent.action.BOOT_COMPLETED",
        ] {
            self.0.broadcasts.send_from_system_with(
                &self.0,
                RawIntent::with_data(Some(action), None, Some(name), 0, &[]),
                user,
                &[RECEIVE_BOOT_COMPLETED.into()],
                None,
                None,
            );
        }
        Ok(())
    }

    /// `setHibernatingGlobally`; `hibernatePackageGlobally` deletes the
    /// package's ART artifacts, without the lock, when the image enables it.
    fn set_globally(
        &self,
        uid: i32,
        name: Option<&str>,
        hibernating: bool,
    ) -> Result<(), Exception> {
        if !self.enabled()? {
            return Ok(());
        }
        self.enforce(uid, "setHibernatingGlobally")?;
        // The constructor's `mOatArtifactDeletionEnabled`, read before any change.
        let deletes = hibernating && self.deletion_enabled()?;
        let visible = name.map(|n| self.visible(uid, n)).transpose()?;
        let name = {
            let mut loaded = self.0.hibernation.0.lock().unwrap();
            let found = name.filter(|_| visible == Some(true));
            let Some((name, states)) = found.zip(loaded.global.as_mut()) else {
                not_installed(name);
                return Ok(());
            };
            let Some(state) = states.get_mut(name) else {
                not_installed(Some(name));
                return Ok(());
            };
            if state.hibernated == hibernating {
                return Ok(());
            }
            state.hibernated = hibernating;
            if !hibernating {
                state.saved = 0;
                return self.persist(true, states);
            }
            name
        };
        // PackageManagerService.deleteOatArtifactsOfPackage logs ART's
        // IllegalArgument/IllegalStateException and answers -1; the change
        // itself has already succeeded.
        let saved = if deletes {
            self.0.delete_oat_artifacts(Some(name.into()), SYSTEM)
        } else {
            Ok(0)
        };
        let saved = match saved {
            Err(e) if matches!(e.code, EX_ILLEGAL_ARGUMENT | EX_ILLEGAL_STATE) => {
                eprintln!("app host: PackageManager: {}", e.message);
                Ok(-1)
            }
            saved => saved,
        };
        let mut loaded = self.0.hibernation.0.lock().unwrap();
        if let Some(states) = loaded.global.as_mut() {
            if let (Ok(saved), Some(state)) = (&saved, states.get_mut(name)) {
                state.saved = (*saved).max(0);
            }
            self.persist(true, states)?;
        }
        saved.map(|_| ())
    }

    /// `isHibernatingGlobally && isOatArtifactDeletionEnabled`: the package
    /// boot's dexopt suppression, which ART's `Utils.canDexoptPackage` asks
    /// over Binder as the system.
    pub(super) fn suppresses_dexopt(&self, name: &str) -> Result<bool, Exception> {
        Ok(self.hibernating(SYSTEM, Some(name), 0, true)? && self.deletion_enabled()?)
    }
}

fn not_installed(name: Option<&str>) {
    eprintln!("app host: AppHibernationService: Package {name:?} is not installed for any user");
}

#[derive(Default)]
struct State {
    hibernated: bool,
    saved: i64,
}

fn error(message: impl std::fmt::Display) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, format!("app_hibernation: {message}"))
}

/// The pinned UserLevel/GlobalLevelHibernationProto read only fields 1/2
/// (package name / hibernated); only global state persists field 3 (saved bytes).
fn decode(bytes: &[u8], global: bool) -> Result<BTreeMap<String, State>, String> {
    let mut states = BTreeMap::new();
    fields(bytes, |number, value| {
        if number != 1 {
            return Ok(());
        }
        let mut name = None;
        let mut state = State::default();
        fields(value.bytes()?, |number, value| {
            match number {
                1 => name = Some(value.string()?),
                2 => state.hibernated = value.int()? != 0,
                3 if global => {
                    state.saved = match value {
                        Value::Varint(v) => v as i64,
                        _ => return Err("wire type mismatch".into()),
                    }
                }
                _ => {}
            }
            Ok(())
        })?;
        if let Some(name) = name {
            states.insert(name, state);
        }
        Ok(())
    })?;
    Ok(states)
}

impl Service for AppHibernation {
    fn descriptor(&self) -> &str {
        ah::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let uid = call.sender_euid as i32;
        let result = match call.code {
            ah::IS_HIBERNATING_FOR_USER => {
                let a = ah::IsHibernatingForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.hibernating(uid, a.package_name.as_deref(), a.user_id, false)
                    .map(|v| ah::write_is_hibernating_for_user_reply(&mut p, v))
            }
            ah::SET_HIBERNATING_FOR_USER => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                let name = call.data.read_string16()?;
                let (user, hibernating) = (call.data.read_i32()?, call.data.read_bool()?);
                self.set_for_user(uid, name.as_deref(), user, hibernating)
                    .map(|()| p.write_no_exception())
            }
            ah::IS_HIBERNATING_GLOBALLY => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                let name = call.data.read_string16()?;
                self.hibernating(uid, name.as_deref(), 0, true).map(|v| {
                    p.write_no_exception();
                    p.write_bool(v);
                })
            }
            ah::SET_HIBERNATING_GLOBALLY => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                let name = call.data.read_string16()?;
                let hibernating = call.data.read_bool()?;
                self.set_globally(uid, name.as_deref(), hibernating)
                    .map(|()| p.write_no_exception())
            }
            ah::GET_HIBERNATING_PACKAGES_FOR_USER => {
                let a = ah::GetHibernatingPackagesForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let mut names = Vec::new();
                    if self.enabled()? {
                        let user = self.user(uid, a.user_id, "getHibernatingPackagesForUser")?;
                        let hibernated: Vec<String> = {
                            let loaded = self.0.hibernation.0.lock().unwrap();
                            let states = loaded.user.iter().filter(|_| user == 0).flatten();
                            states
                                .filter(|(_, s)| s.hibernated)
                                .map(|(name, _)| name.clone())
                                .collect()
                        };
                        for name in hibernated {
                            if self.visible(uid, &name)? {
                                names.push(Some(name));
                            }
                        }
                    }
                    ah::write_get_hibernating_packages_for_user_reply(&mut p, &Some(names));
                    Ok(())
                })()
            }
            ah::GET_HIBERNATION_STATS_FOR_USER => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                let names = aim_service_aidl::read_string_list(&mut call.data)?;
                let user = call.data.read_i32()?;
                (|| {
                    let mut entries = Vec::new();
                    if self.enabled()? {
                        let user = self.user(uid, user, "getHibernationStatsForUser")?;
                        let saved: Vec<(String, i64)> = {
                            let loaded = self.0.hibernation.0.lock().unwrap();
                            let users = loaded.user.iter().filter(|_| user == 0).flatten();
                            users
                                .filter(|(name, _)| {
                                    names.as_ref().is_none_or(|names| {
                                        names.iter().any(|n| n.as_deref() == Some(name))
                                    })
                                })
                                .filter_map(|(name, state)| {
                                    let global = loaded.global.as_ref()?.get(name)?;
                                    Some((name.clone(), state.saved.wrapping_add(global.saved)))
                                })
                                .collect()
                        };
                        for (name, saved) in saved {
                            if !self.visible(uid, &name)? {
                                continue;
                            }
                            let mut value = Parcel::new();
                            value.write_string16(Some("android.apphibernation.HibernationStats"));
                            value.write_i64(saved);
                            entries.push((
                                aim_service_aidl::ParcelValue::String(Some(name)),
                                aim_service_aidl::ParcelValue::Custom {
                                    kind: 4,
                                    bytes: value.data().to_vec(),
                                },
                            ));
                        }
                    }
                    p.write_no_exception();
                    aim_service_aidl::write_value_map(&mut p, Some(&entries));
                    Ok(())
                })()
            }
            ah::IS_OAT_ARTIFACT_DELETION_ENABLED => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                self.enforce(uid, "isOatArtifactDeletionEnabled")
                    .and_then(|_| {
                        let enabled = self.deletion_enabled()?;
                        p.write_no_exception();
                        p.write_bool(enabled);
                        Ok(())
                    })
            }
            _ => Err(self
                .0
                .unimplemented_named("app_hibernation", "hibernation operations (#2064)")),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests;
