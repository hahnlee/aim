//! APEX input phase from android-16.0.0_r1 InstallPackageHelper.scanApexPackages.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::Error;
use crate::package::{
    bootstrap::{ApexInventory, ApexPackage},
    parse,
    pkg::AndroidPackage,
    sign,
    write::Apks,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct ApexCode {
    pub info: ApexPackage,
    pub parsed: AndroidPackage,
    pub signing: sign::SigningDetails,
    /// The initial scan clears SYSTEM_DIR for updated APEX only after parsing.
    pub scan_parse_flags: i32,
}

#[derive(Debug, Default)]
pub struct ApexImage {
    pub packages: Vec<ApexCode>,
}

impl ApexImage {
    /// Read the original owner's module paths, not flattened payloads or APK
    /// scan directories. Errors abort the phase, as in scanApexPackages.
    pub fn load(apks: &Apks, inventory: &ApexInventory, parse_flags: i32) -> Result<Self, Error> {
        let mut packages = Vec::new();
        for info in ordered_sources(inventory) {
            let fail = |phase, message| Error {
                package: info.module_name.clone().unwrap_or_default(),
                path: info.module_path.clone(),
                phase,
                message,
            };
            let parsed = apks
                .parsed_path(&info.module_path, parse_flags)
                .map_err(|error| fail("apex-parse", error))?;
            let signing = apks
                .signing_details(&parsed)
                .map_err(|error| fail("apex-signatures", error))?;
            packages.push(ApexCode {
                info: info.clone(),
                parsed,
                signing,
                scan_parse_flags: if info.factory {
                    parse_flags
                } else {
                    parse_flags & !parse::PARSE_IS_SYSTEM_DIR
                },
            });
        }
        Ok(Self { packages })
    }
}

fn ordered_sources(inventory: &ApexInventory) -> Vec<&ApexPackage> {
    // Original ArrayMap<File,ApexInfo> retains the last metadata for a path.
    // Serial parse completion order is owner order; factory-first sorting is
    // stable, like the original's sort over parallel completion results.
    let mut positions = BTreeMap::new();
    let mut sources = Vec::new();
    for info in inventory.packages.iter().flatten() {
        if let Some(&at) = positions.get(&info.module_path) {
            sources[at] = info;
        } else {
            positions.insert(&info.module_path, sources.len());
            sources.push(info);
        }
    }
    sources.sort_by_key(|info| !info.factory);
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apex_sources_preserve_factory_priority_and_last_path_metadata() {
        let info = |path: &str, factory, version| ApexPackage {
            module_name: Some("same.module".into()),
            module_path: path.into(),
            preinstalled_path: "/system/apex/factory.capex".into(),
            version_code: version,
            factory,
            active: !factory,
            active_changed: !factory,
        };
        let mut inventory = ApexInventory {
            packages: None,
            active: Vec::new(),
        };
        assert!(ordered_sources(&inventory).is_empty());
        inventory.packages = Some(vec![
            info("/data/updated.apex", false, 2),
            info("/system/z.capex", true, 1),
            info("/system/a.apex", true, 1),
            info("/system/z.capex", true, 3),
        ]);
        let sources = ordered_sources(&inventory);
        assert_eq!(
            sources
                .iter()
                .map(|i| i.module_path.as_str())
                .collect::<Vec<_>>(),
            ["/system/z.capex", "/system/a.apex", "/data/updated.apex"]
        );
        assert_eq!(sources[0].version_code, 3);
        assert_eq!(sources.len(), 3);
    }
}
