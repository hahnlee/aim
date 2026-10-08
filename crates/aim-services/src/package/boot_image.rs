//! Image/data owners for the native PackageManager C constructor.
use super::{parse::Platform, system_config::SystemConfig, write::Apks, sign::Overrides};
use aim_android_init::ImageRoot;
use std::{path::{Path, PathBuf}, sync::Arc};

pub struct Configuration { pub image: PathBuf, pub data: PathBuf, pub original_roots: Vec<PathBuf> }
impl Configuration {
    pub fn pending(image: &Path, data: &Path, original_roots: &[PathBuf]) -> Result<Self, String> {
        let image = std::fs::canonicalize(image).map_err(|error| error.to_string())?;
        if !data.is_absolute() || data.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
            return Err("native data mapping must be an absolute guest filesystem path".into());
        }
        let original_roots = original_roots.iter().map(std::fs::canonicalize).collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(Self { image, data: data.into(), original_roots })
    }
    pub fn open(&self) -> Result<Arc<Owner>, String> { Owner::open(&self.image, &self.data, &self.original_roots) }
}

pub struct Owner { pub image: PathBuf, pub data: PathBuf, pub original_roots: Vec<PathBuf> }
impl Owner {
    pub fn open(image: &Path, data: &Path, original_roots: &[PathBuf]) -> Result<Arc<Self>, String> {
        let image = std::fs::canonicalize(image).map_err(|error| error.to_string())?;
        let data = std::fs::canonicalize(data).map_err(|error| error.to_string())?;
        if !image.is_dir() || !data.is_dir() || image.starts_with(&data) || data.starts_with(&image) {
            return Err("native package image and writable data owners overlap or are absent".into());
        }
        let original_roots = original_roots.iter().map(std::fs::canonicalize).collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if original_roots.iter().any(|original| data.starts_with(original) || original.starts_with(&data)) {
            return Err("native package data overlaps an original image owner".into());
        }
        Ok(Arc::new(Self { image, data, original_roots }))
    }
    pub fn host_path(&self, guest: &str) -> Option<PathBuf> {
        if !guest.starts_with('/') || guest.split('/').any(|component| component == "..") { return None; }
        if guest == "/data" { return Some(self.data.clone()); }
        if let Some(suffix) = guest.strip_prefix("/data/") { return Some(self.data.join(suffix)); }
        Some(ImageRoot::new(self.image.clone()).host_path(guest))
    }
    pub fn apks(self: &Arc<Self>, config: &SystemConfig, signing: Arc<Overrides>, density: i32,
        properties: &dyn Fn(&str) -> Result<Option<String>, String>) -> Result<Arc<Apks>, String> {
        let features = config.features.iter().map(|(name, _)| name.clone()).collect();
        if density <= 0 { return Err("native APK parser density requires actual display metrics".into()); }
        let mut platform = Platform::load(&self.image, features)?;
        platform.density_dpi = Some(density);
        platform.set_sdk_extensions(properties)?;
        let files = self.clone();
        Ok(Arc::new(Apks { platform, signing_overrides: Some(signing), files: Box::new(move |guest| files.host_path(guest)) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_configuration_waits_for_real_guest_data_directory() {
        let root = std::env::temp_dir().join(format!("aim-package-image-mount-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let image = root.join("image");
        std::fs::create_dir(&image).unwrap();
        let volume = root.join("volume");
        let data = volume.join("data");
        let pending = Configuration::pending(&image, &data, &[]).unwrap();
        assert!(pending.open().is_err());
        std::fs::create_dir_all(&data).unwrap();
        let owner = pending.open().unwrap();
        assert_eq!(owner.host_path("/data/system/packages.xml").unwrap(), data.canonicalize().unwrap().join("system/packages.xml"));
        assert_ne!(owner.data, volume);
        std::fs::remove_dir_all(root).unwrap();
    }
}
