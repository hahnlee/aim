//! The packages the app host knows: PackageManager's persisted state
//! (`packages.xml`, `packages.list`, user 0's restrictions) joined with
//! each package's parse, as the scan would join them, into the model the
//! native queries answer from (`package::model::State`).
//!
//! Only what an app's start reads is joined: the package's code, its
//! app id, ABI and native library directories, its seinfo, signatures
//! and shared user, and user 0's state. Shared libraries, overlays,
//! permissions and the apps filter's inputs are not (#1426).

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
    /// User 0's permission and app op state (`access.abx`).
    access: Option<package::permissions::AccessUser>,
    /// The permissions defined (`access.abx`'s system part).
    permissions: Vec<package::permissions::Permission>,
}

impl Packages {
    /// The packages of the guest's `/data` at `data`, their code read
    /// below the image `root` or `data`, on a display of `density_dpi`.
    pub fn load(
        root: &Path,
        data: &Path,
        density_dpi: i32,
        locale: &str,
        property: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let persisted = package::State::read(data, &[0])?
            .ok_or("no package state: Android never booted on this data")?;
        let mut platform = Platform::load(root, Default::default())?;
        platform.density_dpi = Some(density_dpi);
        // The device's locale, as `persist.sys.locale` holds it (`ko-KR`).
        if let Some((language, country)) = locale.split_once('-')
            && let (Ok(language), Ok(country)) = (
                <[u8; 2]>::try_from(language.as_bytes()),
                <[u8; 2]>::try_from(country.as_bytes()),
            )
        {
            platform.locale = (language, country);
        }
        let restrictions = persisted
            .users
            .iter()
            .find(|(user, _)| *user == 0)
            .map(|(_, user)| &user.restrictions);
        let access = persisted
            .users
            .iter()
            .find(|(user, _)| *user == 0)
            .and_then(|(_, user)| user.access.clone())
            .ok_or("no permission state of user 0")?;
        // Each package's parse, on as many threads as there are cores, as
        // `ParallelPackageParser` parses.
        let joined = crate::package::scan::parse_all(&persisted.settings.packages, |setting| {
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
            Ok::<_, String>((setting.name.clone(), ps))
        });
        let mut packages = joined.into_iter().collect::<Result<BTreeMap<_, _>, _>>()?;
        // The shared users, with their packages and signatures
        // (`SharedUserSetting`).
        let mut shared_users = BTreeMap::new();
        for su in &persisted.settings.shared_users {
            let members: Vec<String> = persisted
                .settings
                .packages
                .iter()
                .filter(|p| p.shared_app_id() == Some(su.app_id))
                .map(|p| p.name.clone())
                .collect();
            for name in &members {
                if let Some(ps) = packages.get_mut(name) {
                    ps.shared_user = Some(su.name.clone());
                }
            }
            shared_users.insert(
                su.name.clone(),
                model::SharedUser {
                    name: su.name.clone(),
                    app_id: su.app_id,
                    flags: su.flags,
                    packages: members,
                    signatures: su.signatures.clone(),
                    ..Default::default()
                },
            );
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
        let permissions = persisted.access.as_ref().map(|a| a.permissions.clone()).unwrap_or_default();
        let mut system =
            system_config::system(root, property, &system_config::Framework::load(root)?)?;
        system.sdk_sandbox_package = Some(Some(sdk_sandbox_package(&packages)?));
        // `<protected-broadcast>` counts in system packages only
        // (`ScanPackageUtils.applyPolicy` clears the others').
        let protected_broadcasts = packages
            .values()
            .filter(|ps| ps.is.system)
            .filter_map(|ps| ps.pkg.as_ref())
            .flat_map(|pkg| pkg.protected_broadcasts.iter().cloned())
            .collect();
        Ok(Packages {
            state: model::State {
                packages,
                shared_users,
                users,
                system,
                protected_broadcasts: Some(protected_broadcasts),
                ..Default::default()
            },
            platform,
            access: Some(access),
            permissions,
        })
    }

    pub fn state(&self) -> &model::State {
        &self.state
    }

    fn framework_resources(&self) -> crate::package::parse::resources::Resources<'_> {
        crate::package::parse::resources::Resources {
            tables: vec![&self.platform.framework],
            overlays: &self.platform.framework_overlays,
            config: self.platform.config(),
        }
    }

