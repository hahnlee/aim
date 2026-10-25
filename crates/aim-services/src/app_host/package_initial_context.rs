//! `IPackageInitialContextLeaf` for the app host: what the native package
//! boot reads of the platform before it scans, answered as
//! `dev.aim.server.PackageInitialContextLeaf` answers it in system_server
//! (`android-16.0.0_r1`): the live properties, the scan's platform facts
//! (`Build.SUPPORTED_*_ABIS`, the runtime's word size, `/data/app-lib`, the
//! time for `fixSystemAppsFirstInstallTime`, the page size,
//! `Build.isDebuggable`), a permission group, `isInstallDisabledForPackage`,
//! the auto-revoke op, the device policy facts, `default_install_location`,
//! the mounted private volumes, incremental paths, a zip entry's time in the
//! device's zone, a directory's guest owner and mode, the SDK sandbox's
//! service action and the boot class path's APEX modules.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::dev_aim_server_ipackageinitialcontextleaf as leaf;

use super::{AppHost, reply};

const SYSTEM_UID: i32 = 1000;
/// `AppOpsManager.OP_REQUEST_INSTALL_PACKAGES`,
/// `OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED`; `MODE_ALLOWED`.
const OP_REQUEST_INSTALL_PACKAGES: i32 = 66;
pub(super) const OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED: i32 = 97;
const MODE_ALLOWED: i32 = 0;
/// `UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES` and its global form.
const RESTRICTIONS: [&str; 2] = [
    "no_install_unknown_sources",
    "no_install_unknown_sources_globally",
];
/// `android.content.pm.Flags.fixSystemAppsFirstInstallTime`.
const FIX_SYSTEM_APPS_FIRST_INSTALL_TIME: &str =
    "android.content.pm.fix_system_apps_first_install_time";
/// The guest's page size: the layer's `AT_PAGESZ`, 16 KiB.
pub(super) const PAGE_SIZE: i64 = 16_384;
/// `SdkSandboxManagerLocal.SERVICE_INTERFACE`.
const SDK_SANDBOX_SERVICE: &str = "com.android.sdksandbox.SdkSandboxService";

pub struct InitialContext {
    host: Arc<AppHost>,
}

impl InitialContext {
    pub fn new(host: Arc<AppHost>) -> Self {
        InitialContext { host }
    }

    /// `SystemProperties.get`: empty when unset.
    fn property(&self, name: &str) -> String {
        self.host.property(name).unwrap_or_default()
    }

    /// `captureImageScanInputs`.
    fn scan_inputs(&self) -> Result<Vec<u8>, Exception> {
        let root = self.host.root()?;
        let flags = crate::package::parse::Platform::load(root, Default::default())
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
            .flags;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        Ok(scan_inputs(
            &|name| self.property(name),
            flags
                .get(FIX_SYSTEM_APPS_FIRST_INSTALL_TIME)
                .copied()
                .unwrap_or(false),
            now,
        ))
    }

    /// `PackageManagerService.isInstallDisabledForPackage`: the user's
    /// unknown-sources restrictions, then the external sources policy
    /// AppOpsService sets (`REQUEST_INSTALL_PACKAGES`, trusted only when
    /// allowed).
    pub(super) fn install_disabled(
        &self,
        name: &str,
        uid: i32,
        user: i32,
    ) -> Result<bool, Exception> {
        // The app host's only user is 0; another has no restrictions.
        if user == 0 {
            let restrictions = self.host.user_restrictions()?;
            if RESTRICTIONS.iter().any(|r| restrictions.contains(*r)) {
                return Ok(true);
            }
        }
        Ok(
            super::appops_service::mode(&self.host, OP_REQUEST_INSTALL_PACKAGES, uid, Some(name))?
                != MODE_ALLOWED,
        )
    }

    /// `getAdminFacts`: the device policy service is present; with no
    /// owner, admin or policy persisted, no device owner and no user with
    /// the package's active admins or as its organization's manager.
    fn admin_facts(&self) -> Result<Vec<u8>, Exception> {
        let policies = super::device_policy::Policies::load(self.host.data()?)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        if policies.owned() || policies.has_admins_or_policies() {
            return Err(self.host.unimplemented_named(
                "package",
                "the admin facts of a device with admins (#1521)",
            ));
        }
        let mut p = Parcel::new();
        p.write_bool(true);
        p.write_string16(None);
        for _ in 0..2 {
            p.write_i32(0);
        }
        Ok(p.data().to_vec())
    }

