//! Platform compat's decisions for a process start: which compat changes
//! are disabled and which are logged for an app, as
//! `com.android.server.compat.CompatConfig` decides them at
//! `android-16.0.0_r1` (`getDisabledChanges`, `getLoggableChanges`,
//! `CompatChange.isEnabled`), from the image's change declarations and
//! the overrides of the product and of the device (`initOverrides`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// `Build.VERSION_CODES.CUR_DEVELOPMENT`.
const CUR_DEVELOPMENT: i32 = 10000;

/// The overrides of the product and the device, as `initOverrides` reads
/// them (`APP_COMPAT_DATA_DIR`'s file after `STATIC_OVERRIDES_PRODUCT_DIR`'s).
const STATIC_OVERRIDES: &str = "product/etc/appcompat/compat_framework_overrides.xml";
const DYNAMIC_OVERRIDES: &str = "misc/appcompat/compat_framework_overrides.xml";

/// One `<compat-change>` of a `compatconfig` file, and its overrides.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    /// `CompatibilityChangeInfo`'s: `enableAfterTargetSdk + 1`, else
    /// `enableSinceTargetSdk`, else -1.
    enable_since_target_sdk: i32,
    disabled: bool,
    /// `mEvaluatedOverrides`: whether it is enabled for a package.
    evaluated: HashMap<String, bool>,
    /// `mRawOverrides`: each package's override, its version codes'
    /// range and whether it enables.
    raw: HashMap<String, (i64, i64, bool)>,
}

/// The changes the image declares, by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompatConfig {
    changes: BTreeMap<i64, Change>,
    /// `AndroidBuildClassifier.platformTargetSdk`.
    platform_target_sdk: i32,
}

