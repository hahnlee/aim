//! The packages the app host knows: PackageManager's persisted state
//! (`packages.xml`, `packages.list`, user 0's restrictions) joined with
//! each package's parse, as the scan would join them, into the model the
//! native queries answer from (`package::model::State`).
//!
//! Only what an app's start reads is joined: the package's code, its
//! app id, ABI and native library directories, its seinfo, signatures
//! and shared user, user 0's state, and the apps filter's device
//! configuration (`config_forceSystemPackagesQueryable`,
//! `config_forceQueryablePackages`). Shared libraries, overlays and
//! permissions are not (#1426).

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
    domains: DomainInputs,
}

/// What DomainVerificationService attaches the packages with at boot
/// (`addPackage`): the persisted state, each package's identity in the
/// scan's order, and the system config (its linked apps).
pub struct DomainInputs {
    pub state: package::domain_verification::State,
    pub legacy_info: BTreeMap<String, i32>,
    pub packages: Vec<DomainPackage>,
    pub config: system_config::SystemConfig,
}

/// A package's domain set id, signers and whether it is a system one.
pub struct DomainPackage {
    pub name: String,
    pub id: Option<String>,
    pub signatures: Option<Vec<Vec<u8>>>,
    pub system: bool,
}

/// `ActivityRecord.WindowStyle`: what an activity's theme says of its
/// window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowStyle {
    pub translucent: bool,
    pub floating: bool,
    pub show_wallpaper: bool,
    pub no_display: bool,
}