    /// `Settings.Global.getInt(default_install_location, 0)`.
    pub(super) fn install_location(&self) -> Result<i32, Exception> {
        Ok(self
            .host
            .platform()?
            .settings
            .get(super::settings::Table::Global, "default_install_location")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// `nativeLibraryInode`: `Os.stat`'s owner and permission bits, as the
    /// layer reports them for the guest (`dev.aim.guest-inode`, else root
    /// and the host's mode bits).
    fn native_library_inode(&self, path: &str) -> Result<Vec<u8>, Exception> {
        let host = super::resources::host_path(self.host.root()?, self.host.data()?, path)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let errno = |e: std::io::Error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("android.system.ErrnoException: stat failed: {e}"),
            )
        };
        let meta = std::fs::metadata(&host).map_err(errno)?;
        let recorded = aim_storage::guest_inode::read(&host).map_err(errno)?;
        Ok(inode(
            recorded.unwrap_or_default(),
            std::os::unix::fs::PermissionsExt::mode(&meta.permissions()),
        ))
    }

    /// `ZoneId.systemDefault()`: `persist.sys.timezone`, GMT when the
    /// image's tzdata has no such zone.
    fn zip_entry_time(&self, dos: i32) -> Result<i64, Exception> {
        let root = self.host.root()?;
        let tzdata = std::fs::read(root.join(crate::tzdata::PATH.trim_start_matches('/')))
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("tzdata: {e}")))?;
        let zone = self.host.property("persist.sys.timezone");
        let rules = zone
            .as_deref()
            .and_then(|z| crate::tzdata::zone(&tzdata, z))
            .or_else(|| crate::tzdata::zone(&tzdata, "GMT"))
            .and_then(crate::tzdata::Rules::parse)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "tzdata: no default zone"))?;
        zip_entry_time(&rules, dos)
    }

    /// `getBootClassPathApexModules`: the APEXes of system_server's
    /// `BOOTCLASSPATH`, which derive_classpath makes of the classpath
    /// fragments, in order.
    fn boot_class_path_apex_modules(&self) -> Result<Vec<Option<String>>, Exception> {
        let jars = aim_android_image::classpath::jars(
            self.host.root()?,
            "bootclasspath.pb",
            aim_android_image::classpath::BOOTCLASSPATH,
        )
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        if jars.is_empty() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Original BOOTCLASSPATH absent",
            ));
        }
        Ok(apex_modules(&jars).into_iter().map(Some).collect())
    }
}

/// `captureImageScanInputs`' record: its schema, `SUPPORTED_ABIS`,
/// `_32_BIT_` and `_64_BIT_` (`ro.product.cpu.abilist*`, split at commas),
/// whether system_server's runtime is 64-bit (the primary zygote's,
/// `ro.zygote`), `/data/app-lib`, the time if the flag fixes system apps'
/// first install time, the page size and `ro.debuggable`.
fn scan_inputs(property: &dyn Fn(&str) -> String, fix_first_install: bool, now: i64) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(1);
    for name in [
        "ro.product.cpu.abilist",
        "ro.product.cpu.abilist32",
        "ro.product.cpu.abilist64",
    ] {
        let value = property(name);
        let list: Vec<&str> = if value.is_empty() {
            Vec::new()
        } else {
            value.split(',').collect()
        };
        p.write_i32(list.len() as i32);
        for abi in list {
            p.write_string16(Some(abi));
        }
    }
    p.write_bool(property("ro.zygote").starts_with("zygote64"));
    p.write_string16(Some("/data/app-lib"));
    p.write_i64(if fix_first_install { now } else { 0 });
    p.write_i64(PAGE_SIZE);
    p.write_bool(property("ro.debuggable") == "1");
    p.data().to_vec()
}

/// `getLiveProperties`' record: its schema, then each name and value.
fn live_properties(
    property: &dyn Fn(&str) -> String,
    names: &[Option<String>],
) -> Result<Vec<u8>, Exception> {
    let mut p = Parcel::new();
    p.write_i32(1);
    p.write_i32(names.len() as i32);
    for name in names {
        let name = name.as_deref().ok_or_else(null)?;
        p.write_string16(Some(name));
        p.write_string16(Some(&property(name)));
    }
    Ok(p.data().to_vec())
}

