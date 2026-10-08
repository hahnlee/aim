//! Concrete image/session producers for the live installation environment.
use super::{
    environment::{InstallSource, LibraryPolicy, Metadata, ZipClock},
    pipeline::VerifiedCode,
};
use crate::package::{pkg::booleans, scan, scan_snapshot::Snapshot, settings};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

fn uuid() -> [u8; 16] {
    let mut value = [0u8; 16];
    unsafe {
        libc::arc4random_buf(value.as_mut_ptr().cast(), value.len());
    }
    value[6] = (value[6] & 15) | 0x40;
    value[8] = (value[8] & 63) | 0x80;
    value
}
fn internal_name(code: &VerifiedCode) -> String {
    if code.package.static_shared_library_name.is_some() {
        format!(
            "{}_{}",
            code.package.package_name, code.package.static_shared_lib_version
        )
    } else {
        code.package.package_name.clone()
    }
}
pub fn metadata(apks: Arc<crate::package::write::Apks>) -> Metadata {
    Arc::new(move |code, destination, base| {
        let name = internal_name(code);
        let old = base
            .owner()
            .settings
            .packages
            .iter()
            .find(|package| package.name == name);
        let system = old.is_some_and(|package| package.flags & settings::FLAG_SYSTEM != 0);
        let mut parsed = code.package.clone();
        // Preserve physical system ownership for updates while manifest flags
        // continue to come from the accepted native parser.
        if system {
            parsed.booleans |= booleans::SYSTEM;
        }
        let (flags, private_flags) = scan::application_flags(&parsed, system);
        let versions = parsed
            .uses_sdk_libraries_versions_major
            .as_deref()
            .unwrap_or_default();
        let optional = parsed
            .uses_sdk_libraries_optional
            .as_deref()
            .unwrap_or_default();
        if versions.len() != parsed.uses_sdk_libraries.len() || optional.len() != versions.len() {
            return Err("Live SDK library metadata arrays disagree".into());
        }
        let sdk = parsed
            .uses_sdk_libraries
            .iter()
            .zip(versions)
            .zip(optional)
            .map(|((name, version), optional)| settings::UsesSdkLibrary {
                name: name.clone(),
                version_major: *version,
                optional: *optional,
            })
            .collect();
        let static_versions = parsed
            .uses_static_libraries_versions
            .as_deref()
            .unwrap_or_default();
        if static_versions.len() != parsed.uses_static_libraries.len() {
            return Err("Live static library metadata arrays disagree".into());
        }
        let static_libraries = parsed
            .uses_static_libraries
            .iter()
            .cloned()
            .zip(static_versions.iter().copied())
            .collect();
        let modified = apks.scan_file_time(&code.package)?;
        Ok(scan::SettingMetadata {
            code_path: destination.into(),
            legacy_native_library_path: old.and_then(|old| old.legacy_native_library_path.clone()),
            primary_cpu_abi: old.and_then(|old| old.primary_cpu_abi.clone()),
            secondary_cpu_abi: old.and_then(|old| old.secondary_cpu_abi.clone()),
            version_code: ((parsed.version_code_major as i64) << 32)
                | parsed.version_code as u32 as i64,
            flags,
            private_flags,
            last_modified_time: modified,
            uses_sdk_libraries: sdk,
            uses_static_libraries: static_libraries,
            mime_groups: parsed.mime_groups.clone(),
            domain_set_id: uuid(),
            target_sdk_version: parsed.target_sdk_version,
            restrict_update_hash: old
                .and_then(|old| old.restrict_update_hash.clone())
                .or(parsed.restrict_update_hash.clone()),
        })
    })
}
/// Standard-session install source policy from InstallPackageHelper. The image's
/// headless-user mode is captured by its boot owner rather than read from macOS.
pub fn install_source(headless_system_user: bool) -> InstallSource {
    Arc::new(move |code: &VerifiedCode, base: &Snapshot| {
        let owner = base.owner();
        let record = &code.record;
        let name = internal_name(code);
        let old = owner
            .settings
            .packages
            .iter()
            .find(|package| package.name == name);
        let old_update = old.and_then(|package| package.install_source.update_owner.as_deref());
        let current_user = if record.installer_package_uid >= 0 {
            record.installer_package_uid / 100000
        } else {
            record.user as i32
        };
        let installed = old.is_some_and(|old| {
            owner.scanned_user_states(&old.name).is_some_and(|users| {
                if current_user >= 0 {
                    users
                        .get(&current_user)
                        .is_some_and(|state| state.installed)
                } else {
                    users.values().filter(|state| !state.installed).count()
                        <= usize::from(headless_system_user)
                }
            })
        });
        let requested = record.params.install_flags & (1 << 25) != 0;
        let mut update_owner = if requested {
            record.installer_package.clone()
        } else {
            None
        };
        let same = old_update == record.installer_package.as_deref();
        let deny = owner
            .update_ownership
            .is_denylisted(&name)
            .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        let provider = owner
            .update_ownership
            .is_provider(update_owner.as_deref())
            .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        if code.session.parameters.install_flags & 0x20000 == 0 {
            if !installed {
                if !requested
                    || deny
                    || provider
                    || old.is_some() && old_update.is_none()
                    || old_update.is_some() && !same
                {
                    update_owner = None;
                }
            } else if !same || old_update.is_none() {
                update_owner = None;
            }
        }
        let initiating = record.initiating_package.clone();
        let signatures = initiating
            .as_deref()
            .filter(|name| *name != "com.android.shell")
            .and_then(|name| {
                owner
                    .settings
                    .packages
                    .iter()
                    .find(|package| package.name == name)
            })
            .and_then(|package| package.signatures.clone());
        Ok(settings::InstallSource {
            installer: record.installer_package.clone(),
            installer_uid: record.installer_package_uid,
            update_owner,
            installer_attribution_tag: record.installer_attribution_tag.clone(),
            package_source: record.params.package_source,
            is_orphaned: false,
            initiating_package: initiating,
            initiating_package_uninstalled: false,
            initiating_package_signatures: signatures,
            originating_package: record.originating_package.clone(),
        })
    })
}
pub struct LibrarySettings {
    pub page_size: u64,
    pub compat_16kb_disabled: bool,
}
pub fn library_policy(settings: LibrarySettings) -> Result<LibraryPolicy, String> {
    if settings.page_size < 4096 || !settings.page_size.is_power_of_two() {
        return Err("Invalid actual guest page size".into());
    }
    Ok(Arc::new(move |code| {
        Ok(scan::NativeLibraryInstallPolicy {
            page_size: settings.page_size,
            extract: code.package.is(booleans::EXTRACT_NATIVE_LIBS),
            debuggable: code.package.is(booleans::DEBUGGABLE),
            compat_16kb_disabled: settings.compat_16kb_disabled,
            manifest_compat_disabled: code.package.page_size_app_compat_flags == 64,
        })
    }))
}
/// Kernel-owned timezone supplies the UTC offset at the decoded local instant.
/// This preserves historical DST changes and never reads the host's timezone.
pub type GuestUtcOffset =
    Arc<dyn Fn(i32, u32, u32, u32, u32, u32) -> Result<i32, String> + Send + Sync>;
