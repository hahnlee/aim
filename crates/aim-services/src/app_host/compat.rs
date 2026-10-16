//! Platform compat's decisions for a process start: which compat changes
//! are disabled and which are logged for an app, as
//! `com.android.server.compat.CompatConfig` decides them at
//! `android-16.0.0_r1` (`getDisabledChanges`, `getLoggableChanges`,
//! `CompatChange.isEnabled`), from the image's change declarations.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::path::Path;

/// `Build.VERSION_CODES.CUR_DEVELOPMENT`.
const CUR_DEVELOPMENT: i32 = 10000;

/// One `<compat-change>` of a `compatconfig` file.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    /// `CompatibilityChangeInfo`'s: `enableAfterTargetSdk + 1`, else
    /// `enableSinceTargetSdk`, else -1.
    enable_since_target_sdk: i32,
    disabled: bool,
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
    pub fn load(root: &Path, sdk: i32, codename: &str) -> Result<Self, String> {
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
        Ok(config)
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
                },
            );
        }
        Ok(())
    }

    /// `CompatChange.isEnabled` without overrides.
    fn enabled(&self, change: &Change, target_sdk: i32) -> bool {
        if change.disabled {
            return false;
        }
        if change.enable_since_target_sdk != -1 {
            return target_sdk.min(self.platform_target_sdk) >= change.enable_since_target_sdk;
        }
        true
    }

    /// Whether change `id` is enabled for an app targeting `target_sdk`;
    /// an undeclared change is enabled (`CompatConfig.isChangeEnabled`).
    pub fn is_enabled(&self, id: i64, target_sdk: i32) -> bool {
        self.changes
            .get(&id)
            .is_none_or(|c| self.enabled(c, target_sdk))
    }

    /// `getDisabledChanges`, sorted.
    pub fn disabled_changes(&self, target_sdk: i32) -> Vec<i64> {
        self.changes
            .iter()
            .filter(|(_, c)| !self.enabled(c, target_sdk))
            .map(|(&id, _)| id)
            .collect()
    }

    /// `getLoggableChanges`, sorted: the enabled changes gated by the
    /// app's target SDK or the development one.
    pub fn loggable_changes(&self, target_sdk: i32) -> Vec<i64> {
        self.changes
            .iter()
            .filter(|(_, c)| {
                let max = c.enable_since_target_sdk;
                self.enabled(c, target_sdk)
                    && max > 0
                    && (max == CUR_DEVELOPMENT || max == target_sdk)
            })
            .map(|(&id, _)| id)
            .collect()
    }
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
        assert_eq!(c.disabled_changes(29), [1, 2, 3, 4]);
        assert_eq!(c.disabled_changes(35), [1, 4]);
        // A change gated past the platform stays off whatever the app targets.
        assert_eq!(c.disabled_changes(10000), [1, 4]);
        assert!(c.is_enabled(99, 1));
    }

    #[test]
    fn loggable_are_the_enabled_ones_gated_at_the_target() {
        let c = config();
        assert_eq!(c.loggable_changes(35), [3]);
        assert_eq!(c.loggable_changes(30), [2]);
    }
}