/// `nativeLibraryInode`'s record: uid, gid and permission bits.
fn inode(recorded: aim_storage::guest_inode::GuestInode, host_mode: u32) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i32(recorded.uid.unwrap_or(0) as i32);
    p.write_i32(recorded.gid.unwrap_or(0) as i32);
    p.write_i32((recorded.mode.unwrap_or(host_mode) & 0o7777) as i32);
    p.data().to_vec()
}

/// `zipEntryTime`: an MS-DOS date and time as `LocalDateTime.of` reads it,
/// in the zone, as epoch milliseconds.
fn zip_entry_time(rules: &crate::tzdata::Rules, dos: i32) -> Result<i64, Exception> {
    let v = dos as u32 as i64;
    let (year, month, day) = ((v >> 25) + 1980, (v >> 21) & 15, (v >> 16) & 31);
    let (hour, minute, second) = ((v >> 11) & 31, (v >> 5) & 63, (v & 31) * 2);
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    // `LocalDateTime.of`'s DateTimeException for a field out of range.
    if day < 1 || day > month_days || hour > 23 || minute > 59 || second > 59 {
        return Err(Exception::new(
            EX_ILLEGAL_ARGUMENT,
            format!("java.time.DateTimeException: Invalid date or time {dos:#x}"),
        ));
    }
    let local = crate::tzdata::days_from_civil(year, month, day) * 86_400
        + hour * 3600
        + minute * 60
        + second;
    Ok(rules.local_to_utc(local) * 1000)
}

/// The APEX module of each `/apex/<module>/...` jar, once, in order.
fn apex_modules(jars: &[String]) -> Vec<String> {
    let mut modules: Vec<String> = Vec::new();
    for jar in jars {
        let mut parts = jar.split('/').filter(|p| !p.is_empty());
        if let (Some("apex"), Some(module)) = (parts.next(), parts.next())
            && !modules.iter().any(|m| m == module)
        {
            modules.push(module.to_string());
        }
    }
    modules
}

fn null() -> Exception {
    Exception::new(EX_NULL_POINTER, "java.lang.NullPointerException")
}

