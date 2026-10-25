//! Framework and APK resources in the device configuration, independent
//! of package selection and the live package model.

use std::path::Path;

use crate::package::parse::{self, Platform};

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
