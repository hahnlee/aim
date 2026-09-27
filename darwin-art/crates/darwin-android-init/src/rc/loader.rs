//! Script discovery: init.cpp `LoadBootScripts` and apex_init_util.cpp.

use std::collections::BTreeMap;

use super::parser::{ParsedScripts, Parser, SectionKind};
use super::{ANDROID_API_FUTURE, IdResolver, InitScripts, ParseEnv};
use crate::PropertyLookup;
use crate::diag::Diagnostic;
use crate::image::ImageRoot;
use crate::libbase::parse_int;

/// The partition script directories `LoadBootScripts` parses after
/// `/system/etc/init/hw/init.rc`, in order.
pub const BOOT_SCRIPT_DIRS: &[&str] = &[
    "/system/etc/init",
    "/system_ext/etc/init",
    "/vendor/etc/init",
    "/odm/etc/init",
    "/product/etc/init",
];

/// One `<apex-info>` of `/apex/apex-info-list.xml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApexInfo {
    pub module_name: String,
    /// `SYSTEM`, `SYSTEM_EXT`, `PRODUCT`, `VENDOR`, `ODM`.
    pub partition: String,
    pub is_active: bool,
}

/// Reads the `moduleName`, `partition` and `isActive` attributes of each
/// `<apex-info>` element. The file is apexd's own output, so a plain
/// attribute scan is sufficient.
pub fn read_apex_info_list(xml: &str) -> Vec<ApexInfo> {
    let mut infos = Vec::new();
    for element in xml.split("<apex-info ").skip(1) {
        let tag = element.split('>').next().unwrap_or("");
        let attribute = |name: &str| -> Option<String> {
            let needle = format!("{name}=\"");
            let start = tag.find(&needle)? + needle.len();
            let end = tag[start..].find('"')? + start;
            Some(tag[start..end].to_string())
        };
        if let Some(module_name) = attribute("moduleName") {
            infos.push(ApexInfo {
                module_name,
                partition: attribute("partition").unwrap_or_default(),
                is_active: attribute("isActive").as_deref() != Some("false"),
            });
        }
    }
    infos
}

/// `FilterVersionedConfigs`: among `foo.rc`, `foo.34rc`, `foo.35rc`, keep
/// the highest version not above `active_sdk`; output ordered by base name.
pub fn filter_versioned_configs(configs: &[String], active_sdk: i64) -> Vec<String> {
    let mut script_map: BTreeMap<String, (String, i64)> = BTreeMap::new();
    for config in configs {
        let parts: Vec<&str> = config.split('.').collect();
        if parts.len() < 2 {
            continue;
        }
        let suffix = parts[parts.len() - 1];
        let sdk: i64 = if suffix == "rc" {
            0
        } else {
            // sscanf("%d%8s"): a number then exactly "rc".
            let digits_end = suffix
                .char_indices()
                .find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))))
                .map_or(suffix.len(), |(i, _)| i);
            let Ok(number) = suffix[..digits_end].parse::<i64>() else {
                continue;
            };
            if &suffix[digits_end..] != "rc" {
                continue;
            }
            number
        };
        if sdk < 0 || sdk > active_sdk {
            continue;
        }
        let base = parts[..parts.len() - 1].join(".");
        match script_map.get(&base) {
            Some((_, existing)) if *existing >= sdk => {}
            _ => {
                script_map.insert(base, (config.clone(), sdk));
            }
        }
    }
    script_map.into_values().map(|(path, _)| path).collect()
}

/// Loads an image's scripts the way init does.
pub struct ScriptLoader<'a> {
    pub image: &'a ImageRoot,
    pub properties: &'a dyn PropertyLookup,
    pub ids: &'a IdResolver,
    pub vendor_api_level: u32,
    /// APEXes from the vendor or odm partition. `None` reads them from
    /// `/apex/apex-info-list.xml`, which apexd writes at boot (the daemon
    /// provides it for the derived image).
    pub vendor_apexes: Option<Vec<String>>,
    /// APEXes apexd activates in bootstrap mode; their scripts load at
    /// `perform_apex_config --bootstrap`, the others' later.
    pub bootstrap_apexes: Vec<String>,
}

