//! Settings.createNewUserLI state and installd requests, Android 16 r1.
//! Copyright The Android Open Source Project, Apache License 2.0.
//! UserManager allocates the user and owns user-info/data; this owner never does.
use super::{
    info, restrictions::UserState, scan::SigningScan, scan_snapshot::query_state::Capture, settings,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct CreationPolicy {
    pub current_time_millis: i64,
    pub fix_system_apps_first_install_time: bool,
    pub stop_system_packages_by_default: bool,
    pub initial_non_stopped_system_packages: BTreeSet<String>,
}

/// Installer.buildCreateAppDataArgs: DE only, plus SDK storage when used.
#[derive(Clone, Debug, PartialEq)]
pub struct AppData {
    pub volume_uuid: Option<String>,
    pub package: String,
    pub user: i32,
    pub flags: i32,
    pub app_id: i32,
    pub seinfo: Option<String>,
    pub target_sdk_version: i32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Creation {
    pub user: i32,
    pub states: BTreeMap<String, UserState>,
    pub app_data: Vec<AppData>,
    pub kernel_exclusions: Vec<String>,
}
impl Creation {
    /// Launcher resolution is the original Computer query in user 0 with both
    /// MATCH_DIRECT_BOOT_AWARE and MATCH_DIRECT_BOOT_UNAWARE (0xc0000).
    pub fn prepare(
        capture: &Capture,
        user: i32,
        installable: Option<&BTreeSet<String>>,
        disallowed: &[String],
        policy: &CreationPolicy,
        has_launcher: impl FnMut(&str, i32, i64) -> Result<bool, String>,
    ) -> Result<Self, String> {
        if user < 0 {
            return Err("new package user is negative".into());
        }
        Self::prepare_state(
            capture.scan().owner(),
            capture.state(),
            user,
            installable,
            disallowed,
            policy,
            has_launcher,
        )
    }
    fn prepare_state(
        scan: &SigningScan,
        current: &super::model::State,
        user: i32,
        installable: Option<&BTreeSet<String>>,
        disallowed: &[String],
        policy: &CreationPolicy,
        mut has_launcher: impl FnMut(&str, i32, i64) -> Result<bool, String>,
    ) -> Result<Self, String> {
        let mut packages: Vec<_> = scan.settings.packages.iter().collect();
        packages.sort_by_key(|package| info::java_hash(&package.name));
        let mut result = Self {
            user,
            states: BTreeMap::new(),
            app_data: Vec::new(),
            kernel_exclusions: Vec::new(),
        };
        for setting in packages {
            let mut state = scan
                .scanned_user_states(&setting.name)
                .ok_or("new-user package state owner absent")?
                .get(&user)
                .cloned()
                .unwrap_or_default();
            let package = current
                .packages
                .get(&setting.name)
                .ok_or("new-user captured package absent")?;
            let Some(code) = package.pkg.as_ref() else {
                state.installed = false;
                result.kernel_exclusions.push(setting.name.clone());
                result.states.insert(setting.name.clone(), state);
                continue;
            };
            let maybe_install = setting.flags & settings::FLAG_SYSTEM != 0
                && !disallowed.contains(&setting.name)
                && !package.is.hidden_until_installed;
            state.installed =
                maybe_install && installable.is_none_or(|names| names.contains(&setting.name));
            if policy.fix_system_apps_first_install_time && state.installed {
                state.first_install_time = policy.current_time_millis;
            }
            state.stopped = policy.stop_system_packages_by_default
                && setting.flags & settings::FLAG_SYSTEM != 0
                && !package.is.apex
                && !policy
                    .initial_non_stopped_system_packages
                    .contains(&setting.name);
            if state.stopped && !has_launcher(&setting.name, 0, 0xc0000)? {
                state.stopped = false;
            }
            state.uninstall_reason = if maybe_install && !state.installed {
                1
            } else {
                0
            };
            if state.installed {
                if setting.app_id >= 0 {
                    result.app_data.push(AppData {
                        volume_uuid: setting.volume_uuid.clone(),
                        package: setting.name.clone(),
                        user,
                        flags: 1 | if code.uses_sdk_libraries.is_empty() {
                            0
                        } else {
                            8
                        },
                        app_id: setting.app_id,
                        seinfo: package.seinfo.clone(),
                        target_sdk_version: code.target_sdk_version,
                    });
                }
            } else {
                result.kernel_exclusions.push(setting.name.clone());
            }
            result.states.insert(setting.name.clone(), state);
        }
        Ok(result)
    }
    pub fn apply_scan(&self, scan: &mut SigningScan) -> Result<(), String> {
        if self.states.len() != scan.settings.packages.len()
            || scan
                .settings
                .packages
                .iter()
                .any(|package| !self.states.contains_key(&package.name))
        {
            return Err("new-user package inventory differs".into());
        }
        let mut candidate = scan.clone();
        for (name, state) in &self.states {
            candidate.set_user_state(name, self.user, state.clone())?;
        }
        *scan = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        model,
        pkg::AndroidPackage,
        scan::CapturedUsers,
        settings::{Package, Settings},
    };
    use std::sync::Arc;

    fn fixture() -> (SigningScan, model::State) {
        let names = [
            "system",
            "allow-blocked",
            "disallowed",
            "hidden",
            "data",
            "no-launcher",
            "apex",
            "unloaded",
        ];
        let settings = Settings {
            packages: names
                .iter()
                .enumerate()
                .map(|(index, name)| Package {
                    name: (*name).into(),
                    app_id: 10100 + index as i32,
                    code_path: format!("/system/app/{name}"),
                    flags: if *name == "data" {
                        0
                    } else {
                        settings::FLAG_SYSTEM
                    },
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        scan.capture_user_states(
            names
                .iter()
                .map(|name| {
                    (
                        ((*name).into(), false),
                        CapturedUsers {
                            states: BTreeMap::from([
                                (0, UserState::default()),
                                (
                                    10,
                                    UserState {
                                        stopped: true,
                                        not_launched: true,
                                        first_install_time: 77,
                                        ..Default::default()
                                    },
                                ),
                            ]),
                            active_aliases: Default::default(),
                        },
                    )
                })
                .collect(),
        )
        .unwrap();
        let mut state = model::State::default();
        for setting in &scan.settings.packages {
            state.packages.insert(
                setting.name.clone(),
                model::PackageState {
                    name: setting.name.clone(),
                    app_id: setting.app_id,
                    seinfo: Some("platform:targetSdkVersion=36".into()),
                    is: model::StateFlags {
                        hidden_until_installed: setting.name == "hidden",
                        apex: setting.name == "apex",
                        ..Default::default()
                    },
                    pkg: (setting.name != "unloaded").then(|| {
                        Arc::new(AndroidPackage {
                            package_name: setting.name.clone(),
                            target_sdk_version: 36,
                            uses_sdk_libraries: if setting.name == "system" {
                                vec!["sdk".into()]
                            } else {
                                vec![]
                            },
                            ..Default::default()
                        })
                    }),
                    ..Default::default()
                },
            );
        }
        (scan, state)
    }
    #[test]
    fn creation_selection_storage_and_stopped_policy_use_original_user_zero_query() {
        let (scan, state) = fixture();
        let installable = BTreeSet::from([
            "system".into(),
            "disallowed".into(),
            "hidden".into(),
            "data".into(),
            "no-launcher".into(),
            "apex".into(),
        ]);
        let policy = CreationPolicy {
            current_time_millis: 12345,
            fix_system_apps_first_install_time: true,
            stop_system_packages_by_default: true,
            initial_non_stopped_system_packages: BTreeSet::from(["system".into()]),
        };
        let mut queries = Vec::new();
        let creation = Creation::prepare_state(
            &scan,
            &state,
            10,
            Some(&installable),
            &["disallowed".into()],
            &policy,
            |name, user, flags| {
                queries.push((name.to_string(), user, flags));
                Ok(name != "no-launcher")
            },
        )
        .unwrap();
        for name in ["system", "no-launcher", "apex"] {
            assert!(creation.states[name].installed);
            assert_eq!(creation.states[name].first_install_time, 12345);
            assert!(!creation.states[name].stopped);
        }
        for name in ["allow-blocked", "disallowed", "hidden", "data", "unloaded"] {
            assert!(!creation.states[name].installed);
            assert_eq!(creation.states[name].first_install_time, 77);
        }
        assert_eq!(creation.states["allow-blocked"].uninstall_reason, 1);
        assert_eq!(creation.states["disallowed"].uninstall_reason, 0);
        assert_eq!(creation.states["hidden"].uninstall_reason, 0);
        assert!(creation.states.values().all(|state| state.not_launched));
        assert!(
            queries
                .iter()
                .all(|(_, user, flags)| *user == 0 && *flags == 0xc0000)
        );
        assert!(!queries.iter().any(|(name, _, _)| ["system", "apex", "data", "unloaded"].contains(&name.as_str())));
        let args = creation
            .app_data
            .iter()
            .find(|args| args.package == "system")
            .unwrap();
        assert_eq!(args.flags, 9);
        assert_eq!(args.user, 10);
        assert_eq!(args.seinfo.as_deref(), Some("platform:targetSdkVersion=36"));
        assert_eq!(creation.app_data.len(), 3);
        let mut updated = scan.clone();
        creation.apply_scan(&mut updated).unwrap();
        assert_eq!(
            updated.scanned_user_states("system").unwrap()[&10],
            creation.states["system"]
        );
        assert_eq!(
            updated.scanned_user_states("system").unwrap()[&0],
            scan.scanned_user_states("system").unwrap()[&0]
        );
        assert_eq!(
            scan.scanned_user_states("system").unwrap()[&10].first_install_time,
            77
        );
    }
    #[test]
    fn launcher_owner_error_aborts_before_any_scan_change() {
        let (scan, state) = fixture();
        let before = scan.scanned_user_states("system").unwrap().clone();
        let policy = CreationPolicy {
            current_time_millis: 1,
            fix_system_apps_first_install_time: false,
            stop_system_packages_by_default: true,
            initial_non_stopped_system_packages: BTreeSet::new(),
        };
        assert!(
            Creation::prepare_state(&scan, &state, 11, None, &[], &policy, |_, _, _| Err(
                "live Computer revoked".into()
            ))
            .is_err()
        );
        assert_eq!(scan.scanned_user_states("system").unwrap(), &before);
    }
    #[test]
    fn user_removal_keeps_factory_object_but_drops_active_uid_alias() {
        let (original, _) = fixture();
        let mut settings = original.settings.clone();
        settings
            .disabled_system_packages
            .push(settings.packages[0].clone());
        let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        let mut users: BTreeMap<_, _> = settings
            .packages
            .iter()
            .map(|package| {
                (
                    (package.name.clone(), false),
                    CapturedUsers {
                        states: original.scanned_user_states(&package.name).unwrap().clone(),
                        active_aliases: Default::default(),
                    },
                )
            })
            .collect();
        users.insert(
            ("system".into(), true),
            CapturedUsers {
                states: original.scanned_user_states("system").unwrap().clone(),
                active_aliases: BTreeSet::from([0, 10]),
            },
        );
        scan.capture_user_states(users).unwrap();
        let factory = scan.disabled_user_states("system").unwrap().clone();
        scan.remove_package_user_state(10).unwrap();
        assert_eq!(scan.disabled_user_states("system").unwrap(), &factory);
        assert!(
            scan.scanned_user_states("system")
                .unwrap()
                .get(&10)
                .is_none()
        );
        assert!(scan.scanned_user_states("system").unwrap().contains_key(&0));
        assert!(scan.remove_package_user_state(-1).is_err());
    }
}
