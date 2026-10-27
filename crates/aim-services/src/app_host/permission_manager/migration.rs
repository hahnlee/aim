//! Migration from android-16.0.0_r1 AccessPersistence,
//! AppIdPermissionMigration, PermissionMigrationHelperImpl and PackageVersionMigration.
//! Copyright (C) 2023 The Android Open Source Project, Apache License 2.0.
use super::*;
use crate::package::settings;

impl PermissionManager {
    pub(super) fn migrate_user(
        &self,
        user: i32,
        path: &str,
        system: &AccessSystem,
    ) -> Result<AccessUser, Exception> {
        let fail = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let data = self.host.data()?;
        let configured = self.host.configured.get().expect("configured");
        let config = crate::package::system_config::SystemConfig::read(
            self.host.root()?,
            &configured.properties,
        );
        let root = crate::package::resilient(
            &data.join("system/packages.xml"),
            &data.join("system/packages-backup.xml"),
            |root| Ok(root.clone()),
        )
        .map_err(fail)?
        .unwrap_or_else(|| aim_android_xml::read(b"<packages/>").expect("empty settings"));
        let runtime = crate::package::atomic(
            &data.join(format!(
                "misc_de/{user}/apexdata/com.android.permission/runtime-permissions.xml"
            )),
            permissions::RuntimePermissions::parse,
        )
        .map_err(fail)?;
        let state = crate::package::State {
            settings: settings::Settings::parse_with_config(&root, &config).map_err(fail)?,
            list: Vec::new(),
            access: None,
            users: vec![(
                user as u32,
                crate::package::User {
                    restrictions: Default::default(),
                    access: None,
                    runtime_permissions: runtime,
                },
            )],
        };
        // Use the same Settings restore as the original helper's input, including
        // install states and the pre-APEX runtime-permissions.xml fallback.
        let legacy = crate::package::owner::legacy_permissions::restore::read_document(
            data, &state, &config, &root,
        )
        .map_err(fail)?;
        let version = match legacy.metadata.users[&user].version.unwrap_or(0) {
            -1 => 0,
            0 => return Ok(AccessUser::default()),
            version if version < -1 => return Ok(AccessUser::default()),
            version => version,
        };
        let version = package_version(version, legacy_app_op_version(data));
        let mut migrated = AccessUser::default();
        // getLegacyPermissionStates ignores empty rows and packages with shared
        // UIDs; the shared owner supplies their one map. The legacy fingerprint
        // belongs to Settings and is not copied by AppIdPermissionMigration.
        let mut app_ids = BTreeMap::new();
        for package in &state.settings.packages {
            if !package.shared_user {
                let rows = legacy.packages[&(package.name.clone(), false)]
                    .permissions(user)
                    .map_err(fail)?;
                if !rows.is_empty() {
                    app_ids.insert(package.app_id, rows);
                }
            }
        }
        let owners =
            crate::package::owner::shared_users::Bootstrap::restore(&config, &state.settings)
                .map_err(|e| fail(format!("legacy shared users: {e:?}")))?;
        for (name, owner) in &owners.shared_users {
            let rows = legacy.shared_users[name].permissions(user).map_err(fail)?;
            if !rows.is_empty() {
                app_ids.insert(owner.app_id, rows);
            }
        }
        for (app_id, rows) in app_ids {
            let packages: Vec<_> = state
                .settings
                .packages
                .iter()
                .filter(|p| p.app_id == app_id)
                .collect();
            if packages.is_empty() {
                continue;
            }
            let flags = rows
                .iter()
                .filter_map(|row| {
                    let name = row.name.as_deref()?;
                    let permission = system.permissions.iter().find(|p| p.name == name)?;
                    let grant = match permission.protection_level & PROTECTION_MASK_BASE {
                        0 => {
                            if row.granted {
                                INSTALL_GRANTED
                            } else {
                                INSTALL_REVOKED
                            }
                        }
                        PROTECTION_SIGNATURE | 4 if row.granted => {
                            PROTECTION_GRANTED
                                | if permission.protection_level
                                    & (PROTECTION_FLAG_DEVELOPMENT | PROTECTION_FLAG_ROLE)
                                    != 0
                                {
                                    RUNTIME_GRANTED
                                } else {
                                    0
                                }
                        }
                        PROTECTION_DANGEROUS if row.granted => RUNTIME_GRANTED,
                        _ => 0,
                    };
                    Some((
                        name.to_owned(),
                        update_flags(0, grant, row.flags, row.flags),
                    ))
                })
                .collect();
            // Migration keeps zero flags and even an empty app-ID map, unlike
            // the subsequent setPermissionFlags mutation's putWithDefault.
            migrated.app_id_permissions.push((app_id, flags));
            migrated
                .package_versions
                .extend(packages.into_iter().map(|p| (p.name.clone(), version)));
        }
        migrated.package_versions = info::array_order(migrated.package_versions, |(name, _)| name);
        self.write_user(path, &migrated)?;
        Ok(migrated)
    }

