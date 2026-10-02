//! New PackageSetting construction, ported from android-16.0.0_r1
//! Settings.createNewSetting and PackageSetting/PackageKeySetData constructors.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Identity, Uid};
use crate::package::{restrictions::UserState, settings};
use std::collections::BTreeMap;

/// Inputs supplied by the code, ABI, scan flag and domain-verification owners.
/// This is the new-package branch, without original/disabled-setting adoption.
#[derive(Clone, Debug)]
pub struct SettingMetadata {
    pub code_path: String,
    pub legacy_native_library_path: Option<String>,
    pub primary_cpu_abi: Option<String>,
    pub secondary_cpu_abi: Option<String>,
    pub version_code: i64,
    pub flags: i32,
    pub private_flags: i32,
    pub last_modified_time: i64,
    pub uses_sdk_libraries: Vec<settings::UsesSdkLibrary>,
    pub uses_static_libraries: Vec<(String, i64)>,
    pub mime_groups: Vec<String>,
    pub domain_set_id: [u8; 16],
    pub target_sdk_version: i32,
    pub restrict_update_hash: Option<Vec<u8>>,
}

/// UserManager.getUsers(excludePartial=true, excludeDying=false,
/// excludePreCreated=false), with DISALLOW_DEBUGGING_FEATURES per user.
#[derive(Clone, Copy, Debug)]
pub struct User {
    pub id: i32,
    pub pre_created: bool,
    pub adb_install_disallowed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct UserPolicy<'a> {
    /// None is the original constructor's null installUser; -1 is USER_ALL.
    pub install_user: Option<i32>,
    /// None represents UserManager not initialized, rather than an empty list.
    pub users: Option<&'a [User]>,
    pub allow_install: bool,
    pub instant_app: bool,
    pub virtual_preload: bool,
    pub stopped_system_app: bool,
}

/// A candidate, before signer authorization, scan enrichment and publication.
#[derive(Clone, Debug, PartialEq)]
pub struct NewSetting {
    pub package: settings::Package,
    /// Only explicitly created states; an absent entry uses UserState::default.
    pub users: BTreeMap<i32, UserState>,
}

impl NewSetting {
    pub(super) fn new(
        identity: &Identity,
        uid: &Uid,
        m: SettingMetadata,
        p: UserPolicy<'_>,
    ) -> Self {
        let mut users = BTreeMap::new();
        let system = m.flags & settings::FLAG_SYSTEM != 0;
        if !system {
            if p.allow_install
                && let Some(all) = p.users
            {
                for user in all {
                    let installed = p.install_user.is_none()
                        || (p.install_user == Some(-1)
                            && !user.adb_install_disallowed
                            && !user.pre_created)
                        || p.install_user == Some(user.id);
                    users.insert(
                        user.id,
                        UserState {
                            installed,
                            stopped: true,
                            not_launched: true,
                            instant_app: p.instant_app,
                            virtual_preload: p.virtual_preload,
                            ..UserState::default()
                        },
                    );
                }
            }
        } else if p.stopped_system_app {
            users.insert(
                p.install_user.unwrap_or(0),
                UserState {
                    stopped: true,
                    ..UserState::default()
                },
            );
        }
        let hex = m
            .domain_set_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let domain_set_id = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        Self {
            package: settings::Package {
                name: identity.internal_name.clone(),
                real_name: identity.real_name.clone(),
                code_path: m.code_path,
                legacy_native_library_path: m.legacy_native_library_path,
                primary_cpu_abi: m.primary_cpu_abi,
                secondary_cpu_abi: m.secondary_cpu_abi,
                version_code: m.version_code,
                flags: m.flags,
                private_flags: m.private_flags,
                last_modified_time: m.last_modified_time,
                uses_sdk_libraries: m.uses_sdk_libraries,
                uses_static_libraries: m.uses_static_libraries,
                mime_groups: m
                    .mime_groups
                    .into_iter()
                    .map(|name| (name, Vec::new()))
                    .collect(),
                domain_set_id: Some(domain_set_id),
                target_sdk_version: m.target_sdk_version,
                restrict_update_hash: m.restrict_update_hash,
                app_id: uid.app_id,
                shared_user: uid.shared_user.is_some(),
                scanned_as_stopped_system_app: system && p.stopped_system_app,
                category_hint: -1,
                key_set_data: settings::KeySetData {
                    proper_signing_key_set: -1,
                    ..Default::default()
                },
                // The constructor starts at progress 0, so isLoading() is true.
                loading: true,
                ..settings::Package::default()
            },
            users,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(flags: i32) -> SettingMetadata {
        SettingMetadata {
            code_path: "/system/app/fixture.apk".into(),
            legacy_native_library_path: None,
            primary_cpu_abi: None,
            secondary_cpu_abi: None,
            version_code: 1,
            flags,
            private_flags: 0,
            last_modified_time: 0,
            uses_sdk_libraries: Vec::new(),
            uses_static_libraries: Vec::new(),
            mime_groups: Vec::new(),
            domain_set_id: [0; 16],
            target_sdk_version: 36,
            restrict_update_hash: None,
        }
    }
    #[test]
    fn install_targets_keep_precreated_and_restricted_user_semantics() {
        let identity = Identity {
            manifest_name: "fixture".into(),
            internal_name: "fixture".into(),
            real_name: None,
        };
        let uid = Uid {
            app_id: 10000,
            shared_user: None,
        };
        let users = [
            User {
                id: 0,
                pre_created: false,
                adb_install_disallowed: false,
            },
            User {
                id: 10,
                pre_created: true,
                adb_install_disallowed: false,
            },
            User {
                id: 11,
                pre_created: false,
                adb_install_disallowed: true,
            },
            User {
                id: 12,
                pre_created: false,
                adb_install_disallowed: false,
            },
        ];
        for (target, installed) in [
            (None, vec![0, 10, 11, 12]),
            (Some(-1), vec![0, 12]),
            (Some(10), vec![10]),
            (Some(11), vec![11]),
            (Some(99), vec![]),
        ] {
            let policy = UserPolicy {
                install_user: target,
                users: Some(&users),
                allow_install: true,
                instant_app: true,
                virtual_preload: true,
                stopped_system_app: true,
            };
            let setting = NewSetting::new(&identity, &uid, metadata(0), policy);
            assert!(!setting.package.scanned_as_stopped_system_app);
            assert_eq!(setting.users.len(), 4);
            for user in users {
                let state = &setting.users[&user.id];
                assert_eq!(
                    state,
                    &UserState {
                        installed: installed.contains(&user.id),
                        stopped: true,
                        not_launched: true,
                        instant_app: true,
                        virtual_preload: true,
                        ..Default::default()
                    }
                );
            }
            for policy in [
                UserPolicy {
                    allow_install: false,
                    ..policy
                },
                UserPolicy {
                    users: None,
                    ..policy
                },
            ] {
                assert!(
                    NewSetting::new(&identity, &uid, metadata(0), policy)
                        .users
                        .is_empty()
                );
            }
            let setting = NewSetting::new(&identity, &uid, metadata(1), policy);
            assert_eq!(setting.users.len(), 1);
            assert_eq!(
                setting.users[&target.unwrap_or(0)],
                UserState {
                    stopped: true,
                    ..Default::default()
                }
            );
            assert!(setting.package.scanned_as_stopped_system_app);
            assert!(
                NewSetting::new(
                    &identity,
                    &uid,
                    metadata(1),
                    UserPolicy {
                        stopped_system_app: false,
                        ..policy
                    }
                )
                .users
                .is_empty()
            );
        }
    }
}
