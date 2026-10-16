//! The packages the app host knows: PackageManager's persisted state
//! (`packages.xml`, `packages.list`, user 0's restrictions) joined with
//! each package's parse, as the scan would join them, into the model the
//! native queries answer from (`package::model::State`).
//!
//! Only what an app's start reads is joined: the package's code, its
//! app id, ABI and native library directories, its seinfo and user 0's
//! state. Shared libraries, overlays, permissions and the apps filter's
//! inputs are not (#1426).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::package::model::{PackageState, PackageUserState};
use crate::package::parse::{self, Platform};
use crate::package::pkg::{AndroidPackage, booleans};
use crate::package::scan::abi::{NativeLibraryEnvironment, NativeLibraryPaths};
use crate::package::{self, info, model, restrictions, settings, system_config};

/// `ApplicationInfo.PRIVATE_FLAG_*` of the partitions.
const PRIVATE_FLAG_OEM: i32 = 1 << 17;
const PRIVATE_FLAG_VENDOR: i32 = 1 << 18;
const PRIVATE_FLAG_PRODUCT: i32 = 1 << 19;
const PRIVATE_FLAG_SYSTEM_EXT: i32 = 1 << 21;
const PRIVATE_FLAG_ODM: i32 = 1 << 30;
/// `Build.SUPPORTED_ABIS[0]` of the image, and `Environment
/// .getDataAppLibDirectory`'s path for 32-bit libraries.
const PREFERRED_ABI: &str = "arm64-v8a";
const APP_LIB32_INSTALL_DIR: &str = "/data/app-lib";

pub struct Packages {
    state: model::State,
    platform: Platform,
}