pub fn zip_clock(offset: GuestUtcOffset) -> ZipClock {
    Arc::new(move |packed| {
        let year = 1980 + ((packed >> 25) & 127) as i32;
        let month = (packed >> 21) & 15;
        let day = (packed >> 16) & 31;
        let hour = (packed >> 11) & 31;
        let minute = (packed >> 5) & 63;
        let second = (packed & 31) * 2;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err("Invalid ZIP DOS local timestamp".into());
        }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let limit = match month {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day > limit {
            return Err("Invalid ZIP DOS calendar day".into());
        }
        let y = year - i32::from(month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let shifted = month as i32 + if month > 2 { -3 } else { 9 };
        let doy = (153 * shifted + 2) / 5 + day as i32 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = (era as i64) * 146097 + doe as i64 - 719468;
        let local = days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;
        let utc = local - offset(year, month, day, hour, minute, second)? as i64;
        if utc >= 0 {
            UNIX_EPOCH.checked_add(Duration::from_secs(utc as u64))
        } else {
            UNIX_EPOCH.checked_sub(Duration::from_secs(utc.unsigned_abs()))
        }
        .ok_or_else(|| "ZIP timestamp exceeds kernel clock range".into())
    })
}

impl super::environment::Config {
    /// Install every native producer together from the same image/kernel capture.
    pub fn install_native_producers(
        mut self,
        headless_system_user: bool,
        library: LibrarySettings,
        timezone: GuestUtcOffset,
    ) -> Result<Self, String> {
        self.metadata = metadata(self.apks.clone());
        self.install_source = install_source(headless_system_user);
        self.library_policy = library_policy(library)?;
        self.zip_clock = zip_clock(timezone);
        Ok(self)
    }
}

pub struct GuestSettings {
    pub library: LibrarySettings,
    pub headless_system_user: bool,
    pub timezone: String,
}
/// Connect the existing per-bootstrap typed policy bridge to actual guest
/// properties, page size, timezone database and native library compatibility.
pub fn configure_guest_sources(
    mut config: super::environment::Config,
    external: Arc<super::preapproval::BridgeOwner>,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    system_config: &crate::package::system_config::SystemConfig,
    properties: &dyn Fn(&str) -> Option<String>,
) -> Result<super::environment::Config, String> {
    let guest = external
        .native_install_environment()
        .map_err(|error| error.message)?;
    config.library_compatibility = Arc::new(
        bridge
            .library_compatibility(system_config, properties)
            .map_err(|error| format!("Actual image library compatibility: {error:?}"))?,
    );
    let compat = bridge.clone();
    config.remove_test_base = Arc::new(move |package, system| {
        compat
            .remove_test_base(package, system)
            .map_err(|error| format!("Actual test.base compatibility: {error:?}"))
    });
    config.compatibility = bridge;
    let timezone = Arc::new(move |year, month, day, hour, minute, second| {
        external
            .zip_local_utc_offset(year, month, day, hour, minute, second)
            .map_err(|error| error.message)
    });
    config.install_native_producers(guest.headless_system_user, guest.library, timezone)
}

/// Reuse the same native System installation lock and installd endpoint for
/// committed old-code cleanup; never instantiate a second service namespace.
pub fn code_resources(
    system: Arc<crate::system::System>,
    data: std::path::PathBuf,
    parser_cache: Option<std::path::PathBuf>,
) -> Result<Arc<crate::package::owner::resources::CodeResources>, String> {
    let data = std::fs::canonicalize(data).map_err(|error| error.to_string())?;
    if !data.is_dir() {
        return Err("Actual writable installation data root is absent".into());
    }
    Ok(Arc::new(
        crate::package::owner::resources::CodeResources::with_system(system, data, parser_cache),
    ))
}