    /// AccessPersistence.readSystemState: migrate only when modern state is absent,
    /// then serialize through the same owner as later permission mutations.
    pub(super) fn migrate_system(&self) -> Result<AccessSystem, Exception> {
        let data = self.host.data()?.join("system");
        let system = crate::package::resilient(
            &data.join("packages.xml"),
            &data.join("packages-backup.xml"),
            legacy_system,
        )
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
        .unwrap_or_default();
        self.write_system(&system)?;
        Ok(system)
    }
}

/// AppOpMigrationHelperImpl's version input to PackageVersionMigration.
fn legacy_app_op_version(data: &std::path::Path) -> i32 {
    let path = data.join("system/appops.xml");
    let backup = path.with_extension("xml.bak");
    let path = if backup.exists() { backup } else { path };
    let read = || -> Result<i32, String> {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let root = aim_android_xml::read(&bytes)?;
        Ok(match root.int("v")?.unwrap_or(-1) {
            -2 => -1,
            -1 => 0,
            version => version,
        })
    };
    match read() {
        Ok(version) => version,
        Err(e) => {
            // LegacyAppOpStateParser catches failed reads and starts empty.
            eprintln!("app host: permissionmgr: No legacy app ops: {e}");
            -1
        }
    }
}

/// PackageVersionMigration.getVersion, called only with legacy permission state.
fn package_version(permission: i32, app_op: i32) -> i32 {
    match (permission, app_op) {
        (11.., 3..) => 15,
        (10.., 3..) => 14,
        (10.., 1..) => 13,
        (9, 1..) => 12,
        (8, 1..) => 11,
        (7, 1..) => 10,
        (6, 1..) => 9,
        (5, 1..) => 8,
        (4, 1..) => 7,
        (3, 1..) => 6,
        (2, 1..) => 5,
        (1, 1..) => 4,
        (0.., 1..) => 3,
        (0.., 0..) => 2,
        _ => 0,
    }
}