impl<'a> ScriptLoader<'a> {
    fn env(&self) -> ParseEnv<'a> {
        ParseEnv {
            properties: self.properties,
            vendor_api_level: self.vendor_api_level,
            ids: self.ids,
        }
    }

    /// Boot scripts, then the bootstrap APEXes' scripts, then the other
    /// APEXes' scripts on top of them.
    pub fn load(&self) -> InitScripts {
        let boot = self.load_boot_scripts();
        let others: Vec<String> = self
            .image
            .subdirectories("/apex")
            .unwrap_or_default()
            .into_iter()
            .filter(|name| !self.bootstrap_apexes.contains(name))
            .collect();
        let bootstrap_apex = self.load_apex_scripts(&boot, Some(&others));
        // init skips the APEXes it parsed in bootstrap mode.
        let apex = self.load_apex_scripts(&bootstrap_apex, Some(&self.bootstrap_apexes));
        InitScripts {
            boot,
            bootstrap_apex,
            apex,
        }
    }

    /// `LoadBootScripts`.
    pub fn load_boot_scripts(&self) -> ParsedScripts {
        let mut parser = Parser::new(self.env(), SectionKind::Boot).with_image(self.image);
        let bootscript = self.properties.property_or("ro.boot.init_rc", "");
        if bootscript.is_empty() {
            parser.parse_config("/system/etc/init/hw/init.rc");
            // A directory that cannot be read goes to late_import_paths,
            // which only Q-and-earlier init consumed; the read failure itself
            // is already reported by parse_config.
            for dir in BOOT_SCRIPT_DIRS {
                parser.parse_config(dir);
            }
        } else {
            parser.parse_config(&bootscript);
        }
        parser.finish()
    }

    /// `GetCurrentSdk` in apex_init_util.cpp.
    fn current_sdk(&self) -> i64 {
        if self.properties.property_or("ro.build.version.codename", "") != "REL" {
            return ANDROID_API_FUTURE as i64;
        }
        parse_int(
            &self.properties.property_or("ro.build.version.sdk", ""),
            i32::MIN as i64,
            i32::MAX as i64,
        )
        .unwrap_or(ANDROID_API_FUTURE as i64)
    }

    /// `CollectRcScriptsFromApex`: `glob("/apex/*/etc/*rc")` (sorted, hidden
    /// entries excluded), skipping `name@version` mounts, directories and
    /// `skip_apexes`.
    pub fn collect_apex_scripts(&self, skip_apexes: &[String]) -> Vec<String> {
        let mut configs = Vec::new();
        let Ok(mut apexes) = self.image.subdirectories("/apex") else {
            return configs;
        };
        apexes.retain(|name| !name.starts_with('.'));
        apexes.sort();
        for apex in apexes {
            if apex.contains('@') || skip_apexes.contains(&apex) {
                continue;
            }
            let etc = format!("/apex/{apex}/etc");
            let Ok(entries) = std::fs::read_dir(self.image.host_path(&etc)) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with("rc") && !name.starts_with('.'))
                .collect();
            names.sort();
            configs.extend(names.into_iter().map(|name| format!("{etc}/{name}")));
        }
        configs.sort();
        configs
    }

    /// `ParseRcScriptsFromAllApexes(bootstrap=false)`, continuing from the
    /// boot scripts' service list. The result's `services` is the complete
    /// list after APEX parsing; its `actions` are only the ones the APEX
    /// scripts add.
    pub fn load_apex_scripts(
        &self,
        boot: &ParsedScripts,
        skip: Option<&[String]>,
    ) -> ParsedScripts {
        let configs = self.collect_apex_scripts(skip.unwrap_or(&[]));
        let filtered = filter_versioned_configs(&configs, self.current_sdk());
        let vendor_apexes: Vec<String> = match &self.vendor_apexes {
            Some(apexes) => apexes.clone(),
            None => self
                .image
                .read("/apex/apex-info-list.xml")
                .map(|bytes| read_apex_info_list(&String::from_utf8_lossy(&bytes)))
                .unwrap_or_default()
                .into_iter()
                .filter(|info| info.partition == "VENDOR" || info.partition == "ODM")
                .map(|info| info.module_name)
                .collect(),
        };
        let start = ParsedScripts {
            services: boot.services.clone(),
            ..ParsedScripts::default()
        };
        let mut parser = Parser::continue_from(self.env(), SectionKind::Apex, start)
            .with_image(self.image)
            .with_vendor_apexes(vendor_apexes);
        for config in filtered {
            if let Err(error) = parser.parse_config_file(&config) {
                parser.push_diagnostic(Diagnostic::error(
                    config.clone(),
                    0,
                    format!("Unable to parse apex configs: {error}"),
                ));
            }
        }
        parser.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_configs_like_init() {
        let configs: Vec<String> = [
            "/apex/a/etc/foo.rc",
            "/apex/a/etc/foo.34rc",
            "/apex/a/etc/foo.37rc",
            "/apex/a/etc/bar.35rc",
            "/apex/a/etc/baz.xrc",
            "/apex/a/etc/src",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            filter_versioned_configs(&configs, 36),
            vec!["/apex/a/etc/bar.35rc", "/apex/a/etc/foo.34rc"]
        );
    }

    #[test]
    fn apex_info_list() {
        let xml = r#"<apex-info-list><apex-info moduleName="com.android.art" partition="SYSTEM" isActive="true"></apex-info><apex-info moduleName="com.vendor.x" partition="VENDOR" isActive="true"/></apex-info-list>"#;
        let infos = read_apex_info_list(xml);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[1].partition, "VENDOR");
    }
}
