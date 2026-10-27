//! `IPackageBootConfigurationLeaf` for the app host: the platform's side of
//! the native package boot's configuration, answered as
//! `dev.aim.server.PackageBootConfigurationLeaf` answers it in system_server
//! (`android-16.0.0_r1`): system_server's resource configuration
//! (`ResourcesImpl.updateConfiguration`'s `ResTable_config` fields) and
//! density, the factory test mode the boot was asked for, two aconfig flags,
//! the owner and mode the original creators give a stage directory and
//! system_server's AtomicFiles, required live properties,
//! `PackageManagerServiceUtils.preparePackageParserCache`, and the user
//! inventories UserManagerService gives the boot.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, Exception, Parcel,
};
use aim_service_aidl::dev_aim_server_ipackagebootconfigurationleaf as leaf;
use aim_storage::guest_inode::GuestInode;

use super::{AppHost, config, reply};

const SYSTEM_UID: i32 = 1000;
/// `android.content.pm.Flags.sdkDependencyInstaller`,
/// `fixSystemAppsFirstInstallTime`.
const SDK_DEPENDENCY_INSTALLER: &str = "android.content.pm.sdk_dependency_installer";
const FIX_SYSTEM_APPS_FIRST_INSTALL_TIME: &str =
    "android.content.pm.fix_system_apps_first_install_time";
/// `Configuration.KEYBOARDHIDDEN_NO`, `HARDKEYBOARDHIDDEN_YES`, and the
/// `ResTable_config` value `KEYSHIDDEN_SOFT` they make together.
const KEYBOARDHIDDEN_NO: i32 = 1;
const HARDKEYBOARDHIDDEN_YES: i32 = 2;
const KEYSHIDDEN_SOFT: i32 = 3;
/// What system_server's creators make: `prepareStageDir`'s `mkdir` and
/// `chmod` of 0775, and an `AtomicFile`'s file (0666 under system_server's
/// umask of 077, `ZygoteInit.handleSystemServerProcess`), both the system's.
const STAGE_DIR_MODE: u32 = 0o775;
const ATOMIC_FILE_MODE: u32 = 0o600;
/// `Environment.getPackageCacheDirectory`, under the guest's `/data`.
const PACKAGE_CACHE: &str = "system/package_cache";

pub struct BootConfiguration {
    host: Arc<AppHost>,
    factory_test: bool,
}

impl BootConfiguration {
    /// The leaf the bridge hands out for a boot in `factory_test` mode.
    pub fn new(host: Arc<AppHost>, factory_test: bool) -> Self {
        BootConfiguration { host, factory_test }
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        Ok(
            crate::package::parse::Platform::load(self.host.root()?, Default::default())
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
                .flags
                .get(name)
                .copied()
                .unwrap_or(false),
        )
    }

    /// `captureResources`.
    fn resources(&self) -> Result<Vec<u8>, Exception> {
        let configuration = self.host.global_configuration()?;
        let display = self.host.display()?;
        let property = |name: &str| self.host.property(name).unwrap_or_default();
        resources(
            &configuration,
            (display.0.width, display.0.height),
            resources_sdk(
                &property("ro.build.version.sdk"),
                &property("ro.build.version.codename"),
            ),
        )
    }

    /// `getLiveProperties`: each key once, each set.
    fn live_properties(&self, keys: Option<Vec<Option<String>>>) -> Result<Vec<u8>, Exception> {
        live_properties(&|name| self.host.property(name), keys)
    }