    /// The framework's resource `kind/name`, resolved as its overlays and
    /// the device's locale leave it.
    fn framework_value(
        &self,
        kind: &str,
        name: &str,
    ) -> Option<crate::package::parse::resources::Selected> {
        use crate::package::parse::resources::{Selected, TYPE_REFERENCE};
        let id = self.platform.framework.id(kind, name)?;
        let mut value = Selected {
            kind: TYPE_REFERENCE,
            data: id,
            table: None,
            resid: 0,
            flags: 0,
        };
        self.framework_resources().resolve(&mut value);
        Some(value)
    }

    /// The framework's integer resource `name`.
    pub fn framework_integer(&self, name: &str) -> Option<i32> {
        let value = self.framework_value("integer", name)?;
        (value.kind >= crate::package::parse::resources::TYPE_FIRST_INT)
            .then_some(value.data as i32)
    }

    /// The framework's boolean resource `name` (`Resources.getBoolean`:
    /// any integer, nonzero is true).
    pub fn framework_bool(&self, name: &str) -> Option<bool> {
        let value = self.framework_value("bool", name)?;
        (value.kind >= crate::package::parse::resources::TYPE_FIRST_INT).then_some(value.data != 0)
    }

    /// The framework's float resource `name` (`<item format="float"
    /// type="dimen">`).
    pub fn framework_float(&self, name: &str) -> Option<f32> {
        let value = self.framework_value("dimen", name)?;
        (value.kind == crate::package::parse::resources::TYPE_FLOAT)
            .then_some(f32::from_bits(value.data))
    }

    /// The framework's dimension resource `name` given in dp
    /// (`TypedValue.complexToFloat` of a `COMPLEX_UNIT_DIP` dimension).
    pub fn framework_dimension_dp(&self, name: &str) -> Option<f32> {
        const COMPLEX_UNIT_DIP: u32 = 1;
        const RADIX_MULTS: [f32; 4] = [1.0 / 256.0, 1.0 / 32768.0, 1.0 / 8388608.0, 1.0 / 2147483648.0];
        let value = self.framework_value("dimen", name)?;
        (value.kind == crate::package::parse::resources::TYPE_DIMENSION && value.data & 0xf == COMPLEX_UNIT_DIP)
            .then(|| (value.data & 0xffff_ff00) as i32 as f32 * RADIX_MULTS[((value.data >> 4) & 3) as usize])
    }

    /// The file of the framework's resource `kind/name` (`openRawResource`,
    /// `getXml`'s compiled XML) in the image whose root is `root`.
    pub fn framework_file(&self, root: &Path, kind: &str, name: &str) -> Option<Vec<u8>> {
        self.platform.framework_file(root, kind, name)
    }

    /// The framework's string resource `name` in the device's locale.
    pub fn framework_string(&self, name: &str) -> Option<String> {
        let id = self.platform.framework.id("string", name)?;
        self.framework_resources().resource_string(id)
    }

    /// The permissions defined.
    pub fn permissions(&self) -> &[package::permissions::Permission] {
        &self.permissions
    }

    /// User 0's permission and app op state.
    pub fn access(&self) -> Option<&package::permissions::AccessUser> {
        self.access.as_ref()
    }

    /// SystemConfig's split permissions.
    pub fn split_permissions(&self) -> &[parse::SplitPermission] {
        &self.platform.split_permissions
    }

