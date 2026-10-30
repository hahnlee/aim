//! Image/data owners for the native PackageManager C constructor.
use super::{
    parse::{Platform, resources::Config},
    sign::Overrides,
    system_config::SystemConfig,
    write::Apks,
};
use aim_android_init::ImageRoot;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

/// Immutable image inputs shared by one package boot and its retained leaves.
/// Runtime properties and resource configurations are applied by each consumer.
pub struct Inputs {
    pub image: PathBuf,
    config: OnceLock<super::system_config::ImageConfig>,
    platform: OnceLock<Result<Platform, String>>,
    seinfo: OnceLock<Result<super::owner::seinfo::Policy, String>>,
}
impl Inputs {
    pub fn new(image: &Path) -> Arc<Self> {
        Arc::new(Self {
            image: image.into(),
            config: OnceLock::new(),
            platform: OnceLock::new(),
            seinfo: OnceLock::new(),
        })
    }
    pub fn config(&self, properties: &dyn Fn(&str) -> Option<String>) -> SystemConfig {
        self.config
            .get_or_init(|| SystemConfig::read_image(&self.image, properties))
            .clone()
            .with_environment(properties)
    }
    pub fn platform(&self) -> Result<&Platform, String> {
        self.platform
            .get_or_init(|| Platform::load(&self.image, Default::default()))
            .as_ref()
            .map_err(Clone::clone)
    }
    pub fn seinfo(&self) -> Result<super::owner::seinfo::Policy, String> {
        self.seinfo
            .get_or_init(|| super::owner::seinfo::Policy::load(&self.image))
            .clone()
    }
}

pub struct Configuration {
    pub image: PathBuf,
    pub data: PathBuf,
    pub original_roots: Vec<PathBuf>,
}
impl Configuration {
    pub fn pending(image: &Path, data: &Path, original_roots: &[PathBuf]) -> Result<Self, String> {
        let image = std::fs::canonicalize(image).map_err(|error| error.to_string())?;
        if !data.is_absolute()
            || data
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("native data mapping must be an absolute guest filesystem path".into());
        }
        let original_roots = original_roots
            .iter()
            .map(std::fs::canonicalize)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            image,
            data: data.into(),
            original_roots,
        })
    }
    pub fn open(&self) -> Result<Arc<Owner>, String> {
        Owner::open(&self.image, &self.data, &self.original_roots)
    }
}

