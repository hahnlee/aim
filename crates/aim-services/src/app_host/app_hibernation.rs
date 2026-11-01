//! AppHibernationService reads (`android-16.0.0_r1`) for the app host.
//! UserLevelHibernationProto and GlobalLevelHibernationProto retain the original
//! disk format. Transitions still require their cache/ART/boot-broadcast owners
//! (#2064, #2063); reads must not discard their persisted state.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::{collections::BTreeMap, io, sync::Arc};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_apphibernation_iapphibernationservice as ah;

use super::exit_info::{Value, fields};
use super::usage_database::atomic_read;
use super::{AppHost, activity_user::AllowMode, reply};
use crate::package::query::Query;

const MANAGE: &str = "android.permission.MANAGE_APP_HIBERNATION";

pub struct AppHibernation(pub Arc<AppHost>);

impl AppHibernation {
    /// The package effect only transitions a package that is hibernating.
    /// Never clear its state without replaying the original boot broadcasts.
    pub(super) fn unhibernate(&self, user: i32) -> Result<(), Exception> {
        if !self.enabled()? {
            return Ok(());
        }
        self.user(1000, user, "isHibernatingForUser")?;
        if self.states(false, user)?.values().any(|s| s.hibernated)
            || self.states(true, user)?.values().any(|s| s.hibernated)
        {
            return Err(self
                .0
                .unimplemented_named("app_hibernation", "unhibernation effects (#2064)"));
        }
        Ok(())
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

    fn states(&self, global: bool, user: i32) -> Result<BTreeMap<String, State>, Exception> {
        if !global && user != 0 {
            return Ok(BTreeMap::new());
        }
        let relative = if global {
            "system/hibernation/states".into()
        } else {
            format!("system_ce/{user}/hibernation/states")
        };
        let path = self.0.data()?.join(relative);
        let mut states = match atomic_read(&path) {
            Ok(bytes) => decode(&bytes, global).map_err(error)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(error(e)),
        };
        let capture = self.0.package_capture()?;
        // initialize{User,Global}HibernationStates: installed inventory supplies
        // default awake rows; stale removed-package records do not survive.
        states.retain(|name, _| capture.state().packages.contains_key(name));
        for (name, ps) in &capture.state().packages {
            if ps.pkg.is_some() {
                states.entry(name.clone()).or_default();
            }
        }
        Ok(states)
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
        Ok(self
            .states(global, user)?
            .get(name)
            .is_some_and(|s| s.hibernated)
            && self.visible(uid, name)?)
    }
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
            ah::IS_HIBERNATING_GLOBALLY => {
                call.data.enforce_interface(ah::DESCRIPTOR)?;
                let name = call.data.read_string16()?;
                self.hibernating(uid, name.as_deref(), 0, true).map(|v| {
                    p.write_no_exception();
                    p.write_bool(v);
                })
            }
            ah::GET_HIBERNATING_PACKAGES_FOR_USER => {
                let a = ah::GetHibernatingPackagesForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let mut names = Vec::new();
                    if self.enabled()? {
                        let user = self.user(uid, a.user_id, "getHibernatingPackagesForUser")?;
                        for (name, state) in self.states(false, user)? {
                            if state.hibernated && self.visible(uid, &name)? {
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
                        let users = self.states(false, user)?;
                        let global = self.states(true, 0)?;
                        for (name, state) in users {
                            if names.as_ref().is_some_and(|names| {
                                !names.iter().any(|n| n.as_deref() == Some(&name))
                            }) || !self.visible(uid, &name)?
                            {
                                continue;
                            }
                            let Some(global) = global.get(&name) else {
                                continue;
                            };
                            let mut value = Parcel::new();
                            value.write_string16(Some("android.apphibernation.HibernationStats"));
                            value.write_i64(state.saved.wrapping_add(global.saved));
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
                        let enabled = self
                            .0
                            .framework()?
                            .framework_bool("config_hibernationDeletesOatArtifactsEnabled")
                            .ok_or_else(|| {
                                error("missing config_hibernationDeletesOatArtifactsEnabled")
                            })?;
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