impl CompatConfig {
    /// The declarations below `root`: `/system/etc/compatconfig` and each
    /// APEX's `etc/compatconfig` (`CompatConfig.create`), for a platform
    /// of SDK `sdk` (`ro.build.version.sdk`) whose codename is `codename`
    /// (`ro.build.version.codename`; `REL` for a final build).
    pub fn load(root: &Path, data: &Path, sdk: i32, codename: &str) -> Result<Self, String> {
        let mut dirs = vec![root.join("system/etc/compatconfig")];
        if let Ok(apexes) = std::fs::read_dir(root.join("apex")) {
            let mut apexes: Vec<_> = apexes.flatten().map(|e| e.path()).collect();
            apexes.sort();
            dirs.extend(apexes.into_iter().map(|apex| apex.join("etc/compatconfig")));
        }
        let mut config = CompatConfig {
            changes: BTreeMap::new(),
            platform_target_sdk: if codename == "REL" {
                sdk
            } else {
                CUR_DEVELOPMENT
            },
        };
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            files.sort();
            for file in files
                .iter()
                .filter(|f| f.extension().is_some_and(|e| e == "xml"))
            {
                let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
                config
                    .read(&bytes)
                    .map_err(|e| format!("{}: {e}", file.display()))?;
            }
        }
        config.load_overrides(&root.join(STATIC_OVERRIDES));
        let dynamic = data.join(DYNAMIC_OVERRIDES);
        let backup = data.join(format!("{DYNAMIC_OVERRIDES}.bak"));
        // A backup is the file a write did not finish replacing.
        config.load_overrides(if backup.exists() { &backup } else { &dynamic });
        Ok(config)
    }

    /// `loadOverrides` of `file`, if it exists: each known change's
    /// overrides (`CompatChange.loadOverrides`); a file that does not
    /// parse is skipped whole.
    fn load_overrides(&mut self, file: &Path) {
        let Ok(bytes) = std::fs::read(file) else { return };
        match parse_overrides(&bytes) {
            Ok(overrides) => {
                for (id, deferred, validated, raw) in overrides {
                    let Some(change) = self.changes.get_mut(&id) else {
                        eprintln!("app host: Change ID {id} not found. Skipping overrides for it.");
                        continue;
                    };
                    for (package, enabled) in deferred {
                        change.raw.insert(package, (i64::MIN, i64::MAX, enabled));
                    }
                    for (package, enabled) in validated {
                        change.evaluated.insert(package.clone(), enabled);
                        change.raw.insert(package, (i64::MIN, i64::MAX, enabled));
                    }
                    for (package, range) in raw {
                        change.raw.insert(package, range);
                    }
                }
            }
            Err(e) => eprintln!("app host: Error processing {} {e}", file.display()),
        }
    }

    fn read(&mut self, bytes: &[u8]) -> Result<(), String> {
        let root = aim_android_xml::read(bytes)?;
        for change in root.children().filter(|c| c.name == "compat-change") {
            let id = change.long("id")?.ok_or("compat-change without an id")?;
            let after = change.int("enableAfterTargetSdk")?.unwrap_or(-1);
            let since = change.int("enableSinceTargetSdk")?.unwrap_or(-1);
            self.changes.insert(
                id,
                Change {
                    enable_since_target_sdk: if after > 0 {
                        after + 1
                    } else if since > 0 {
                        since
                    } else {
                        -1
                    },
                    disabled: change.bool("disabled")?.unwrap_or(false),
                    evaluated: HashMap::new(),
                    raw: HashMap::new(),
                },
            );
        }
        Ok(())
    }

    /// `CompatChange.isEnabled` for an app of `package` targeting
    /// `target_sdk`: its override, else the declaration.
    fn enabled(&self, change: &Change, package: Option<&str>, target_sdk: i32) -> bool {
        if let Some(&enabled) = package.and_then(|p| change.evaluated.get(p)) {
            return enabled;
        }
        if change.disabled {
            return false;
        }
        if change.enable_since_target_sdk != -1 {
            return target_sdk.min(self.platform_target_sdk) >= change.enable_since_target_sdk;
        }
        true
    }

    /// Whether change `id` is enabled for an app of `package` targeting
    /// `target_sdk`; an undeclared change is enabled
    /// (`CompatConfig.isChangeEnabled`).
    pub fn is_enabled(&self, id: i64, package: Option<&str>, target_sdk: i32) -> bool {
        self.changes
            .get(&id)
            .is_none_or(|c| self.enabled(c, package, target_sdk))
    }

    /// `willChangeBeEnabled`: change `id` for `package` once installed,
    /// by an override for all its versions, else its default.
    pub fn will_be_enabled(&self, id: i64, package: &str) -> bool {
        self.changes.get(&id).is_none_or(|c| match c.raw.get(package) {
            Some(&(i64::MIN, i64::MAX, enabled)) => enabled,
            _ => !c.disabled,
        })
    }

    /// Change `id`'s value for a uid of no package
    /// (`CompatConfig.defaultChangeIdValue`): whether it is not disabled.
    pub fn default_value(&self, id: i64) -> bool {
        self.changes.get(&id).is_none_or(|c| !c.disabled)
    }

    /// `getDisabledChanges`, sorted.
    pub fn disabled_changes(&self, package: &str, target_sdk: i32) -> Vec<i64> {
        self.changes
            .iter()
            .filter(|(_, c)| !self.enabled(c, Some(package), target_sdk))
            .map(|(&id, _)| id)
            .collect()
    }

    /// `getLoggableChanges`, sorted: the enabled changes gated by the
    /// app's target SDK or the development one.
    pub fn loggable_changes(&self, package: &str, target_sdk: i32) -> Vec<i64> {
        self.changes
            .iter()
            .filter(|(_, c)| {
                let max = c.enable_since_target_sdk;
                self.enabled(c, Some(package), target_sdk)
                    && max > 0
                    && (max == CUR_DEVELOPMENT || max == target_sdk)
            })
            .map(|(&id, _)| id)
            .collect()
    }
}

/// A change's overrides in a file: its deferred, validated and raw ones.
type ChangeOverrides = (i64, Vec<(String, bool)>, Vec<(String, bool)>, Vec<(String, (i64, i64, bool))>);