    /// `preparePackageParserCache(IS_ENG, IS_USERDEBUG, INCREMENTAL)`: none
    /// on an eng build or with `pm.boot.disable_package_cache`; else the
    /// directory named by `PackagePartitions.FINGERPRINT` under
    /// `/data/system/package_cache`, the others deleted, made again on a
    /// userdebug `eng.` build when the framework is newer.
    fn parser_cache(&self) -> Result<Option<String>, Exception> {
        let property = |name: &str| self.host.property(name);
        if property("ro.build.type").as_deref() == Some("eng")
            || matches!(
                property("pm.boot.disable_package_cache").as_deref(),
                Some("1" | "y" | "yes" | "on" | "true")
            )
        {
            return Ok(None);
        }
        let base = self.host.data()?.join(PACKAGE_CACHE);
        let name = super::app_data::fingerprint(property);
        let framework = self.host.root()?.join("system/framework");
        let incremental = property("ro.build.version.incremental").unwrap_or_default();
        let userdebug_eng = property("ro.build.type").as_deref() == Some("userdebug")
            && incremental.starts_with("eng.");
        prepare_parser_cache(&base, &name, userdebug_eng.then_some(framework.as_path()))
            .map(|made| made.then(|| format!("/data/{PACKAGE_CACHE}/{name}")))
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("package cache: {e}")))
    }

    /// `NativeBootInodeInputs.capture`: what the original creator of the
    /// one of these paths makes, its parent there.
    fn creation_inode(&self, path: Option<&str>) -> Result<Vec<u8>, Exception> {
        let path = path.unwrap_or_default();
        let (mode, parent) = creation(path)?;
        let host = super::resources::host_path(self.host.root()?, self.host.data()?, parent)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        if !host.is_dir() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!(
                    "Native boot inode creation failed: Creation parent owner absent: {parent}"
                ),
            ));
        }
        let mut p = Parcel::new();
        p.write_i32(SYSTEM_UID);
        p.write_i32(SYSTEM_UID);
        p.write_i32(mode as i32);
        Ok(p.data().to_vec())
    }

    /// The boot's user inventory, never empty (`getUserIds`).
    fn boot_users(&self) -> Result<Vec<i32>, Exception> {
        let users = super::user_service::user_ids(&self.host, true)?;
        if users.is_empty() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Original UM boot inventory empty",
            ));
        }
        Ok(users)
    }
}

/// `ResourcesManager`'s `RESOURCES_SDK_INT`: the SDK, one more before its
/// release.
fn resources_sdk(sdk: &str, codename: &str) -> i32 {
    sdk.parse::<i32>().unwrap_or(0) + i32::from(!codename.is_empty() && codename != "REL")
}

/// `ResTable_config`'s packing of a locale's language or country
/// (`pack`): two letters as they are, three in five-bit codes from `base`.
fn pack(value: &str, base: u8) -> Result<[i32; 2], Exception> {
    let b = value.as_bytes();
    match b.len() {
        0 => Ok([0, 0]),
        2 => Ok([b[0] as i32, b[1] as i32]),
        3 => {
            let code = |c: u8| {
                Some(c as i32 - base as i32)
                    .filter(|v| (0..=31).contains(v))
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Locale packed code invalid"))
            };
            let (first, second, third) = (code(b[0])?, code(b[1])?, code(b[2])?);
            Ok([
                0x80 | (third << 2) | (second >> 3),
                ((second << 5) | first) & 0xff,
            ])
        }
        _ => Err(Exception::new(
            EX_ILLEGAL_STATE,
            "Locale code cannot be represented by ResTable_config",
        )),
    }
}

