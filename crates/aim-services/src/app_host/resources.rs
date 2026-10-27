//! Framework and APK resources in the device configuration, independent
//! of package selection and the live package model.

use std::path::{Path, PathBuf};

use crate::package::parse::{self, Platform};
use crate::package::pkg::AndroidPackage;

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

/// The framework's resources as the device configures them
/// (`Resources.getSystem`: framework-res.apk with its static overlays, at
/// the device's density and locale).
pub struct Framework(pub Platform);

impl Framework {
    /// The device's framework resources, read below the image `root`.
    pub fn load(root: &Path, density_dpi: i32, locale: &str) -> Result<Self, String> {
        device_platform(root, density_dpi, locale).map(Framework)
    }

    fn framework_resources(&self) -> crate::package::parse::resources::Resources<'_> {
        crate::package::parse::resources::Resources {
            tables: vec![&self.0.framework],
            overlays: &self.0.framework_overlays,
            config: self.0.config(),
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
        let id = self.0.framework.id(kind, name)?;
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
        self.0.framework.id(kind, name)
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
        self.0.framework_file(root, kind, name)
    }

    /// The framework's string resource `name` in the device's locale.
    pub fn framework_string(&self, name: &str) -> Option<String> {
        let id = self.0.framework.id("string", name)?;
        self.framework_resources().resource_string(id)
    }

    pub fn framework_int_array(&self, name: &str) -> Option<Vec<i32>> {
        self.framework_resources()
            .int_array(self.0.framework.id("array", name)?)
    }

    /// The framework's string array resource `name` (`getStringArray`).
    pub fn framework_string_array(&self, name: &str) -> Option<Vec<Option<String>>> {
        let id = self.0.framework.id("array", name)?;
        self.framework_resources().string_array(id)
    }

    /// SystemConfig's split permissions.
    pub fn split_permissions(&self) -> &[parse::SplitPermission] {
        &self.0.split_permissions
    }
}

/// The image's platform at the device's density and locale, as
/// `persist.sys.locale` holds it (`ko-KR`).
pub(super) fn device_platform(
    root: &Path,
    density_dpi: i32,
    locale: &str,
) -> Result<Platform, String> {
    let mut platform = Platform::load(root, Default::default())?;
    platform.density_dpi = Some(density_dpi);
    if let Some((language, country)) = locale.split_once('-')
        && let (Ok(language), Ok(country)) = (
            <[u8; 2]>::try_from(language.as_bytes()),
            <[u8; 2]>::try_from(country.as_bytes()),
        )
    {
        platform.locale = (language, country);
    }
    Ok(platform)
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

impl Framework {
    /// A package context's resources (`createPackageContext`): the
    /// framework's, then the package's `table`, in the device's
    /// configuration.
    pub fn package_resources<'a>(
        &'a self,
        table: &'a crate::package::parse::resources::Table,
    ) -> crate::package::parse::resources::Resources<'a> {
        crate::package::parse::resources::Resources {
            tables: vec![&self.0.framework, table],
            overlays: &self.0.framework_overlays,
            config: self.0.config(),
        }
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
                attrs: &self.0.framework_attrs,
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
            self.0
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
                    .0
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
        let mut resources = vec![&self.0.framework];
        for (i, table) in tables.iter().enumerate() {
            if let Some(table) = table {
                resources.push(table);
                table_apks.push(Some(i));
            }
        }
        let resources = parse::resources::Resources {
            tables: resources,
            overlays: &self.0.framework_overlays,
            config: self.0.config(),
        };
        f(&resources, &apks, &table_apks)
    }
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
}