/// `com.android.server.compat.overrides.XmlParser.read`: the
/// `<change-overrides>` of `<overrides>`.
fn parse_overrides(bytes: &[u8]) -> Result<Vec<ChangeOverrides>, String> {
    let root = aim_android_xml::read(bytes)?;
    let value = |e: &aim_android_xml::Element| -> Result<(String, bool), String> {
        let package = e.string("packageName").ok_or("an override without a packageName")?;
        // xsdc's reader takes a boolean as `Boolean.parseBoolean` does.
        let enabled = e.string("enabled").ok_or("an override without enabled")?.eq_ignore_ascii_case("true");
        Ok((package.to_string(), enabled))
    };
    let mut changes = Vec::new();
    for change in root.children().filter(|c| c.name == "change-overrides") {
        let id = change.long("changeId")?.ok_or("change-overrides without a changeId")?;
        let values = |name: &str| -> Result<Vec<(String, bool)>, String> {
            change.children().filter(|e| e.name == name).flat_map(|e| e.children().filter(|v| v.name == "override-value")).map(value).collect()
        };
        let (deferred, validated) = (values("deferred")?, values("validated")?);
        let mut raw = Vec::new();
        for v in change.children().filter(|e| e.name == "raw").flat_map(|e| e.children().filter(|v| v.name == "raw-override-value")) {
            let (package, enabled) = value(v)?;
            let min = v.long("minVersionCode")?.unwrap_or(i64::MIN);
            let max = v.long("maxVersionCode")?.unwrap_or(i64::MAX);
            raw.push((package, (min, max, enabled)));
        }
        changes.push((id, deferred, validated, raw));
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CompatConfig {
        let mut c = CompatConfig {
            platform_target_sdk: 36,
            ..Default::default()
        };
        c.read(
            br#"<config>
            <compat-change disabled="true" id="1" name="OFF" />
            <compat-change enableAfterTargetSdk="29" id="2" name="AFTER_Q" />
            <compat-change enableSinceTargetSdk="35" id="3" name="SINCE_V" />
            <compat-change enableSinceTargetSdk="37" id="4" name="FUTURE" />
            <compat-change id="5" name="ON" />
        </config>"#,
        )
        .unwrap();
        c
    }

    #[test]
    fn disabled_follow_target_sdk() {
        let c = config();
        assert_eq!(c.disabled_changes("a", 29), [1, 2, 3, 4]);
        assert_eq!(c.disabled_changes("a", 35), [1, 4]);
        // A change gated past the platform stays off whatever the app targets.
        assert_eq!(c.disabled_changes("a", 10000), [1, 4]);
        assert!(c.is_enabled(99, None, 1));
    }

    #[test]
    fn loggable_are_the_enabled_ones_gated_at_the_target() {
        let c = config();
        assert_eq!(c.loggable_changes("a", 35), [3]);
        assert_eq!(c.loggable_changes("a", 30), [2]);
    }

    #[test]
    fn a_validated_override_decides_for_its_package() {
        let mut c = config();
        let overrides = parse_overrides(
            br#"<overrides>
            <change-overrides changeId="1"><validated><override-value packageName="p" enabled="true" /><override-value packageName="d" enabled="disable" /></validated></change-overrides>
            <change-overrides changeId="3"><raw><raw-override-value packageName="q" maxVersionCode="5" enabled="false" /></raw></change-overrides>
        </overrides>"#,
        )
        .unwrap();
        for (id, deferred, validated, raw) in overrides {
            let change = c.changes.get_mut(&id).unwrap();
            assert!(deferred.is_empty());
            change.evaluated.extend(validated.iter().cloned());
            change.raw.extend(validated.into_iter().map(|(p, e)| (p, (i64::MIN, i64::MAX, e))));
            change.raw.extend(raw);
        }
        assert!(c.is_enabled(1, Some("p"), 29));
        assert!(!c.is_enabled(1, Some("other"), 29));
        // Not "true" reads as false.
        assert!(!c.is_enabled(1, Some("d"), 29));
        // A raw override waits for its package's version to be checked.
        assert!(c.is_enabled(3, Some("q"), 35));
        assert!(c.will_be_enabled(1, "p"));
        assert!(c.will_be_enabled(3, "q"));
    }
}