/// `Locale.forLanguageTag(tag)`'s language and country: the first subtag
/// lowercased, and the region (two letters, or three digits) uppercased.
fn language_and_country(tag: &str) -> (String, String) {
    let mut parts = tag.split(['-', '_']);
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    let country = parts
        .find(|p| {
            (p.len() == 2 && p.bytes().all(|c| c.is_ascii_alphabetic()))
                || (p.len() == 3 && p.bytes().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or_default()
        .to_ascii_uppercase();
    (language, country)
}

/// `captureResources`' record: system_server's configuration as
/// `AssetManager.NativeSetConfiguration` takes it, the larger dimension
/// first, `minorVersion` 0.
fn resources(
    c: &config::Configuration,
    (width, height): (i32, i32),
    sdk: i32,
) -> Result<Vec<u8>, Exception> {
    let first = c
        .locales
        .split(',')
        .next()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Actual resource locale list empty"))?;
    let (language, country) = language_and_country(first);
    let language = pack(&language, b'a')?;
    let country = pack(&country, b'0')?;
    let hidden = if c.keyboard_hidden == KEYBOARDHIDDEN_NO
        && c.hard_keyboard_hidden == HARDKEYBOARDHIDDEN_YES
    {
        KEYSHIDDEN_SOFT
    } else {
        c.keyboard_hidden
    };
    let values = [
        c.mcc,
        c.mnc,
        language[0],
        language[1],
        country[0],
        country[1],
        c.orientation,
        c.touchscreen,
        c.density_dpi,
        c.keyboard,
        c.navigation,
        hidden,
        c.grammatical_gender,
        width.max(height),
        width.min(height),
        sdk,
        0,
        c.screen_layout,
        c.ui_mode,
        c.smallest_screen_width_dp,
        c.screen_width_dp,
        c.screen_height_dp,
        (c.screen_layout & 0x300) >> 8,
        c.color_mode,
    ];
    let mut p = Parcel::new();
    p.write_i32(1);
    p.write_i32(values.len() as i32);
    for v in values {
        p.write_i32(v);
    }
    Ok(p.data().to_vec())
}

fn live_properties(
    property: &dyn Fn(&str) -> Option<String>,
    keys: Option<Vec<Option<String>>>,
) -> Result<Vec<u8>, Exception> {
    let keys = keys
        .filter(|k| !k.is_empty())
        .ok_or_else(|| Exception::new(EX_ILLEGAL_ARGUMENT, "Actual property inventory absent"))?;
    let mut p = Parcel::new();
    p.write_i32(1);
    p.write_i32(keys.len() as i32);
    let mut seen = std::collections::HashSet::new();
    for key in &keys {
        let key = key
            .as_deref()
            .filter(|k| !k.is_empty() && seen.insert(*k))
            .ok_or_else(|| Exception::new(EX_ILLEGAL_ARGUMENT, "Invalid actual property key"))?;
        let value = property(key).ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Actual required property absent: {key}"),
            )
        })?;
        p.write_string16(Some(key));
        p.write_string16(Some(&value));
    }
    Ok(p.data().to_vec())
}

/// The mode the original creator of `path` gives, and the directory it
/// makes it in: a stage directory under `/data/app`, the install sessions'
/// file or a user's runtime permissions, both AtomicFiles.
fn creation(path: &str) -> Result<(u32, &str), Exception> {
    let runtime = path
        .strip_prefix("/data/misc_de/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(user, rest)| {
            !user.is_empty()
                && user.bytes().all(|c| c.is_ascii_digit())
                && rest == "apexdata/com.android.permission/runtime-permissions.xml"
        });
    match path {
        "/data/app/vmdl.session.tmp" => Ok((STAGE_DIR_MODE, "/data/app")),
        "/data/system/install_sessions.xml" => Ok((ATOMIC_FILE_MODE, "/data/system")),
        _ if runtime => Ok((ATOMIC_FILE_MODE, path.rsplit_once('/').unwrap().0)),
        _ => Err(Exception::new(EX_ILLEGAL_ARGUMENT, "Unowned inode request")),
    }
}

/// `preparePackageParserCache`'s files: `base` made (`FileUtils.createDir`),
/// every directory but `name` deleted, `name` made; with `framework` (a
/// userdebug `eng.` build), everything made again if the framework is
/// newer. Whether the cache directory is there.
fn prepare_parser_cache(
    base: &Path,
    name: &str,
    framework: Option<&Path>,
) -> std::io::Result<bool> {
    // System-owned, as system_server's `mkdir` makes them.
    const SYSTEM_DIR: GuestInode = GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o700),
    };
    let create = |dir: &Path| -> std::io::Result<bool> {
        if dir.exists() {
            return Ok(dir.is_dir());
        }
        match std::fs::create_dir(dir) {
            Ok(()) => aim_storage::guest_inode::record(dir, SYSTEM_DIR).map(|()| true),
            Err(_) => Ok(false),
        }
    };
    if !create(base)? {
        return Ok(false);
    }
    let remove_others = || -> std::io::Result<()> {
        for entry in std::fs::read_dir(base)? {
            let entry = entry?;
            if entry.file_name() == name {
                continue;
            }
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    };
    remove_others()?;
    let cache = base.join(name);
    if !create(&cache)? {
        std::fs::remove_dir_all(base)?;
        return Ok(false);
    }
    if let Some(framework) = framework {
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified());
        if modified(&cache)? < modified(framework)? {
            for entry in std::fs::read_dir(base)? {
                let path = entry?.path();
                if path.is_dir() {
                    std::fs::remove_dir_all(&path)?;
                } else {
                    std::fs::remove_file(&path)?;
                }
            }
            return create(&cache);
        }
    }
    Ok(true)
}