    /// `ComponentInfo.loadXmlMetaData`'s parser and the attributes of
    /// `getResourcesForApplication`: the compiled XML resource `id` of
    /// `pkg`, from its APKs below the image `root` or the guest's `/data`
    /// at `data`, its root element given to `read` with the package's
    /// resources (its own and the framework's; overlays of the package
    /// are not applied, #1426).
    pub fn xml_resource<T>(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        id: u32,
        read: impl FnOnce(&aim_apps::res::Element, &parse::attrs::Ctx<'_>) -> T,
    ) -> Result<T, String> {
        let base = pkg.base_apk_path.as_deref().ok_or("no base APK")?;
        let mut paths = vec![base];
        for split in pkg.split_code_paths.iter().flatten() {
            paths.push(split.as_deref().ok_or("a null split path")?);
        }
        let apks = paths
            .iter()
            .map(|path| {
                let host = host_path(root, data, path)?;
                aim_apps::apk::Apk::open(&host).map_err(|e| format!("{}: {e}", host.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tables = apks
            .iter()
            .map(|apk| {
                apk.file_if_present("resources.arsc")
                    .map_err(|e| e.to_string())?
                    .map(|b| parse::resources::Table::parse(&b).map_err(|e| e.to_string()))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Each table's APK: the framework's first, then the package's.
        let mut table_apks = vec![None];
        let mut resources = vec![&self.platform.framework];
        for (i, table) in tables.iter().enumerate() {
            if let Some(table) = table {
                resources.push(table);
                table_apks.push(Some(i));
            }
        }
        let resources = parse::resources::Resources {
            tables: resources,
            overlays: &self.platform.framework_overlays,
            config: self.platform.config(),
        };
        let (table, file) = resources
            .resource_string_source(id)
            .ok_or_else(|| format!("no XML resource {id:#x}"))?;
        let bytes = match table_apks.get(table).ok_or("an XML resource of an overlay")? {
            Some(i) => apks[*i].file(file),
            None => aim_apps::apk::Apk::open(&root.join("system/framework/framework-res.apk"))
                .and_then(|apk| apk.file(file)),
        }
        .map_err(|e| e.to_string())?;
        let document = aim_apps::res::xml(&bytes).map_err(|e| e.to_string())?;
        let ctx = parse::attrs::Ctx {
            res: &resources,
            attrs: &self.platform.framework_attrs,
            error: Default::default(),
        };
        let value = read(&document, &ctx);
        match ctx.error.take() {
            Some(e) => Err(e),
            None => Ok(value),
        }
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

/// `SdkSandboxManagerLocal.SERVICE_INTERFACE`.
const SDK_SANDBOX_SERVICE: &str = "com.android.sdksandbox.SdkSandboxService";

/// `PackageManagerService.getRequiredSdkSandboxPackageName`: the one
/// installed, enabled system package with an enabled service for the SDK
/// sandbox's action.
fn sdk_sandbox_package(packages: &BTreeMap<String, PackageState>) -> Result<String, String> {
    // `COMPONENT_ENABLED_STATE_DISABLED`, `_DISABLED_USER`,
    // `_DISABLED_UNTIL_USED`; `_DEFAULT`.
    const DISABLED: [i32; 3] = [2, 3, 4];
    const DEFAULT: i32 = 0;
    let mut found = Vec::new();
    for ps in packages.values().filter(|ps| ps.is.system) {
        let (Some(pkg), Some(state)) = (ps.pkg.as_deref(), ps.users.get(&0)) else {
            continue;
        };
        if !state.installed || state.hidden || DISABLED.contains(&state.enabled) {
            continue;
        }
        if state.enabled == DEFAULT && !pkg.is(booleans::ENABLED) {
            continue;
        }
        let matches = pkg.services.iter().any(|service| {
            let name = &service.main.component.name;
            let enabled = state.enabled_components.contains(name)
                || (service.main.enabled && !state.disabled_components.contains(name));
            enabled
                && service
                    .main
                    .component
                    .intents
                    .iter()
                    .any(|i| i.filter.actions.iter().any(|a| a == SDK_SANDBOX_SERVICE))
        });
        if matches {
            found.push(ps.name.clone());
        }
    }
    match <[String; 1]>::try_from(found) {
        Ok([name]) => Ok(name),
        Err(found) => Err(format!(
            "required SDK sandbox service matches: {}",
            found.len()
        )),
    }
}

/// `PermissionFlags.isPermissionGranted` (services/permission at
/// `android-16.0.0_r1`).
pub(super) fn permission_granted(flags: i32) -> bool {
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
        signatures: setting.signatures.clone(),
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
            enabled_components: user.enabled_components.clone().unwrap_or_default(),
            disabled_components: user.disabled_components.clone().unwrap_or_default(),
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