impl Packages {
    /// The packages of the guest's `/data` at `data`, their code read
    /// below the image `root` or `data`, on a display of `density_dpi`.
    pub fn load(
        root: &Path,
        data: &Path,
        density_dpi: i32,
        property: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let persisted = package::State::read(data, &[0])?
            .ok_or("no package state: Android never booted on this data")?;
        let mut platform = Platform::load(root, Default::default())?;
        platform.density_dpi = Some(density_dpi);
        let restrictions = persisted
            .users
            .iter()
            .find(|(user, _)| *user == 0)
            .map(|(_, user)| &user.restrictions);
        let access = persisted
            .users
            .iter()
            .find(|(user, _)| *user == 0)
            .and_then(|(_, user)| user.access.as_ref())
            .ok_or("no permission state of user 0")?;
        let mut packages = BTreeMap::new();
        for setting in &persisted.settings.packages {
            let listed = persisted.list.iter().find(|e| e.name == setting.name);
            let user = restrictions
                .and_then(|r| r.packages.iter().find(|(name, _)| *name == setting.name))
                .map(|(_, state)| state);
            let host = host_path(root, data, &setting.code_path)?;
            let granted: Vec<String> = access
                .app_id_permissions
                .iter()
                .find(|(app_id, _)| *app_id == setting.app_id)
                .map(|(_, permissions)| {
                    permissions
                        .iter()
                        .filter(|(_, flags)| permission_granted(*flags))
                        .map(|(name, _)| name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let mut ps = join(setting, listed, user, &host, &platform)
                .map_err(|e| format!("{}: {e}", setting.name))?;
            ps.users.get_mut(&0).unwrap().granted_permissions = granted;
            packages.insert(setting.name.clone(), ps);
        }
        let mut users = BTreeMap::new();
        users.insert(
            0,
            model::User {
                id: 0,
                unlocking_or_unlocked: true,
                ..Default::default()
            },
        );
        let system = system_config::system(root, property, &system_config::Framework::load(root)?)?;
        Ok(Packages {
            state: model::State {
                packages,
                users,
                system,
                ..Default::default()
            },
            platform,
        })
    }

    pub fn state(&self) -> &model::State {
        &self.state
    }

    /// The framework's integer resource `name`, as its overlays leave it.
    pub fn framework_integer(&self, name: &str) -> Option<i32> {
        use crate::package::parse::resources::{
            Resources, Selected, TYPE_FIRST_INT, TYPE_REFERENCE,
        };
        let id = self.platform.framework.id("integer", name)?;
        let resources = Resources {
            tables: vec![&self.platform.framework],
            overlays: &self.platform.framework_overlays,
            config: self.platform.config(),
        };
        let mut value = Selected {
            kind: TYPE_REFERENCE,
            data: id,
            table: None,
            resid: 0,
            flags: 0,
        };
        resources.resolve(&mut value);
        (value.kind >= TYPE_FIRST_INT).then_some(value.data as i32)
    }

    /// `getApplicationInfo(name, flags, 0)`.
    pub fn application_info(&self, name: &str, flags: i64) -> Option<info::ApplicationInfo> {
        let ps = self.state.packages.get(name)?;
        let target = info::Target {
            sys: &self.state.system,
            pkg: ps.pkg.as_deref()?,
            ps,
            state: ps.users.get(&0)?,
            user: 0,
        };
        info::generate_application_info(&target, flags)
    }
}

/// `PermissionFlags.isPermissionGranted` (services/permission at
/// `android-16.0.0_r1`).
fn permission_granted(flags: i32) -> bool {
    const INSTALL_GRANTED: i32 = 1 << 0;
    const INSTALL_REVOKED: i32 = 1 << 1;
    const PROTECTION_GRANTED: i32 = 1 << 2;
    const RUNTIME_GRANTED: i32 = 1 << 4;
    const LEGACY_GRANTED: i32 = 1 << 10;
    const IMPLICIT_GRANTED: i32 = 1 << 11;
    const RESTRICTION_REVOKED: i32 = 1 << 18;
    let has = |bits: i32| flags & bits == bits;
    if has(INSTALL_GRANTED) {
        return true;
    }
    if has(INSTALL_REVOKED) {
        return false;
    }
    if has(PROTECTION_GRANTED) || has(LEGACY_GRANTED) || has(IMPLICIT_GRANTED) {
        return true;
    }
    if has(RESTRICTION_REVOKED) {
        return false;
    }
    has(RUNTIME_GRANTED)
}

/// Where the guest's `path` is on the host: below `data` for `/data`,
/// else below the image.
fn host_path(root: &Path, data: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = path
        .strip_prefix('/')
        .ok_or_else(|| format!("relative code path {path}"))?;
    Ok(match relative.strip_prefix("data/") {
        Some(rest) => data.join(rest),
        None => root.join(relative),
    })
}

/// One package as the scan leaves it: parsed, with the setting's
/// identity, ABI and partition applied.
fn join(
    setting: &settings::Package,
    listed: Option<&package::list::Entry>,
    user: Option<&restrictions::UserState>,
    host: &Path,
    platform: &Platform,
) -> Result<PackageState, String> {
    let system = setting.flags & settings::FLAG_SYSTEM != 0;
    let parse_flags = if system {
        parse::PARSE_IS_SYSTEM_DIR
    } else {
        0
    };
    let parsed = parse::parse(host, &setting.code_path, parse_flags, platform)
        .map_err(|e| format!("parse: {e:?}"))?;
    let mut pkg = AndroidPackage::read_cache_entry(&parsed.to_cache_entry().bytes)
        .map_err(|e| format!("parsed package does not read: {e}"))?;
    pkg.uid = setting.app_id;
    pkg.primary_cpu_abi = setting.primary_cpu_abi.clone();
    pkg.secondary_cpu_abi = setting.secondary_cpu_abi.clone();
    let seinfo = listed.map(|l| l.seinfo.clone());
    for (set, flag) in [
        (system, booleans::SYSTEM),
        (
            setting.private_flags & settings::PRIVATE_FLAG_PRIVILEGED != 0,
            booleans::PRIVILEGED,
        ),
        (setting.private_flags & PRIVATE_FLAG_OEM != 0, booleans::OEM),
        (
            setting.private_flags & PRIVATE_FLAG_VENDOR != 0,
            booleans::VENDOR,
        ),
        (
            setting.private_flags & PRIVATE_FLAG_PRODUCT != 0,
            booleans::PRODUCT,
        ),
        (
            setting.private_flags & PRIVATE_FLAG_SYSTEM_EXT != 0,
            booleans::SYSTEM_EXT,
        ),
        (setting.private_flags & PRIVATE_FLAG_ODM != 0, booleans::ODM),
        (
            seinfo
                .as_deref()
                .is_some_and(|s| s == "platform" || s.starts_with("platform:")),
            booleans::SIGNED_WITH_PLATFORM_KEY,
        ),
    ] {
        if set {
            pkg.booleans |= flag;
        }
    }
    let updated_system = false;
    NativeLibraryPaths::derive(
        &pkg,
        &NativeLibraryEnvironment {
            preferred_abi: PREFERRED_ABI,
            app_lib32_install_dir: APP_LIB32_INSTALL_DIR,
            code_is_directory: host.is_dir(),
            canonical_source: None,
        },
        system,
        updated_system,
    )?
    .apply(&mut pkg);
    let mut ps = PackageState {
        name: setting.name.clone(),
        app_id: setting.app_id,
        path: setting.code_path.clone(),
        volume_uuid: setting.volume_uuid.clone(),
        primary_cpu_abi: setting.primary_cpu_abi.clone(),
        secondary_cpu_abi: setting.secondary_cpu_abi.clone(),
        cpu_abi_override: setting.cpu_abi_override.clone(),
        seinfo,
        version_code: setting.version_code,
        target_sdk_version: setting.target_sdk_version,
        last_modified_time: setting.last_modified_time,
        last_update_time: setting.last_update_time,
        ..Default::default()
    };
    ps.is.system = system;
    ps.is.privileged = pkg.is(booleans::PRIVILEGED);
    let user = user.ok_or("no user 0 state")?;
    ps.users.insert(
        0,
        PackageUserState {
            ce_data_inode: user.ce_data_inode,
            de_data_inode: user.de_data_inode,
            installed: user.installed,
            stopped: user.stopped,
            not_launched: user.not_launched,
            hidden: user.hidden,
            instant_app: user.instant_app,
            virtual_preload: user.virtual_preload,
            enabled: user.enabled,
            install_reason: user.install_reason,
            first_install_time: user.first_install_time,
            data_exists: true,
            ..Default::default()
        },
    );
    ps.pkg = Some(Arc::new(pkg));
    Ok(ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_follow_permission_flags() {
        assert!(permission_granted(1));
        assert!(!permission_granted(1 << 1 | 1 << 4));
        assert!(permission_granted(1 << 4));
        assert!(!permission_granted(1 << 18 | 1 << 4));
        assert!(!permission_granted(1 << 5));
    }

    #[test]
    fn code_paths_resolve_below_data_or_the_image() {
        let (root, data) = (Path::new("/img"), Path::new("/d"));
        assert_eq!(
            host_path(root, data, "/data/app/~~a/b").unwrap(),
            Path::new("/d/app/~~a/b")
        );
        assert_eq!(
            host_path(root, data, "/system/app/X").unwrap(),
            Path::new("/img/system/app/X")
        );
        assert!(host_path(root, data, "app").is_err());
    }
}