impl Service for InitialContext {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforce`: the native owner, the system.
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Native initial context owner required",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::GET_LIVE_PROPERTY => {
                let a = leaf::GetLiveProperty::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.name.ok_or_else(null).map(|name| {
                    leaf::write_get_live_property_reply(&mut p, &Some(self.property(&name)))
                })
            }
            leaf::GET_LIVE_PROPERTIES => {
                let a = leaf::GetLiveProperties::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.names
                    .ok_or_else(null)
                    .and_then(|names| live_properties(&|n| self.property(n), &names))
                    .map(|v| leaf::write_get_live_properties_reply(&mut p, &Some(v)))
            }
            leaf::CAPTURE_IMAGE_SCAN_INPUTS => {
                leaf::CaptureImageScanInputs::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.scan_inputs()
                    .map(|v| leaf::write_capture_image_scan_inputs_reply(&mut p, &Some(v)))
            }
            leaf::GET_PERMISSION_GROUP => {
                let a = leaf::GetPermissionGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `IPermissionManager.getPermissionGroupInfo`, which requires
                // the name.
                self.host
                    .permission_manager
                    .get()
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "permission manager unavailable")
                    })
                    .and_then(|manager| {
                        let name = a.name.as_deref().ok_or_else(null)?;
                        manager.group_info(SYSTEM_UID, name, a.flags)
                    })
                    .map(|info| {
                        let info = info.map(|info| {
                            let mut out = Parcel::new();
                            info.write_to(&mut out);
                            out.data().to_vec()
                        });
                        leaf::write_get_permission_group_reply(&mut p, &info);
                    })
            }
            leaf::IS_INSTALL_DISABLED => {
                let a = leaf::IsInstallDisabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.install_disabled(a.name.as_deref().unwrap_or_default(), a.uid, a.user)
                    .map(|v| leaf::write_is_install_disabled_reply(&mut p, v))
            }
            leaf::GET_AUTO_REVOKE => {
                let a = leaf::GetAutoRevoke::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::appops_service::mode(
                    &self.host,
                    OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED,
                    a.uid,
                    a.name.as_deref(),
                )
                .map(|mode| leaf::write_get_auto_revoke_reply(&mut p, mode))
            }
            leaf::GET_ADMIN_FACTS => {
                leaf::GetAdminFacts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.admin_facts()
                    .map(|v| leaf::write_get_admin_facts_reply(&mut p, &Some(v)))
            }
            leaf::GET_INSTALL_LOCATION => {
                leaf::GetInstallLocation::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.install_location()
                    .map(|v| leaf::write_get_install_location_reply(&mut p, v))
            }
            leaf::GET_MOUNTED_PRIVATE_VOLUMES => {
                leaf::GetMountedPrivateVolumes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uuids = self
                    .host
                    .mounted_private_volume_uuids()
                    .into_iter()
                    .map(Some)
                    .collect();
                leaf::write_get_mounted_private_volumes_reply(&mut p, &Some(uuids));
                Ok(())
            }
            // `IncrementalManager.isIncrementalPath`: whether the path is on
            // incfs; the layer has no incremental file system, so none is.
            leaf::IS_INCREMENTAL_PATH => {
                leaf::IsIncrementalPath::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                leaf::write_is_incremental_path_reply(&mut p, false);
                Ok(())
            }
            leaf::ZIP_ENTRY_TIME => {
                let a = leaf::ZipEntryTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.zip_entry_time(a.dos_time)
                    .map(|v| leaf::write_zip_entry_time_reply(&mut p, v))
            }
            leaf::NATIVE_LIBRARY_INODE => {
                let a = leaf::NativeLibraryInode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.path
                    .ok_or_else(null)
                    .and_then(|path| self.native_library_inode(&path))
                    .map(|v| leaf::write_native_library_inode_reply(&mut p, &Some(v)))
            }
            leaf::GET_SDK_SANDBOX_SERVICE_ACTION => {
                leaf::GetSdkSandboxServiceAction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                leaf::write_get_sdk_sandbox_service_action_reply(
                    &mut p,
                    &Some(SDK_SANDBOX_SERVICE.into()),
                );
                Ok(())
            }
            leaf::GET_BOOT_CLASS_PATH_APEX_MODULES => {
                leaf::GetBootClassPathApexModules::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.boot_class_path_apex_modules()
                    .map(|m| leaf::write_get_boot_class_path_apex_modules_reply(&mut p, &Some(m)))
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

    fn properties(name: &str) -> String {
        match name {
            "ro.product.cpu.abilist" => "arm64-v8a",
            "ro.product.cpu.abilist64" => "arm64-v8a",
            "ro.zygote" => "zygote64",
            "ro.debuggable" => "0",
            _ => "",
        }
        .into()
    }

    /// The record the native owner's `scan_facts` reads back.
    #[test]
    fn scan_inputs_are_the_original_record() {
        let bytes = scan_inputs(&properties, true, 1_700_000_000_000);
        let mut r = Reader::new(&bytes, &[]);
        assert_eq!(r.read_i32().unwrap(), 1);
        let list = |r: &mut Reader<'_>| {
            (0..r.read_i32().unwrap())
                .map(|_| r.read_string16().unwrap().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(list(&mut r), ["arm64-v8a"]);
        assert!(list(&mut r).is_empty());
        assert_eq!(list(&mut r), ["arm64-v8a"]);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("/data/app-lib"));
        assert_eq!(r.read_i64().unwrap(), 1_700_000_000_000);
        assert_eq!(r.read_i64().unwrap(), PAGE_SIZE);
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.remaining(), 0);
        // Without the flag, no time.
        let unfixed = scan_inputs(&properties, false, 5);
        let mut r = Reader::new(&unfixed, &[]);
        r.read_i32().unwrap();
        for _ in 0..3 {
            list(&mut r);
        }
        r.read_bool().unwrap();
        r.read_string16().unwrap();
        assert_eq!(r.read_i64().unwrap(), 0);
    }

    #[test]
    fn live_properties_pair_names_with_values() {
        let bytes = live_properties(
            &properties,
            &[Some("ro.zygote".into()), Some("unset".into())],
        )
        .unwrap();
        let mut r = Reader::new(&bytes, &[]);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 2));
        let pair = |r: &mut Reader<'_>| (r.read_string16().unwrap(), r.read_string16().unwrap());
        assert_eq!(
            pair(&mut r),
            (Some("ro.zygote".into()), Some("zygote64".into()))
        );
        assert_eq!(pair(&mut r), (Some("unset".into()), Some(String::new())));
        assert_eq!(
            live_properties(&properties, &[None]).unwrap_err().code,
            EX_NULL_POINTER
        );
    }

    #[test]
    fn inodes_report_the_guest_owner() {
        let record = |b: Vec<u8>| {
            let mut r = Reader::new(&b, &[]);
            [
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
            ]
        };
        let recorded = aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o771),
        };
        assert_eq!(record(inode(recorded, 0o40755)), [1000, 1000, 0o771]);
        assert_eq!(record(inode(Default::default(), 0o40755)), [0, 0, 0o755]);
    }

    #[test]
    fn zip_times_are_local_to_the_zone() {
        let Some(root) =
            aim_paths::original_image_with(crate::tzdata::PATH.trim_start_matches('/'))
        else {
            return;
        };
        let tzdata = std::fs::read(root.join(crate::tzdata::PATH.trim_start_matches('/'))).unwrap();
        let rules = |z: &str| {
            crate::tzdata::Rules::parse(crate::tzdata::zone(&tzdata, z).unwrap()).unwrap()
        };
        // 2008-02-29 12:34:56 local.
        let dos = ((2008 - 1980) << 25) | (2 << 21) | (29 << 16) | (12 << 11) | (34 << 5) | 28;
        let local = crate::tzdata::days_from_civil(2008, 2, 29) * 86_400 + 12 * 3600 + 34 * 60 + 56;
        assert_eq!(zip_entry_time(&rules("GMT"), dos).unwrap(), local * 1000);
        assert_eq!(
            zip_entry_time(&rules("Asia/Seoul"), dos).unwrap(),
            (local - 9 * 3600) * 1000
        );
        // Day 0 is not a date.
        assert_eq!(
            zip_entry_time(&rules("GMT"), (2 << 21) | (12 << 11))
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    /// The native boot's owner of the leaf (`InitialOwner`) reads it as this
    /// process's own node, as the app host serves it: the scan's facts and a
    /// live property.
    #[test]
    fn the_native_owner_reads_the_leaf_as_a_local_node() {
        use crate::system::package_boot_inputs::PlatformFacts;
        let Some(root) = aim_paths::original_image_with("system/etc/classpaths/bootclasspath.pb")
        else {
            return;
        };
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID as u32,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let data = std::env::temp_dir().join(format!("aim-initial-leaf-{}", std::process::id()));
        host.configure(
            &root,
            &data,
            &data,
            Box::new(|name| {
                properties(name)
                    .to_string()
                    .chars()
                    .next()
                    .map(|_| properties(name))
            }),
            Box::new(|_, _| {}),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(InitialContext::new(host)))
        else {
            unreachable!()
        };
        let owner = crate::system::package_boot_scan::InitialOwner::new(
            crate::system::ServiceOwner::Local(process.local_service(ptr).unwrap()),
        );
        let facts = owner.scan_facts().unwrap();
        assert_eq!(facts.abis, ["arm64-v8a"]);
        assert!(facts.abis32.is_empty() && facts.runtime_64 && !facts.debuggable);
        assert_eq!(
            (facts.app_lib32.as_str(), facts.page_size),
            ("/data/app-lib", PAGE_SIZE)
        );
        assert_eq!(
            owner.property("ro.zygote").unwrap().as_deref(),
            Some("zygote64")
        );
        assert_eq!(owner.property("unset").unwrap(), None);
    }

    #[test]
    fn boot_class_path_modules_are_its_apexes_in_order() {
        let jars = [
            "/apex/com.android.art/javalib/core-oj.jar",
            "/system/framework/framework.jar",
            "/apex/com.android.art/javalib/core-libart.jar",
            "/apex/com.android.i18n/javalib/core-icu4j.jar",
        ]
        .map(String::from);
        assert_eq!(apex_modules(&jars), ["com.android.art", "com.android.i18n"]);
    }
}