/// LegacyPermissionSettings' two maps, as PermissionMigrationHelperImpl passes
/// them to AppIdPermissionMigration. Other Settings records have their own owners.
fn legacy_system(root: &aim_android_xml::Element) -> Result<AccessSystem, String> {
    let mut legacy = settings::Settings::default();
    for section in root.children() {
        let out = match section.name.as_str() {
            "permissions" => &mut legacy.permissions,
            "permission-trees" => &mut legacy.permission_trees,
            _ => continue,
        };
        for item in section.children() {
            settings::read_permission(out, item);
        }
    }
    let migrate = |definitions: Vec<settings::Permission>| {
        let mut definitions = definitions
            .into_iter()
            .map(|p| Permission {
                name: p.name,
                package: p.package,
                protection_level: p.protection_level,
                dynamic: p.dynamic,
            })
            .collect::<Vec<_>>();
        // MutableIndexedMap / ArrayMap order, stable within hash collisions.
        definitions.sort_by_key(|p| info::java_hash(&p.name));
        definitions
    };
    Ok(AccessSystem {
        permissions: migrate(legacy.permissions),
        permission_trees: migrate(legacy.permission_trees),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{BOOT_USER, PersistedAccess};
    use super::*;

    // LegacyPermission.read / fixProtectionLevel, then migratePermissions:
    // replacement in ArrayMap order, normalized protection and dynamic metadata.
    const PACKAGES: &[u8] = br#"<packages>
        <package name="unrelated"><permissions><item name="nested" package="ignored"/></permissions></package>
        <permissions>
            <item name="z" package="android" protection="3" icon="42" label="ignored"/>
            <item name="Aa" package="old" protection="1"/>
            <item name="BB" package="owner" type="dynamic" label=""/>
            <item name="Aa" package="owner" protection="4098" type="dynamic" icon="-249346713" label="&lt;dynamic&gt; &amp; label"/>
            <item name="a" package="owner" protection="32768"/>
            <item name="default" package="owner" protection="invalid" type="dynamic" icon="invalid"/>
            <item name="missing-package"/>
            <item package="missing-name"/>
            <unknown name="ignored" package="owner"><item name="nested" package="ignored"/></unknown>
        </permissions>
        <permission-trees>
            <item name="tree" package="old" protection="2"/>
            <item name="tree" package="owner" protection="1"/>
        </permission-trees>
    </packages>"#;

    #[test]
    fn legacy_system_migrates_persists_and_reloads() {
        for binary in [false, true] {
            let fixture = PersistedAccess::new();
            fixture.write(USER0_ACCESS, BOOT_USER);
            let user_before = std::fs::read(fixture.data.join(USER0_ACCESS)).unwrap();
            if binary {
                fixture.write("system/packages.xml", PACKAGES);
            } else {
                std::fs::create_dir_all(fixture.data.join("system")).unwrap();
                std::fs::write(fixture.data.join("system/packages.xml"), PACKAGES).unwrap();
            }
            let manager = PermissionManager::new(fixture.host.clone());
            let state = manager.system().unwrap();
            let p = |name: &str| state.permissions.iter().find(|p| p.name == name).unwrap();
            assert_eq!(state.permissions.len(), 5);
            assert_eq!(p("z").protection_level, 0x12);
            assert_eq!(p("z").dynamic, None);
            assert_eq!(p("a").protection_level, 0);
            assert_eq!(p("Aa").package, "owner");
            assert_eq!(p("Aa").protection_level, 0x1002);
            assert_eq!(
                p("Aa").dynamic,
                Some((-249346713, Some("<dynamic> & label".into())))
            );
            assert_eq!(p("BB").dynamic, Some((0, Some(String::new()))));
            assert_eq!(p("default").protection_level, 0);
            assert_eq!(p("default").dynamic, Some((0, None)));
            assert_eq!(
                state.permission_trees,
                [Permission {
                    name: "tree".into(),
                    package: "owner".into(),
                    protection_level: 1,
                    dynamic: None,
                }]
            );
            let hashes = state
                .permissions
                .iter()
                .map(|p| info::java_hash(&p.name))
                .collect::<Vec<_>>();
            assert!(hashes.windows(2).all(|pair| pair[0] <= pair[1]));
            let collision = state
                .permissions
                .iter()
                .filter(|p| p.name == "Aa" || p.name == "BB")
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(collision, ["Aa", "BB"]);
            let path = fixture.data.join(SYSTEM_ACCESS);
            let persisted = std::fs::read(&path).unwrap();
            assert_eq!(&persisted[..4], b"ABX\0");
            assert_eq!(
                AccessSystem::parse(&aim_android_xml::read(&persisted).unwrap()).unwrap(),
                *state
            );
            assert_eq!(
                std::fs::read(path.with_extension("abx.reservecopy")).unwrap(),
                persisted
            );
            assert_eq!(*manager.access_writes.lock().unwrap(), [SYSTEM_ACCESS]);
            assert_eq!(
                std::fs::read(fixture.data.join(USER0_ACCESS)).unwrap(),
                user_before
            );
            manager.user0().unwrap();
            // A new owner must read modern state without consulting legacy inputs.
            std::fs::write(fixture.data.join("system/packages.xml"), b"broken").unwrap();
            let reloaded = PermissionManager::new(fixture.host.clone());
            assert_eq!(*reloaded.system().unwrap(), *state);
            assert!(reloaded.access_writes.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn legacy_settings_backup_and_reserve_are_migration_inputs() {
        for backup in [false, true] {
            let fixture = PersistedAccess::new();
            fixture.write(
                if backup {
                    "system/packages-backup.xml"
                } else {
                    "system/packages.xml.reservecopy"
                },
                PACKAGES,
            );
            std::fs::write(fixture.data.join("system/packages.xml"), b"broken").unwrap();
            let manager = PermissionManager::new(fixture.host.clone());
            assert_eq!(manager.system().unwrap().permissions.len(), 5);
        }
    }

    #[test]
    fn first_boot_persists_empty_registry_and_failed_write_does_not_publish() {
        let fixture = PersistedAccess::new();
        let manager = PermissionManager::new(fixture.host.clone());
        let path = fixture.data.join(SYSTEM_ACCESS);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir(path.with_extension("abx.new")).unwrap();
        assert_eq!(manager.system().unwrap_err().code, EX_ILLEGAL_STATE);
        assert!(manager.owner.lock().unwrap().system.is_none());
        assert!(!path.exists());
        std::fs::remove_dir(path.with_extension("abx.new")).unwrap();
        assert_eq!(*manager.system().unwrap(), AccessSystem::default());
        assert!(path.is_file());
    }

    const USER_PACKAGES: &[u8] = br#"<packages>
        <version sdkVersion="36" databaseVersion="3"/>
        <package name="app" codePath="/data/app/app" userId="10100">
            <perms><item name="normal" granted="true" flags="0"/></perms>
        </package>
        <shared-user name="group" userId="10101"/>
        <package name="member.one" codePath="/data/app/one" sharedUserId="10101"/>
        <package name="member.two" codePath="/data/app/two" sharedUserId="10101"/>
        <package name="empty" codePath="/data/app/empty" userId="10102"/>
        <package name="unknown.only" codePath="/data/app/unknown" userId="10103"/>
    </packages>"#;

    const USER_SYSTEM: &[u8] = br#"<access><permissions>
        <permission name="camera" packageName="android" protectionLevel="1" type="0"/>
        <permission name="contacts" packageName="android" protectionLevel="1" type="0"/>
        <permission name="normal" packageName="android" protectionLevel="0" type="0"/>
        <permission name="signature" packageName="android" protectionLevel="2" type="0"/>
        <permission name="development" packageName="android" protectionLevel="22" type="0"/>
        <permission name="role" packageName="android" protectionLevel="4000004" type="0"/>
    </permissions></access>"#;

    pub(crate) const RUNTIME: &[u8] = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
    <runtime-permissions version="11" fingerprint="partition?pc_version=123">
        <package name="app">
            <permission name="camera" granted="false" flags="0"/>
            <permission name="camera" granted="true" flags="3"/>
            <permission name="contacts" granted="false" flags="14"/>
            <permission name="signature" granted="false" flags="0"/>
            <permission name="development" granted="true" flags="0"/>
            <permission name="role" granted="true" flags="8000"/>
            <permission name="unknown" granted="true" flags="1"/>
        </package>
        <package name="member.one"><permission name="contacts" granted="true" flags="0"/></package>
        <shared-user name="group"><permission name="camera" granted="false" flags="40"/></shared-user>
        <package name="empty"/>
        <package name="unknown.only"><permission name="unknown" granted="true" flags="0"/></package>
        <package name="gone"><permission name="camera" granted="true" flags="0"/></package>
        <shared-user name="gone"><permission name="camera" granted="true" flags="0"/></shared-user>
    </runtime-permissions>"#;

    pub(crate) fn user_fixture() -> PersistedAccess {
        let fixture = PersistedAccess::new();
        fixture.write("system/packages.xml", USER_PACKAGES);
        fixture.write(SYSTEM_ACCESS, USER_SYSTEM);
        fixture
    }

    pub(crate) fn write_runtime(
        fixture: &PersistedAccess,
        user: i32,
        bytes: &[u8],
    ) -> std::path::PathBuf {
        let path = fixture.data.join(format!(
            "misc_de/{user}/apexdata/com.android.permission/runtime-permissions.xml"
        ));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn legacy_user_grants_flags_shared_uid_persist_and_reload() {
        let fixture = user_fixture();
        let legacy = write_runtime(&fixture, 0, RUNTIME);
        std::fs::write(fixture.data.join("system/appops.xml"), b"<app-ops v='3'/>").unwrap();
        let manager = PermissionManager::new(fixture.host.clone());
        let user = manager.user0().unwrap();
        assert_eq!(
            user.permission_flags(10100, "camera"),
            RUNTIME_GRANTED | USER_SET | USER_FIXED
        );
        assert_eq!(
            user.permission_flags(10100, "contacts"),
            POLICY_FIXED | SYSTEM_FIXED
        );
        assert_eq!(user.permission_flags(10100, "normal"), INSTALL_GRANTED);
        assert_eq!(
            user.permission_flags(10100, "development"),
            PROTECTION_GRANTED | RUNTIME_GRANTED
        );
        assert_eq!(
            user.permission_flags(10100, "role"),
            PROTECTION_GRANTED | RUNTIME_GRANTED | ROLE
        );
        assert!(
            user.app_id_permissions[0]
                .1
                .contains(&("signature".into(), 0))
        );
        assert_eq!(user.permission_flags(10101, "camera"), IMPLICIT);
        assert_eq!(user.permission_flags(10101, "contacts"), 0);
        assert_eq!(user.app_id_permissions.len(), 3);
        assert_eq!(user.app_id_permissions[2], (10103, vec![]));
        assert_eq!(
            user.package_versions,
            info::array_order(
                vec![
                    ("app".into(), 15),
                    ("member.one".into(), 15),
                    ("member.two".into(), 15),
                    ("unknown.only".into(), 15),
                ],
                |(name, _): &(String, i32)| name
            )
        );
        assert_eq!(user.default_permission_grant_fingerprint, None);
        assert_eq!(std::fs::read(&legacy).unwrap(), RUNTIME);
        let path = fixture.data.join(USER0_ACCESS);
        let persisted = std::fs::read(&path).unwrap();
        assert_eq!(&persisted[..4], b"ABX\0");
        assert_eq!(
            AccessUser::parse(&aim_android_xml::read(&persisted).unwrap()).unwrap(),
            *user
        );
        assert_eq!(
            std::fs::read(path.with_extension("abx.reservecopy")).unwrap(),
            persisted
        );
        assert_eq!(*manager.access_writes.lock().unwrap(), [USER0_ACCESS]);
        std::fs::write(legacy, b"broken").unwrap();
        std::fs::write(fixture.data.join("system/packages.xml"), b"broken").unwrap();
        let reloaded = PermissionManager::new(fixture.host.clone());
        assert_eq!(*reloaded.user0().unwrap(), *user);
        assert!(reloaded.access_writes.lock().unwrap().is_empty());
    }

    #[test]
    fn legacy_user_versions_and_fingerprint_follow_original_helpers() {
        for (version, app_ops, expected, writes) in [
            (None, None, 0, true),
            (Some(-1), None, 0, true),
            (Some(0), Some(3), 0, false),
            (Some(-2), None, 0, false),
            (Some(11), None, 0, true),
            (None, Some(1), 3, true),
            (Some(7), Some(1), 10, true),
            (Some(10), Some(3), 14, true),
            (Some(11), Some(3), 15, true),
            (Some(11), Some(0), 2, true),
        ] {
            let fixture = user_fixture();
            let version = version
                .map(|v| format!(" version='{v}'"))
                .unwrap_or_default();
            let xml = format!(
                "<runtime-permissions{version} fingerprint='legacy'><package name='app'><permission name='camera' granted='true' flags='0'/></package></runtime-permissions>"
            );
            write_runtime(&fixture, 0, xml.as_bytes());
            if let Some(v) = app_ops {
                std::fs::write(
                    fixture.data.join("system/appops.xml"),
                    format!("<app-ops v='{v}'/>"),
                )
                .unwrap();
            }
            let manager = PermissionManager::new(fixture.host.clone());
            let user = manager.user0().unwrap();
            assert_eq!(
                user.package_versions,
                if writes {
                    vec![("app".into(), expected)]
                } else {
                    vec![]
                }
            );
            assert_eq!(user.default_permission_grant_fingerprint, None);
            assert_eq!(fixture.data.join(USER0_ACCESS).exists(), writes);
        }
    }

    #[test]
    fn missing_legacy_user_leaves_empty_state_unwritten_but_old_file_migrates() {
        let fixture = user_fixture();
        let manager = PermissionManager::new(fixture.host.clone());
        assert_eq!(*manager.user0().unwrap(), AccessUser::default());
        assert!(!fixture.data.join(USER0_ACCESS).exists());
        assert!(manager.access_writes.lock().unwrap().is_empty());
        let old = fixture.data.join("system/users/0/runtime-permissions.xml");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(old, b"<runtime-permissions fingerprint='old'><package name='app'><item name='camera' granted='false' flags='2'/></package></runtime-permissions>").unwrap();
        let migrated = PermissionManager::new(fixture.host.clone())
            .user0()
            .unwrap();
        assert_eq!(migrated.permission_flags(10100, "camera"), USER_FIXED);
        assert_eq!(migrated.package_versions, [("app".into(), 0)]);
        assert_eq!(migrated.default_permission_grant_fingerprint, None);
        assert!(fixture.data.join(USER0_ACCESS).is_file());
    }

    #[test]
    fn first_write_after_missing_legacy_creates_user_state() {
        let fixture = user_fixture();
        let manager = PermissionManager::new(fixture.host.clone());
        let mut user = (*manager.user0().unwrap()).clone();
        user.set_permission_flags(10100, "camera", RUNTIME_GRANTED);
        manager.write_user0(&user).unwrap();
        let reloaded = PermissionManager::new(fixture.host.clone());
        assert_eq!(*reloaded.user0().unwrap(), user);
    }

    #[test]
    fn legacy_user_atomic_recovery_and_failed_write_does_not_publish() {
        let fixture = user_fixture();
        let legacy = write_runtime(&fixture, 0, b"broken");
        std::fs::write(legacy.with_extension("xml.bak"), RUNTIME).unwrap();
        let manager = PermissionManager::new(fixture.host.clone());
        let path = fixture.data.join(USER0_ACCESS);
        std::fs::create_dir(path.with_extension("abx.new")).unwrap();
        assert_eq!(manager.user0().unwrap_err().code, EX_ILLEGAL_STATE);
        assert!(manager.owner.lock().unwrap().user0.is_none());
        assert!(!path.exists());
        std::fs::remove_dir(path.with_extension("abx.new")).unwrap();
        std::fs::write(legacy.with_extension("xml.bak"), b"broken").unwrap();
        std::fs::write(legacy.with_extension("xml.reservecopy"), RUNTIME).unwrap();
        assert_eq!(
            manager.user0().unwrap().permission_flags(10100, "camera"),
            RUNTIME_GRANTED | USER_SET | USER_FIXED
        );
    }
}