pub struct Owner {
    pub image: PathBuf,
    pub data: PathBuf,
    pub original_roots: Vec<PathBuf>,
}
impl Owner {
    pub fn open(
        image: &Path,
        data: &Path,
        original_roots: &[PathBuf],
    ) -> Result<Arc<Self>, String> {
        let image = std::fs::canonicalize(image).map_err(|error| error.to_string())?;
        let data = std::fs::canonicalize(data).map_err(|error| error.to_string())?;
        if !image.is_dir() || !data.is_dir() || image.starts_with(&data) || data.starts_with(&image)
        {
            return Err(
                "native package image and writable data owners overlap or are absent".into(),
            );
        }
        let original_roots = original_roots
            .iter()
            .map(std::fs::canonicalize)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if original_roots
            .iter()
            .any(|original| data.starts_with(original) || original.starts_with(&data))
        {
            return Err("native package data overlaps an original image owner".into());
        }
        Ok(Arc::new(Self {
            image,
            data,
            original_roots,
        }))
    }
    pub fn host_path(&self, guest: &str) -> Option<PathBuf> {
        if !guest.starts_with('/') || guest.split('/').any(|component| component == "..") {
            return None;
        }
        if guest == "/data" {
            return Some(self.data.clone());
        }
        if let Some(suffix) = guest.strip_prefix("/data/") {
            return Some(self.data.join(suffix));
        }
        Some(ImageRoot::new(self.image.clone()).host_path(guest))
    }
    pub fn apks(
        self: &Arc<Self>,
        config: &SystemConfig,
        signing: Arc<Overrides>,
        density: i32,
        resources: Config,
        properties: &dyn Fn(&str) -> Result<Option<String>, String>,
    ) -> Result<Arc<Apks>, String> {
        self.apks_with_platform(config, signing, density, resources, properties, || {
            Platform::load(&self.image, Default::default())
        })
    }
    pub(crate) fn apks_with_platform(
        self: &Arc<Self>,
        config: &SystemConfig,
        signing: Arc<Overrides>,
        density: i32,
        resources: Config,
        properties: &dyn Fn(&str) -> Result<Option<String>, String>,
        platform: impl FnOnce() -> Result<Platform, String>,
    ) -> Result<Arc<Apks>, String> {
        let features = config
            .features
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        if density <= 0 {
            return Err("native APK parser density requires actual display metrics".into());
        }
        if resources.language == [0, 0] {
            return Err("native APK parser locale requires actual resource configuration".into());
        }
        let mut platform = platform()?;
        platform.features = features;
        platform.density_dpi = Some(density);
        platform.locale = (resources.language, resources.country);
        platform.use_round_icon = platform.framework_boolean("config_useRoundIcon")?;
        platform.set_sdk_extensions(properties)?;
        let files = self.clone();
        Ok(Arc::new(Apks {
            platform,
            signing_overrides: Some(signing),
            files: Box::new(move |guest| files.host_path(guest)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_inputs_retain_policy_but_reapply_environment_and_retire_with_boot() {
        let root = crate::app_host::test_temp_dir("boot-image-inputs");
        let dir = root.join("system/etc/sysconfig");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("features.xml");
        std::fs::write(&file, "<config><feature name='fixture.initial'/><unavailable-feature name='android.software.adoptable_storage'/></config>").unwrap();
        let inputs = Inputs::new(&root);
        for adoptable in ["0", "1"] {
            let properties = |name: &str| (name == "vold.has_adoptable").then(|| adoptable.into());
            assert_eq!(
                inputs.config(&properties),
                SystemConfig::read(&root, &properties)
            );
        }
        std::fs::write(
            &file,
            "<config><feature name='fixture.replacement'/></config>",
        )
        .unwrap();
        let properties = |name: &str| (name == "vold.has_adoptable").then(|| "1".into());
        let retained = inputs.config(&properties);
        assert!(
            retained
                .features
                .iter()
                .any(|(name, _)| name == "fixture.initial")
        );
        assert!(
            !retained
                .features
                .iter()
                .any(|(name, _)| name == "android.software.adoptable_storage")
        );
        let weak = Arc::downgrade(&inputs);
        drop(inputs);
        assert!(weak.upgrade().is_none());
        let next_boot = Inputs::new(&root);
        let fresh = next_boot.config(&properties);
        assert_eq!(fresh, SystemConfig::read(&root, &properties));
        assert!(
            fresh
                .features
                .iter()
                .any(|(name, _)| name == "fixture.replacement")
        );
        assert!(
            fresh
                .features
                .iter()
                .any(|(name, _)| name == "android.software.adoptable_storage")
        );
        assert!(
            !next_boot
                .config(&|_| None)
                .features
                .iter()
                .any(|(name, _)| name == "android.software.adoptable_storage")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn boot_input_errors_are_lazy_and_confined_to_their_epoch() {
        let root = crate::app_host::test_temp_dir("boot-image-errors");
        let inputs = Inputs::new(&root);
        let platform_error = inputs.platform().err().unwrap();
        std::fs::create_dir_all(root.join("system/etc/selinux")).unwrap();
        std::fs::write(root.join("system/build.prop"), "ro.build.version.sdk=36\n").unwrap();
        assert_eq!(inputs.platform().err().unwrap(), platform_error);
        assert_ne!(Inputs::new(&root).platform().err().unwrap(), platform_error);
        let seinfo_error = inputs.seinfo().unwrap_err();
        std::fs::write(
            root.join("system/etc/selinux/plat_mac_permissions.xml"),
            "<policy/>",
        )
        .unwrap();
        assert_eq!(inputs.seinfo().unwrap_err(), seinfo_error);
        assert!(Inputs::new(&root).seinfo().is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires pinned original image; host-only, no guest boot"]
    fn native_apk_platform_uses_original_configuration() {
        use crate::package::parse::resources::Resources;
        let image = aim_paths::original_image();
        assert!(image.join("system/framework/framework-res.apk").is_file());
        let data = std::env::temp_dir().join(format!("aim-parser-inputs-{}", std::process::id()));
        std::fs::create_dir(&data).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let _cleanup = Cleanup(data.clone());
        let owner = Owner::open(&image, &data, std::slice::from_ref(&image)).unwrap();
        let mut config = SystemConfig::default();
        config.features = vec![("fixture.owner.feature".into(), 3)];
        let signing = Arc::new(Overrides::new(false));
        let inputs = Inputs::new(&image);
        let load = |resources, density| {
            owner.apks_with_platform(
                &config,
                signing.clone(),
                density,
                resources,
                &|_| Ok(None),
                || inputs.platform().cloned(),
            )
        };
        assert!(load(Config::default(), 320).is_err());
        for (language, country, density, expected) in
            [(*b"ko", *b"KR", 320, "확인"), (*b"en", *b"US", 240, "OK")]
        {
            let resources = Config {
                language,
                country,
                sdk_version: 36,
                ..Default::default()
            };
            assert!(load(resources, 0).is_err());
            let apks = load(resources, density).unwrap();
            let platform = &apks.platform;
            assert_eq!(platform.features, ["fixture.owner.feature".into()].into());
            assert_eq!(platform.density_dpi, Some(density));
            let selected = Resources {
                tables: vec![&platform.framework],
                overlays: &platform.framework_overlays,
                config: platform.config(),
            };
            let ok = platform.framework.id("string", "ok").unwrap();
            assert_eq!(selected.resource_string(ok).as_deref(), Some(expected));
            let original = owner
                .apks(&config, signing.clone(), density, resources, &|_| Ok(None))
                .unwrap();
            assert_eq!(platform.config(), original.platform.config());
            assert_eq!(platform.use_round_icon, original.platform.use_round_icon);
            assert_eq!(platform.sdk_extensions, original.platform.sdk_extensions);
            assert_eq!(inputs.platform().unwrap().sdk_extensions, None);
        }
    }

    #[test]
    fn image_configuration_waits_for_real_guest_data_directory() {
        let root =
            std::env::temp_dir().join(format!("aim-package-image-mount-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let image = root.join("image");
        std::fs::create_dir(&image).unwrap();
        let volume = root.join("volume");
        let data = volume.join("data");
        let pending = Configuration::pending(&image, &data, &[]).unwrap();
        assert!(pending.open().is_err());
        std::fs::create_dir_all(&data).unwrap();
        let owner = pending.open().unwrap();
        assert_eq!(
            owner.host_path("/data/system/packages.xml").unwrap(),
            data.canonicalize().unwrap().join("system/packages.xml")
        );
        assert_ne!(owner.data, volume);
        std::fs::remove_dir_all(root).unwrap();
    }
}