impl Service for BootConfiguration {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforce`: the system uid.
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Native boot configuration requires system uid",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::GET_DENSITY => {
                leaf::GetDensity::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host
                    .global_configuration()
                    .map(|c| leaf::write_get_density_reply(&mut p, c.density_dpi))
            }
            leaf::IS_FACTORY_TEST => {
                leaf::IsFactoryTest::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                leaf::write_is_factory_test_reply(&mut p, self.factory_test);
                Ok(())
            }
            leaf::IS_DEPENDENCY_INSTALLER_ENABLED => {
                leaf::IsDependencyInstallerEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.flag(SDK_DEPENDENCY_INSTALLER)
                    .map(|v| leaf::write_is_dependency_installer_enabled_reply(&mut p, v))
            }
            leaf::FIX_SYSTEM_APPS_FIRST_INSTALL_TIME => {
                leaf::FixSystemAppsFirstInstallTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.flag(FIX_SYSTEM_APPS_FIRST_INSTALL_TIME)
                    .map(|v| leaf::write_fix_system_apps_first_install_time_reply(&mut p, v))
            }
            leaf::CREATION_INODE => {
                let a = leaf::CreationInode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.creation_inode(a.guest_path.as_deref())
                    .map(|v| leaf::write_creation_inode_reply(&mut p, &Some(v)))
            }
            leaf::CAPTURE_RESOURCES => {
                leaf::CaptureResources::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.resources()
                    .map(|v| leaf::write_capture_resources_reply(&mut p, &Some(v)))
            }
            leaf::GET_LIVE_PROPERTIES => {
                let a = leaf::GetLiveProperties::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.live_properties(a.keys)
                    .map(|v| leaf::write_get_live_properties_reply(&mut p, &Some(v)))
            }
            leaf::GET_PARSER_CACHE_DIRECTORY => {
                leaf::GetParserCacheDirectory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.parser_cache()
                    .map(|v| leaf::write_get_parser_cache_directory_reply(&mut p, &v))
            }
            leaf::GET_USER_IDS => {
                leaf::GetUserIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.boot_users()
                    .map(|v| leaf::write_get_user_ids_reply(&mut p, &Some(v)))
            }
            // `getUsers(true, true, true)`, and `UserManagerInternal
            // .getUserIds`: neither the partial nor the pre-created.
            leaf::GET_PACKAGE_LIST_ACTIVE_USER_IDS => {
                leaf::GetPackageListActiveUserIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::user_ids(&self.host, false)
                    .map(|v| leaf::write_get_package_list_active_user_ids_reply(&mut p, &Some(v)))
            }
            leaf::GET_RUNTIME_PERMISSION_USER_IDS => {
                leaf::GetRuntimePermissionUserIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::user_ids(&self.host, false)
                    .map(|v| leaf::write_get_runtime_permission_user_ids_reply(&mut p, &Some(v)))
            }
            code => Err(self.host.unimplemented("package", leaf::DESCRIPTOR, code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;

    /// The native scan asks for density and resources before it publishes
    /// package queries. GrammaticalInflectionService's flag is an image
    /// input, so the global configuration must be available then too.
    #[test]
    fn boot_configuration_precedes_package_queries() {
        struct Data(std::path::PathBuf);
        impl Drop for Data {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let Some(root) = aim_paths::original_image_with("system/framework/framework-res.apk")
        else {
            return;
        };
        let data = crate::app_host::test_temp_dir("boot-configuration");
        let _cleanup = Data(data.clone());
        let process = crate::test_process(SYSTEM_UID as u32);
        let host = AppHost::new(process.clone());
        host.set_system(crate::system::System::new(process.clone(), &[]));
        crate::app_host::configure_test_host(
            &host,
            &root,
            &data,
            &data,
            Box::new(|name| {
                Some(
                    match name {
                        "ro.build.version.sdk" => "36",
                        "ro.build.version.codename" => "REL",
                        "persist.sys.locale" => "en-US",
                        "ro.sf.lcd_density" => "320",
                        "persist.sys.grammatical_gender" => "3",
                        _ => return None,
                    }
                    .into(),
                )
            }),
        );
        let display = super::super::display::Display::phone_for_test();
        let density = display.density_dpi();
        assert!(
            host.display
                .set(Arc::new((
                    display,
                    super::super::display_service::Resources {
                        name: "Built-in Screen".into(),
                        density_dpi: density,
                        brightness: (1, 255, 128),
                        brightness_dim: 0.05,
                    },
                )))
                .is_ok()
        );
        let missing = host.package_capture().err().unwrap();
        assert_eq!(
            missing.message,
            "native package query owners are unavailable"
        );
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(BootConfiguration::new(host.clone(), false)))
        else {
            unreachable!()
        };
        let owner = process.local_service(ptr).unwrap();
        let mut request = Parcel::new();
        leaf::CaptureResources {}.write(&mut request);
        let reply = owner
            .transact(leaf::CAPTURE_RESOURCES, &request, false)
            .unwrap();
        let bytes = leaf::read_capture_resources_reply(&mut reply.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        let resources = crate::package::boot_configuration::decode_resources(&bytes).unwrap();
        assert_eq!(resources.density, density as u16);
        assert_eq!(resources.grammatical_inflection, 3);
        let mut request = Parcel::new();
        leaf::GetDensity {}.write(&mut request);
        let reply = owner.transact(leaf::GET_DENSITY, &request, false).unwrap();
        assert_eq!(
            leaf::read_get_density_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            density
        );
        assert!(host.package_capture().is_err());
    }
    #[test]
    fn locale_codes_pack_as_res_table_config() {
        assert_eq!(pack("ko", b'a').unwrap(), [b'k' as i32, b'o' as i32]);
        assert_eq!(pack("", b'a').unwrap(), [0, 0]);
        // "fil": f=5, i=8, l=11 -> 0x80 | 11<<2 | 8>>3, (8<<5 | 5) & 0xff.
        assert_eq!(pack("fil", b'a').unwrap(), [0x80 | 44 | 1, 5]);
        assert_eq!(pack("419", b'0').unwrap(), [0x80 | (9 << 2), (1 << 5) | 4]);
        assert!(pack("abcd", b'a').is_err());
        assert_eq!(language_and_country("ko-KR"), ("ko".into(), "KR".into()));
        assert_eq!(
            language_and_country("zh-Hant-TW"),
            ("zh".into(), "TW".into())
        );
        assert_eq!(language_and_country("es-419"), ("es".into(), "419".into()));
        assert_eq!(language_and_country("en"), ("en".into(), String::new()));
        assert_eq!(resources_sdk("36", "REL"), 36);
        assert_eq!(resources_sdk("36", "Baklava"), 37);
    }

    #[test]
    fn resources_are_the_24_res_table_config_fields() {
        let c = config::Configuration {
            locales: "ko-KR,en-US".into(),
            orientation: 2,
            touchscreen: 3,
            density_dpi: 320,
            keyboard: 2,
            navigation: 1,
            keyboard_hidden: KEYBOARDHIDDEN_NO,
            hard_keyboard_hidden: HARDKEYBOARDHIDDEN_YES,
            screen_layout: 0x112,
            ui_mode: 0x11,
            smallest_screen_width_dp: 600,
            screen_width_dp: 960,
            screen_height_dp: 600,
            ..Default::default()
        };
        let bytes = resources(&c, (1200, 1920), 36).unwrap();
        let mut r = Reader::new(&bytes, &[]);
        let v: Vec<i32> = (0..26).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(&v[..2], &[1, 24]);
        assert_eq!(
            &v[4..8],
            &[b'k' as i32, b'o' as i32, b'K' as i32, b'R' as i32]
        );
        assert_eq!(v[2 + 11], KEYSHIDDEN_SOFT);
        assert_eq!(&v[2 + 13..2 + 17], &[1920, 1200, 36, 0]);
        assert_eq!(v[2 + 22], 1);
        assert_eq!(r.remaining(), 0);
        // The native boot reads it back as the resource configuration.
        let read = crate::package::boot_configuration::decode_resources(&bytes).unwrap();
        assert_eq!((read.language, read.country), (*b"ko", *b"KR"));
        assert_eq!(
            (read.density, read.screen_width, read.screen_height),
            (320, 1920, 1200)
        );
        let empty = config::Configuration::default();
        assert!(resources(&empty, (1, 1), 36).is_err());
    }

    #[test]
    fn live_properties_need_each_key_set_once() {
        let props = |k: &str| (k == "ro.a").then(|| "1".to_string());
        let bytes = live_properties(&props, Some(vec![Some("ro.a".into())])).unwrap();
        let mut r = Reader::new(&bytes, &[]);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 1));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("ro.a"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("1"));
        let code = |keys| live_properties(&props, keys).unwrap_err().code;
        assert_eq!(code(None), EX_ILLEGAL_ARGUMENT);
        assert_eq!(code(Some(vec![])), EX_ILLEGAL_ARGUMENT);
        assert_eq!(
            code(Some(vec![Some("ro.a".into()), Some("ro.a".into())])),
            EX_ILLEGAL_ARGUMENT
        );
        assert_eq!(code(Some(vec![Some("ro.b".into())])), EX_ILLEGAL_STATE);
    }

    #[test]
    fn creations_follow_their_original_creators() {
        assert_eq!(
            creation("/data/app/vmdl.session.tmp").unwrap(),
            (0o775, "/data/app")
        );
        assert_eq!(
            creation("/data/system/install_sessions.xml").unwrap(),
            (0o600, "/data/system")
        );
        assert_eq!(
            creation("/data/misc_de/0/apexdata/com.android.permission/runtime-permissions.xml")
                .unwrap(),
            (0o600, "/data/misc_de/0/apexdata/com.android.permission")
        );
        for path in [
            "/data/misc_de/x/apexdata/com.android.permission/runtime-permissions.xml",
            "/data/system/packages.xml",
        ] {
            assert_eq!(creation(path).unwrap_err().code, EX_ILLEGAL_ARGUMENT);
        }
    }

    #[test]
    fn the_parser_cache_keeps_only_its_fingerprint() {
        let base = crate::app_host::test_temp_dir("parser-cache");
        std::fs::remove_dir_all(&base).unwrap();
        assert!(prepare_parser_cache(&base, "abc", None).unwrap());
        std::fs::create_dir(base.join("old")).unwrap();
        std::fs::write(base.join("old/entry"), b"x").unwrap();
        std::fs::write(base.join("abc/entry"), b"kept").unwrap();
        assert!(prepare_parser_cache(&base, "abc", None).unwrap());
        assert!(!base.join("old").exists());
        assert_eq!(std::fs::read(base.join("abc/entry")).unwrap(), b"kept");
        assert_eq!(
            aim_storage::guest_inode::read(&base.join("abc"))
                .unwrap()
                .unwrap()
                .mode,
            Some(0o700)
        );
        // A newer framework on a userdebug eng. build empties the cache.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let framework = base.with_extension("framework");
        std::fs::create_dir_all(&framework).unwrap();
        assert!(prepare_parser_cache(&base, "abc", Some(&framework)).unwrap());
        assert!(!base.join("abc/entry").exists());
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::remove_dir_all(&framework).unwrap();
    }
}
