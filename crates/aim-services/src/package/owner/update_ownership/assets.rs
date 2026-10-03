//! UpdateOwnershipHelper's ResourcesManager input at android-16.0.0_r1.
//! PackageImpl.toAppInfoWithoutState supplies base/all splits, but no
//! state-derived resourceDirs, overlayPaths or sharedLibraryFiles.
use super::*;
use crate::package::{
    parse::resources::{Config, Resources, Table},
    pkg::PropertyValue,
    settings::Settings,
    system_config::SystemConfig,
    write::Apks,
};
use aim_apps::res::{Result, bad};
use android_image_extract::{source::FileSource, zip::Archive};

const MAX_ENTRY: u64 = 512 << 20;

impl UpdateOwnership {
    /// Execute a queued post-commit read through guest file ownership. The
    /// resource configuration is the boot owner's ResourcesManager config,
    /// not the manifest parser's reduced configuration.
    pub fn complete_apk_read(
        &mut self,
        parsed: &AndroidPackage,
        settings: &mut Settings,
        system_config: &SystemConfig,
        apks: &Apks,
        config: Config,
    ) -> Result<Vec<String>> {
        if !self.pending.contains(&parsed.package_name) {
            return Err(bad("update ownership provider read is not queued"));
        }
        let id = parsed
            .properties
            .as_ref()
            .and_then(|properties| {
                properties.iter().find(|(name, _)| {
                    name == "android.app.PROPERTY_LEGACY_UPDATE_OWNERSHIP_DENYLIST"
                })
            })
            .map(|(_, property)| match property.value {
                PropertyValue::Resource(id) => id as u32,
                _ => 0, // Property.getResourceId for a different property type.
            })
            .ok_or_else(|| bad("queued provider lost its denylist property"))?;
        let base = parsed
            .base_apk_path
            .as_deref()
            .ok_or_else(|| bad("no parsed denylist provider base APK"))?;
        let mut paths = vec![base];
        for split in parsed.split_code_paths.iter().flatten() {
            paths.push(
                split
                    .as_deref()
                    .ok_or_else(|| bad("null denylist provider split path"))?,
            );
        }
        let mut assets = Vec::new();
        for path in paths {
            let host = (apks.files)(path).ok_or_else(|| bad(format!("{path}: not readable")))?;
            let source = FileSource::open(&host).map_err(|e| bad(format!("{path}: {e}")))?;
            let archive = Archive::open(&source).map_err(|e| bad(format!("{path}: {e}")))?;
            let table = archive
                .find(b"resources.arsc")
                .map(|entry| {
                    archive
                        .read(entry, MAX_ENTRY)
                        .map_err(|e| bad(format!("{path}: {e}")))
                        .and_then(|bytes| Table::parse(&bytes))
                })
                .transpose()?;
            // Code-only splits still have to be readable APK archives.
            assets.push((source, table));
        }
        let mut tables = vec![&apks.platform.framework];
        let mut source_indices = Vec::new();
        for (index, (_, table)) in assets.iter().enumerate() {
            if let Some(table) = table {
                tables.push(table);
                source_indices.push(index);
            }
        }
        let resources = Resources {
            tables,
            overlays: &apks.platform.framework_overlays,
            config,
        };
        let file = |cookie: usize, name: &str| {
            if cookie > 0 && cookie < resources.tables.len() {
                let source = &assets[source_indices[cookie - 1]].0;
                return read_file(source, name);
            }
            let host = if cookie == 0 {
                (apks.files)("/system/framework/framework-res.apk")
                    .ok_or_else(|| bad("framework resource APK is not readable"))?
            } else {
                apks.platform
                    .framework_overlay_apks
                    .get(cookie - resources.tables.len())
                    .ok_or_else(|| bad("framework overlay resource source is absent"))?
                    .clone()
            };
            let source = FileSource::open(&host).map_err(|e| bad(e.to_string()))?;
            read_file(&source, name)
        };
        self.complete_resource_read(
            &parsed.package_name,
            settings,
            system_config,
            &resources,
            id,
            file,
        )
    }
}

fn read_file(source: &FileSource, name: &str) -> Result<Vec<u8>> {
    let archive = Archive::open(source).map_err(|e| bad(e.to_string()))?;
    let entry = archive
        .find(name.as_bytes())
        .ok_or_else(|| bad(format!("no resource asset {name}")))?;
    archive
        .read(entry, MAX_ENTRY)
        .map_err(|e| bad(e.to_string()))
}