impl WindowStyle {
    /// `mOccludesParent` of an activity of this style: not translucent
    /// or floating, or showing the wallpaper.
    pub fn occludes_parent(&self) -> bool {
        !(self.translucent || self.floating) || self.show_wallpaper
    }
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
        enforce_native_dependencies: &dyn Fn(&str, i32) -> bool,
        apex: &package::bootstrap::ApexInventory,
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
        // `scanApexPackages`: the containers ApexManager reports, parsed
        // with the system's parse flags (`mSystemParseFlags`) and
        // registered before the APKs.
        let apks = package::write::Apks {
            signing_overrides: None,
            files: {
                let (root, data) = (root.to_owned(), data.to_owned());
                Box::new(move |path| host_path(&root, &data, path).ok())
            },
            platform,
        };
        let config = system_config::SystemConfig::read(root, property);
        let image = package::scan::ApexImage::load(&apks, apex, parse::PARSE_IS_SYSTEM_DIR)
            .map_err(|e| format!("{}: {}", e.path, e.message))?;
        let mut containers = Vec::new();
        for code in &image.packages {
            let (pkg, flags, private_flags, stopped) =
                package::scan::registered_apex(code, &persisted.settings, &config, &apks)
                    .map_err(|e| format!("{}: {e}", code.info.module_path))?;
            let pkg = Arc::new(pkg);
            containers.push((
                code,
                pkg.clone(),
                join_apex(code, pkg, flags, private_flags, stopped)?,
            ));
        }
        let platform = apks.platform;
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
        for (_, _, ps) in &containers {
            if packages.insert(ps.name.clone(), ps.clone()).is_some() {
                return Err(format!("{}: a container and an APK of one name", ps.name));
            }
        }
        // `SharedLibrariesImpl`: the system's libraries, then those the
        // packages declare in their scan's order, and each package's
        // dependencies resolved (`updateAllSharedLibrariesLPw`), native
        // ones as `ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES` asks.
        let config = system_config::SystemConfig::read(root, property);
        let mut libraries = package::libraries::Registry::new(&config);
        for setting in &persisted.settings.packages {
            if let Some(ps) = packages.get(&setting.name) {
                libraries
                    .add_package(ps, None)
                    .map_err(|e| format!("{}: shared libraries: {}", setting.name, e.0))?;
            }
        }
        let resolved = libraries
            .resolve(&packages, &|ps| {
                let target_sdk = ps.pkg.as_ref().map_or(0, |pkg| pkg.target_sdk_version);
                Ok(package::libraries::Policy::pinned(
                    enforce_native_dependencies(&ps.name, target_sdk),
                ))
            })
            .map_err(|e| format!("{}: shared libraries: {:?}", e.package, e.cause))?;
        let shared_libraries = resolved.registry.entries().cloned().collect();
        let mut packages = resolved.packages;
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
                ..Default::default()
            },
        );
        let permissions = persisted
            .access
            .as_ref()
            .map(|a| a.permissions.clone())
            .unwrap_or_default();
        let mut system =
            system_config::system(root, property, &system_config::Framework::load(root)?)?;
        system.sdk_sandbox_package = Some(Some(sdk_sandbox_package(&packages)?));
        // The apps filter's device configuration (`AppsFilterImpl.create`).
        (
            system.force_system_packages_queryable,
            system.force_queryable_packages,
        ) = package::scan_snapshot::boot_context::apps_filter_config(&platform, platform.config())?;
        // `<protected-broadcast>` counts in system packages only
        // (`ScanPackageUtils.applyPolicy` clears the others').
        let protected_broadcasts = packages
            .values()
            .filter(|ps| ps.is.system)
            .filter_map(|ps| ps.pkg.as_ref())
            .flat_map(|pkg| pkg.protected_broadcasts.iter().cloned())
            .collect();
        let mut state = model::State {
            packages,
            shared_users,
            users,
            system,
            shared_libraries: Some(shared_libraries),
            protected_broadcasts: Some(protected_broadcasts),
            apex_inventory: Some(apex.clone()),
            ..Default::default()
        };
        // `ModuleInfoProvider.systemReady`: the modules of the metadata
        // provider, over the containers and the APKs in them
        // (`ApexManager`'s links, the APKs in their settings' order).
        let results: Vec<_> = containers
            .iter()
            .map(|(code, pkg, _)| package::scan::ApexScanResult {
                info: code.info.clone(),
                package: (**pkg).clone(),
                signing: code.signing.clone(),
            })
            .collect();
        let registrations: Vec<AndroidPackage> = persisted
            .settings
            .packages
            .iter()
            .filter(|setting| setting.code_path.starts_with("/apex/"))
            .filter_map(|setting| state.packages.get(&setting.name)?.pkg.as_deref().cloned())
            .collect();
        let links = package::module_metadata::ApexLinks::from_scan(&results, apex, &registrations)?;
        // A provider the owner cannot represent leaves the module queries
        // failing as not modelled.
        match package::module_metadata::Owner::load(
            &state,
            &platform,
            platform.config(),
            &|path| host_path(root, data, path),
            &links,
        ) {
            Ok(modules) => state.system.module_metadata = Some(Arc::new(modules)),
            Err(e) => eprintln!("app host: module metadata: {e}"),
        }
        // `PackageManagerService`'s `mRequiredPermissionControllerPackage`.
        state.system.permission_controller_package =
            Some(package::scan_snapshot::query_state::select_permission_controller(&state)?);
        Ok(Packages {
            state,
            platform,
            access: Some(access),
            permissions,
            domains: DomainInputs {
                packages: persisted
                    .settings
                    .packages
                    .iter()
                    .map(|setting| DomainPackage {
                        name: setting.name.clone(),
                        id: setting.domain_set_id.clone(),
                        signatures: setting.signatures.as_ref().map(|s| s.signatures.clone()),
                        system: setting.flags & settings::FLAG_SYSTEM != 0,
                    })
                    .collect(),
                state: persisted.settings.domain_verification,
                legacy_info: persisted.settings.legacy_domain_info,
                config,
            },
        })
    }

    pub fn state(&self) -> &model::State {
        &self.state
    }

    pub fn domains(&self) -> &DomainInputs {
        &self.domains
    }

    /// What the APK signature verifier depends on of the platform.
    pub fn signing_build(&self) -> package::sign::Build {
        package::sign::Build::of(&self.platform)
    }

    /// A package context's resources (`createPackageContext`): the
    /// framework's, then the package's `table`, in the device's
    /// configuration.
    pub fn package_resources<'a>(
        &'a self,
        table: &'a crate::package::parse::resources::Table,
    ) -> crate::package::parse::resources::Resources<'a> {
        crate::package::parse::resources::Resources {
            tables: vec![&self.platform.framework, table],
            overlays: &self.platform.framework_overlays,
            config: self.platform.config(),
        }
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

    /// The id of the framework's resource `kind/name`: its `R` constant.
    pub fn framework_id(&self, kind: &str, name: &str) -> Option<u32> {
        self.platform.framework.id(kind, name)
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
        const RADIX_MULTS: [f32; 4] = [
            1.0 / 256.0,
            1.0 / 32768.0,
            1.0 / 8388608.0,
            1.0 / 2147483648.0,
        ];
        let value = self.framework_value("dimen", name)?;
        (value.kind == crate::package::parse::resources::TYPE_DIMENSION
            && value.data & 0xf == COMPLEX_UNIT_DIP)
            .then(|| {
                (value.data & 0xffff_ff00) as i32 as f32
                    * RADIX_MULTS[((value.data >> 4) & 3) as usize]
            })
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

    /// The framework's string array resource `name` (`getStringArray`).
    pub fn framework_string_array(&self, name: &str) -> Option<Vec<Option<String>>> {
        let id = self.platform.framework.id("array", name)?;
        self.framework_resources().string_array(id)
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
        self.with_resources(root, data, pkg, |resources, apks, table_apks| {
            let (table, file) = resources
                .resource_string_source(id)
                .ok_or_else(|| format!("no XML resource {id:#x}"))?;
            let bytes = match table_apks
                .get(table)
                .ok_or("an XML resource of an overlay")?
            {
                Some(i) => apks[*i].file(file),
                None => aim_apps::apk::Apk::open(&root.join("system/framework/framework-res.apk"))
                    .and_then(|apk| apk.file(file)),
            }
            .map_err(|e| e.to_string())?;
            let document = aim_apps::res::xml(&bytes).map_err(|e| e.to_string())?;
            let ctx = parse::attrs::Ctx {
                res: resources,
                attrs: &self.platform.framework_attrs,
                error: Default::default(),
            };
            let value = read(&document, &ctx);
            match ctx.error.take() {
                Some(e) => Err(e),
                None => Ok(value),
            }
        })
    }

    /// `getResourcesForApplication`'s `getResourceName` and `getString` of
    /// each of `ids` in `pkg`'s resources: its package id, type and entry,
    /// and its string if it is one.
    #[allow(clippy::type_complexity)]
    pub fn resource_names_and_strings(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        ids: &[u32],
    ) -> Result<Vec<(Option<(u8, String, String)>, Option<String>)>, String> {
        self.with_resources(root, data, pkg, |resources, _, _| {
            Ok(ids
                .iter()
                .map(|&id| {
                    let name = resources
                        .entry_name(id)
                        .map(|(p, t, n)| (p, t.to_owned(), n.to_owned()));
                    (name, resources.resource_string(id))
                })
                .collect())
        })
    }

    /// `getResourcesForApplication`'s `getIdentifier(name, "string",
    /// package)` in `pkg`'s resources: 0 when there is none.
    pub fn string_identifier(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        name: &str,
    ) -> Result<u32, String> {
        self.with_resources(root, data, pkg, |resources, _, _| {
            Ok(resources
                .identifier(&pkg.package_name, "string", name)
                .unwrap_or(0))
        })
    }

    /// `getResourcesForApplication`'s `getIdentifier(name, null, null)` in
    /// `pkg`'s resources: 0 when there is none.
    pub fn named_identifier(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        name: &str,
    ) -> Result<u32, String> {
        self.with_resources(root, data, pkg, |resources, _, _| {
            Ok(full_name_identifier(resources, name).unwrap_or(0))
        })
    }

    /// `ActivityRecord.WindowStyle` of the theme `theme` of `pkg`, an app
    /// targeting `target_sdk` (`getThemeResource`, else `Theme` or
    /// `Theme_Holo`): the `R.styleable.Window` attributes
    /// `AttributeCache.get` obtains on the theme of a package's context
    /// (`Resources.selectDefaultTheme`).
    pub fn window_style(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        theme: u32,
        target_sdk: i32,
    ) -> Result<WindowStyle, String> {
        const HONEYCOMB: i32 = 11;
        const ICE_CREAM_SANDWICH: i32 = 14;
        const N: i32 = 24;
        let style = |name: &str| {
            self.platform
                .framework
                .id("style", name)
                .ok_or_else(|| format!("no framework style {name}"))
        };
        let theme = match theme {
            0 => style(if target_sdk < HONEYCOMB {
                "Theme"
            } else {
                "Theme.Holo"
            })?,
            theme => theme,
        };
        let default = style(if target_sdk < HONEYCOMB {
            "Theme"
        } else if target_sdk < ICE_CREAM_SANDWICH {
            "Theme.Holo"
        } else if target_sdk < N {
            "Theme.DeviceDefault"
        } else {
            "Theme.DeviceDefault.Light.DarkActionBar"
        })?;
        self.with_resources(root, data, pkg, |resources, _, _| {
            // `TypedArray.getBoolean`: an integer's, else false.
            let flag = |name: &str| -> Result<bool, String> {
                let attr = self
                    .platform
                    .framework
                    .id("attr", name)
                    .ok_or_else(|| format!("no attribute {name}"))?;
                let int = parse::resources::TYPE_FIRST_INT..=parse::resources::TYPE_LAST_INT;
                Ok(resources
                    .styled_attr(theme, default, attr)
                    .is_some_and(|v| int.contains(&v.kind) && v.data != 0))
            };
            Ok(WindowStyle {
                translucent: flag("windowIsTranslucent")?,
                floating: flag("windowIsFloating")?,
                show_wallpaper: flag("windowShowWallpaper")?,
                no_display: flag("windowNoDisplay")?,
            })
        })
    }

    /// `pkg`'s resources (its own and the framework's), from its APKs
    /// below the image `root` or the guest's `/data` at `data`, given to
    /// `f` with the APKs and each table's APK among them (None for the
    /// framework's).
    fn with_resources<T>(
        &self,
        root: &Path,
        data: &Path,
        pkg: &AndroidPackage,
        f: impl FnOnce(
            &parse::resources::Resources<'_>,
            &[aim_apps::apk::Apk],
            &[Option<usize>],
        ) -> Result<T, String>,
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
        f(&resources, &apks, &table_apks)
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
pub(super) fn host_path(root: &Path, data: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = path
        .strip_prefix('/')
        .ok_or_else(|| format!("relative code path {path}"))?;
    Ok(match relative.strip_prefix("data/") {
        Some(rest) => data.join(rest),
        None => root.join(relative),
    })
}

/// `ResourcesImpl.getIdentifier(name, null, null)`: a decimal id as is,
/// else `AssetManager2::GetResourceId` of `@?package:type/entry`
/// (`ExtractResourceName`) with neither part to fall back on, a private
/// attribute found under `^attr-private`.
fn full_name_identifier(resources: &parse::resources::Resources<'_>, name: &str) -> Option<u32> {
    if let Ok(id) = name.parse::<i32>() {
        return Some(id as u32);
    }
    let (package, kind, entry) = full_name(name)?;
    resources.identifier(package, kind, entry).or_else(|| {
        (kind == "attr")
            .then(|| resources.identifier(package, "^attr-private", entry))
            .flatten()
    })
}

/// `ExtractResourceName` of `@?package:type/entry`, with no default
/// package or type: none unless both are named.
fn full_name(name: &str) -> Option<(&str, &str, &str)> {
    let name = name.strip_prefix('@').unwrap_or(name);
    let (mut package, mut kind) = ("", "");
    let (mut has_package, mut has_kind) = (false, false);
    let mut start = 0;
    for (i, c) in name.char_indices() {
        if kind.is_empty() && c == '/' {
            has_kind = true;
            kind = &name[start..i];
            start = i + 1;
        } else if package.is_empty() && c == ':' {
            has_package = true;
            package = &name[start..i];
            start = i + 1;
        }
    }
    if (has_package && package.is_empty()) || (has_kind && kind.is_empty()) {
        return None;
    }
    // `GetResourceId` takes neither from a fallback.
    (!package.is_empty() && !kind.is_empty()).then(|| (package, kind, &name[start..]))
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
    // `scanSystemPackageLI`: a static shared library not from an APEX
    // has its synthetic name (`renameStaticSharedLibraryPackage`).
    if pkg.static_shared_library_name.is_some() && !setting.code_path.starts_with("/apex/") {
        pkg.set_package_name(format!(
            "{}_{}",
            pkg.package_name, pkg.static_shared_lib_version
        ));
    }
    pkg.uid = setting.app_id;
    pkg.primary_cpu_abi = setting.primary_cpu_abi.clone();
    pkg.secondary_cpu_abi = setting.secondary_cpu_abi.clone();
    // The signers PackageManager collected and persisted (`<sigs>`).
    if let Some(saved) = &setting.signatures {
        let signing =
            package::sign::SigningDetails::from_saved(saved).and_then(|s| s.parcel_details());
        pkg.signing_details = Some(signing.map_err(|e| format!("signatures: {e}"))?);
    }
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
    // `ScanPackageUtils.applyPolicy`: a system app that is direct boot
    // aware has all its components so.
    if system && pkg.is(booleans::DIRECT_BOOT_AWARE) {
        for c in pkg.activities.iter_mut().chain(&mut pkg.receivers) {
            c.main.direct_boot_aware = true;
        }
        for c in &mut pkg.services {
            c.main.direct_boot_aware = true;
        }
        for c in &mut pkg.providers {
            c.main.direct_boot_aware = true;
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
        // `categoryHint`: `CATEGORY_UNDEFINED` without one.
        category_override: setting.category_hint,
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

/// A container as `addForInitLI` leaves its setting: new each boot (apexd's
/// packages are not persisted), of no app id, installed for user 0.
fn join_apex(
    code: &package::scan::ApexCode,
    pkg: Arc<AndroidPackage>,
    flags: i32,
    private_flags: i32,
    stopped: bool,
) -> Result<PackageState, String> {
    let signing = &code.signing;
    let mut ps = PackageState {
        name: pkg.package_name.clone(),
        app_id: -1,
        path: code.info.module_path.clone(),
        version_code: (i64::from(pkg.version_code_major) << 32)
            | i64::from(pkg.version_code as u32),
        target_sdk_version: pkg.target_sdk_version,
        category_override: -1,
        signatures: Some(settings::Signatures {
            scheme_version: signing.scheme_version,
            signatures: signing.signatures.clone(),
            current_flags: signing.current_flags.clone(),
            public_keys: signing.serialized_public_keys()?,
            past_signatures: signing.past_signing_certificates.clone(),
        }),
        ..Default::default()
    };
    ps.is.system = flags & settings::FLAG_SYSTEM != 0;
    ps.is.privileged = private_flags & settings::PRIVATE_FLAG_PRIVILEGED != 0;
    ps.users.insert(
        0,
        PackageUserState {
            installed: true,
            stopped,
            ..Default::default()
        },
    );
    ps.pkg = Some(pkg);
    Ok(ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `getIdentifier(name, null, null)` takes only a full name, as
    /// `ExtractResourceName` splits it.
    #[test]
    fn full_resource_names_split_as_extract_resource_name() {
        assert_eq!(full_name("p:drawable/ic"), Some(("p", "drawable", "ic")));
        assert_eq!(
            full_name("@android:drawable/ic_lock"),
            Some(("android", "drawable", "ic_lock"))
        );
        // Without a default to fall back on, a missing part finds none.
        assert_eq!(full_name("drawable/ic"), None);
        assert_eq!(full_name("p:ic"), None);
        assert_eq!(full_name("ic"), None);
        // An empty part given by its separator is refused.
        assert_eq!(full_name(":drawable/ic"), None);
        assert_eq!(full_name("p:/ic"), None);
    }

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

    /// The derived image's tethering container registers as
    /// `scanApexPackages` registers it, and the module metadata provider's
    /// tethering module names its APEX and the APK in it.
    #[test]
    fn containers_register_and_name_their_modules() {
        use crate::package::bootstrap::{ActiveApex, ApexInventory, ApexPackage};
        let Some(root) = aim_paths::input(aim_paths::derived_image(), "derived-image") else {
            return;
        };
        let data = Path::new("/nonexistent");
        let module_path = "/system/apex/com.google.android.tethering.capex";
        let inventory = ApexInventory {
            packages: Some(vec![ApexPackage {
                module_name: Some("com.android.tethering".into()),
                module_path: module_path.into(),
                preinstalled_path: module_path.into(),
                version_code: 0,
                factory: true,
                active: true,
                active_changed: false,
            }]),
            active: vec![ActiveApex {
                module_name: Some("com.android.tethering".into()),
                mount_path: "/apex/com.android.tethering".into(),
                preinstalled_path: module_path.into(),
                factory: true,
                module_path: module_path.into(),
                active_changed: false,
            }],
        };
        let apks = package::write::Apks {
            signing_overrides: None,
            files: {
                let root = root.clone();
                Box::new(move |path| host_path(&root, Path::new("/nonexistent"), path).ok())
            },
            platform: Platform::load(&root, Default::default()).unwrap(),
        };
        let config = system_config::SystemConfig::read(&root, &|_| None);
        let image =
            package::scan::ApexImage::load(&apks, &inventory, parse::PARSE_IS_SYSTEM_DIR).unwrap();
        let code = &image.packages[0];
        let (pkg, flags, private_flags, stopped) =
            package::scan::registered_apex(code, &Default::default(), &config, &apks).unwrap();
        let pkg = Arc::new(pkg);
        let container = join_apex(code, pkg.clone(), flags, private_flags, stopped).unwrap();
        assert_eq!(container.name, "com.google.android.tethering");
        assert_eq!((container.app_id, pkg.uid), (-1, -1));
        assert!(pkg.is2(package::pkg::booleans2::APEX) && pkg.is(booleans::SYSTEM));
        assert!(container.is.system && container.users[&0].installed);

        let user = restrictions::UserState {
            installed: true,
            ..Default::default()
        };
        let system_package = |name: &str, code_path: &str| {
            let setting = settings::Package {
                name: name.into(),
                code_path: code_path.into(),
                flags: settings::FLAG_SYSTEM,
                app_id: 10_000,
                ..Default::default()
            };
            let host = host_path(&root, data, code_path).unwrap();
            join(&setting, None, Some(&user), &host, &apks.platform).unwrap()
        };
        let tethering = system_package(
            "com.google.android.networkstack.tethering",
            "/apex/com.android.tethering/priv-app/TetheringGoogle@BE2A.250530.026.F3",
        );
        let provider = system_package(
            "com.google.android.modulemetadata",
            "/product/app/ModuleMetadataGoogle",
        );
        let mut users = BTreeMap::new();
        users.insert(
            0,
            model::User {
                id: 0,
                unlocking_or_unlocked: true,
                ..Default::default()
            },
        );
        let state = model::State {
            packages: [container, tethering.clone(), provider]
                .into_iter()
                .map(|ps| (ps.name.clone(), ps))
                .collect(),
            users,
            system: system_config::system(
                &root,
                &|_| None,
                &system_config::Framework::load(&root).unwrap(),
            )
            .unwrap(),
            ..Default::default()
        };
        let results = [package::scan::ApexScanResult {
            info: code.info.clone(),
            package: (*pkg).clone(),
            signing: code.signing.clone(),
        }];
        let links = package::module_metadata::ApexLinks::from_scan(
            &results,
            &inventory,
            &[tethering.pkg.as_deref().unwrap().clone()],
        )
        .unwrap();
        let modules = package::module_metadata::Owner::load(
            &state,
            &apks.platform,
            apks.platform.config(),
            &|path| host_path(&root, data, path),
            &links,
        )
        .unwrap();
        assert!(modules.loaded(), "{:?}", modules.diagnostic());
        let module = modules
            .modules()
            .iter()
            .find(|m| m.package.as_deref() == Some("com.google.android.tethering"))
            .unwrap();
        assert_eq!(module.apex.as_deref(), Some("com.android.tethering"));
        assert_eq!(
            module.apks.as_deref(),
            Some(
                &[Some(
                    "com.google.android.networkstack.tethering".to_string()
                )][..]
            )
        );
    }
}
