//! ScanPackageUtils scan metadata and PackageStateUtils timestamp policy,
//! ported from android-16.0.0_r1. Copyright (C) The Android Open Source
//! Project, Apache License 2.0. ABI and side effects have separate owners.
use crate::package::{
    pkg::{AndroidPackage, booleans},
    restrictions::UserState,
    settings,
};
use std::collections::BTreeMap;

/// Original scan clock, verified-code timestamp and install target inputs.
/// Zero current time is boot's file-time policy; -1 targets existing users.
#[derive(Clone, Copy, Debug)]
pub struct ScanTime {
    pub current_time: i64,
    pub file_time: i64,
    pub user_id: i32,
    pub update_time: bool,
}

/// ScanPackageUtils currentTime, userId and SCAN_UPDATE_TIME inputs.
#[derive(Clone, Copy, Debug)]
pub struct ScanClock {
    pub current_time: i64,
    pub user_id: i32,
    pub update_time: bool,
}

pub(super) fn apply(
    package: &mut settings::Package,
    parsed: &AndroidPackage,
    users: &mut BTreeMap<i32, UserState>,
    time: ScanTime,
    system_directory: bool,
) {
    // The original uses Long.MAX_VALUE as its sentinel, excluding zero.
    let earliest = users
        .values()
        .map(|s| s.first_install_time)
        .filter(|t| *t != 0)
        .min()
        .filter(|t| *t != i64::MAX)
        .unwrap_or(0);
    if time.current_time != 0 {
        if earliest == 0 {
            set_first(users, time.user_id, time.current_time);
            package.last_update_time = time.current_time;
        } else if time.update_time {
            package.last_update_time = time.current_time;
        }
    } else if earliest == 0 {
        set_first(users, time.user_id, time.file_time);
        package.last_update_time = time.file_time;
    } else if system_directory && time.file_time != package.last_modified_time {
        package.last_update_time = time.file_time;
    }
    package.last_modified_time = time.file_time;
    package.version_code =
        (i64::from(parsed.version_code_major) << 32) | i64::from(parsed.version_code as u32);
    package.volume_uuid.clone_from(&parsed.volume_uuid);
    package.debuggable = parsed.is(booleans::DEBUGGABLE);
    package.base_revision_code = parsed.base_revision_code;
    if package.flags & settings::FLAG_SYSTEM != 0 {
        package.install_source.is_orphaned = true;
    }
}

fn set_first(users: &mut BTreeMap<i32, UserState>, target: i32, time: i64) {
    if target == -1 {
        for state in users.values_mut() {
            state.first_install_time = time;
        }
    } else {
        users.entry(target).or_default().first_install_time = time;
    }
}

pub(super) fn application(
    package: &mut settings::Package,
    parsed: &mut AndroidPackage,
    factory_test: bool,
    updated_system_app: bool,
) {
    parsed.booleans &= !booleans::FACTORY_TEST;
    if factory_test
        && parsed
            .requested_permissions
            .iter()
            .any(|p| p == "android.permission.FACTORY_TEST")
    {
        parsed.booleans |= booleans::FACTORY_TEST;
    }
    (package.flags, package.private_flags) = super::application_flags(parsed, updated_system_app);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_flags_replace_saved_bits_and_gate_factory_mode_by_requested_permission() {
        for factory in [false, true] {
            for permission in [false, true] {
                for old_factory in [false, true] {
                    for updated in [false, true] {
                        let mut pkg = AndroidPackage {
                            target_sdk_version: 36,
                            booleans: booleans::DEBUGGABLE
                                | booleans::SYSTEM
                                | if old_factory {
                                    booleans::FACTORY_TEST
                                } else {
                                    0
                                },
                            requested_permissions: if permission {
                                vec!["android.permission.FACTORY_TEST".into()]
                            } else {
                                vec!["android.permission.FACTORY_TEST.other".into()]
                            },
                            ..Default::default()
                        };
                        let mut setting = settings::Package {
                            flags: -1,
                            private_flags: -1,
                            ..Default::default()
                        };
                        application(&mut setting, &mut pkg, factory, updated);
                        assert_eq!(pkg.is(booleans::FACTORY_TEST), factory && permission);
                        assert_eq!(setting.flags & 16 != 0, factory && permission);
                        assert_eq!(setting.flags & 128 != 0, updated);
                        assert_eq!(setting.flags & 3, 3);
                        assert_eq!(setting.private_flags, 0);
                    }
                }
            }
        }
    }
    #[test]
    fn scan_times_distinguish_initial_install_explicit_updates_and_changed_system_code() {
        let parsed = AndroidPackage {
            version_code: -1,
            version_code_major: 2,
            base_revision_code: 3,
            booleans: booleans::DEBUGGABLE,
            ..Default::default()
        };
        for (first, current, system, update, file, expected_first, expected_update) in [
            (0, 100, false, false, 30, 100, 100),
            (0, 0, false, false, 30, 30, 30),
            (10, 100, false, false, 30, 10, 20),
            (10, 100, false, true, 30, 10, 100),
            (10, 0, false, true, 30, 10, 20),
            (10, 0, true, false, 30, 10, 30),
            (10, 0, true, false, 25, 10, 20),
            (-5, 100, false, false, 30, -5, 20),
            (i64::MAX, 100, false, false, 30, 100, 100),
        ] {
            let mut package = settings::Package {
                flags: settings::FLAG_SYSTEM,
                last_modified_time: 25,
                last_update_time: 20,
                volume_uuid: Some("old-volume".into()),
                ..Default::default()
            };
            let mut users = BTreeMap::from([(
                0,
                UserState {
                    first_install_time: first,
                    installed: false,
                    stopped: true,
                    ..Default::default()
                },
            )]);
            apply(
                &mut package,
                &parsed,
                &mut users,
                ScanTime {
                    current_time: current,
                    file_time: file,
                    user_id: 0,
                    update_time: update,
                },
                system,
            );
            assert_eq!(users[&0].first_install_time, expected_first);
            assert!(!users[&0].installed);
            assert!(users[&0].stopped);
            assert_eq!(package.last_update_time, expected_update);
            assert_eq!(package.last_modified_time, file);
            assert_eq!(package.version_code, 0x2ffffffff);
            assert_eq!(package.volume_uuid, None);
            assert_eq!(package.base_revision_code, 3);
            assert!(package.debuggable && package.install_source.is_orphaned);
        }
    }

    #[test]
    fn user_all_only_changes_existing_explicit_states_and_earliest_ignores_zero() {
        let mut package = settings::Package {
            last_update_time: 20,
            ..Default::default()
        };
        let parsed = AndroidPackage::default();
        let time = ScanTime {
            current_time: 100,
            file_time: 30,
            user_id: -1,
            update_time: false,
        };
        let mut users = BTreeMap::new();
        apply(&mut package, &parsed, &mut users, time, false);
        assert!(users.is_empty());
        assert_eq!(package.last_update_time, 100);
        users.insert(0, UserState::default());
        users.insert(10, UserState::default());
        apply(&mut package, &parsed, &mut users, time, false);
        assert!(users.values().all(|s| s.first_install_time == 100));
        users.get_mut(&0).unwrap().first_install_time = 0;
        package.last_update_time = 20;
        apply(&mut package, &parsed, &mut users, time, false);
        assert_eq!(users[&0].first_install_time, 0);
        assert_eq!(package.last_update_time, 20);
        assert!(!package.install_source.is_orphaned);
    }
}
