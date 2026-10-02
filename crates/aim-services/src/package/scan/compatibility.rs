//! PackageBackwardCompatibility and library updaters at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::{
    info::java_hash,
    pkg::AndroidPackage,
    system_config::{Library, Sdk, SystemConfig},
};
use std::collections::BTreeSet;

const HTTP: &str = "org.apache.http.legacy";
const HIDL_BASE: &str = "android.hidl.base-V1.0-java";
const HIDL_MANAGER: &str = "android.hidl.manager-V1.0-java";
const TEST_RUNNER: &str = "android.test.runner";
const TEST_MOCK: &str = "android.test.mock";
const TEST_BASE: &str = "android.test.base";

#[derive(Clone, Debug)]
pub struct LibraryCompatibility {
    libraries: Vec<Library>,
    sdk: Sdk,
    test_base_on_bootclasspath: bool,
}

impl LibraryCompatibility {
    /// Image SystemConfig/Build and selected boot classpath policy. No class
    /// loading or environment feature flags. Keeps Android ArrayMap order.
    pub fn new(
        config: &SystemConfig,
        prop: &dyn Fn(&str) -> Option<String>,
        test_base_on_bootclasspath: bool,
    ) -> Result<Self, String> {
        let names: BTreeSet<_> = config.library_order.iter().collect();
        if names.len() != config.library_order.len()
            || names.len() != config.libraries.len()
            || !config.libraries.keys().all(|n| names.contains(n))
        {
            return Err("SystemConfig library insertion order is incomplete".into());
        }
        let mut libraries: Vec<_> = config
            .library_order
            .iter()
            .map(|n| config.libraries[n].clone())
            .collect();
        libraries.sort_by_key(|l| java_hash(&l.name));
        Ok(Self {
            libraries,
            sdk: Sdk::new(prop),
            test_base_on_bootclasspath,
        })
    }

    /// Before validation/reconciliation. Non-system apps need the owner's
    /// resolved PlatformCompat change 133396946 when test.base is separate.
    /// System apps follow the original target-SDK fallback without querying it.
    /// Missing decisions or invalid image policy reject without mutation.
    pub fn apply(
        &self,
        pkg: &mut AndroidPackage,
        is_system_app: bool,
        is_updated_system_app: bool,
        remove_test_base: Option<bool>,
    ) -> Result<(), String> {
        let mut lists = Lists {
            required: pkg.uses_libraries.clone(),
            optional: pkg.uses_optional_libraries.clone(),
        };
        for name in [
            "wear-sdk",
            "android.net.ipsec.ike",
            "com.google.android.maps",
        ] {
            lists.remove(name);
        }
        if pkg.target_sdk_version < 28 {
            lists.prefix_required(HTTP);
        }
        if pkg.target_sdk_version <= 28 && (is_system_app || is_updated_system_app) {
            lists.prefix_required(HIDL_BASE);
            lists.prefix_required(HIDL_MANAGER);
        } else {
            lists.remove(HIDL_BASE);
            lists.remove(HIDL_MANAGER);
        }
        lists.prefix_dependency(TEST_RUNNER, TEST_MOCK);
        if self.test_base_on_bootclasspath {
            lists.remove(TEST_BASE);
        } else {
            let enabled = if is_system_app {
                pkg.target_sdk_version > 29
            } else {
                remove_test_base.ok_or("unresolved PlatformCompat change 133396946")?
            };
            if enabled {
                lists.prefix_dependency(TEST_RUNNER, TEST_BASE);
            } else {
                lists.prefix_required(TEST_BASE);
            }
        }
        for library in &self.libraries {
            if let Some(before) = &library.on_bootclasspath_before {
                let first = before
                    .encode_utf16()
                    .next()
                    .ok_or("empty bootclasspath SDK")?;
                let target_before =
                    if char::from_u32(u32::from(first)).is_some_and(char::is_uppercase) {
                        pkg.target_sdk_version < 10000
                    } else {
                        pkg.target_sdk_version
                            < crate::package::system_config::decimal_uid(before)
                                .ok_or_else(|| format!("invalid bootclasspath SDK {before}"))?
                    };
                if target_before && self.sdk.at_least_checked(before)? {
                    lists.prefix_required(&library.name);
                }
            }
            if library.can_be_safely_ignored {
                lists.remove(&library.name);
            }
        }
        pkg.uses_libraries = lists.required;
        pkg.uses_optional_libraries = lists.optional;
        Ok(())
    }
}

struct Lists {
    required: Vec<String>,
    optional: Vec<String>,
}
impl Lists {
    fn contains(&self, name: &str) -> bool {
        self.required
            .iter()
            .chain(&self.optional)
            .any(|n| n == name)
    }
    fn remove(&mut self, name: &str) {
        // PackageImpl's ArrayList.remove removes the first occurrence only.
        for list in [&mut self.required, &mut self.optional] {
            if let Some(i) = list.iter().position(|n| n == name) {
                list.remove(i);
            }
        }
    }
    fn prefix_required(&mut self, name: &str) {
        if !self.contains(name) {
            self.required.insert(0, name.into());
        }
    }
    fn prefix_dependency(&mut self, existing: &str, implicit: &str) {
        if !self.contains(implicit) {
            if self.required.iter().any(|n| n == existing) {
                self.required.insert(0, implicit.into());
            } else if self.optional.iter().any(|n| n == existing) {
                self.optional.insert(0, implicit.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(on_bcp: bool) -> LibraryCompatibility {
        LibraryCompatibility::new(
            &Default::default(),
            &|p| match p {
                "ro.build.version.sdk" => Some("36".into()),
                "ro.build.version.codename" => Some("REL".into()),
                _ => None,
            },
            on_bcp,
        )
        .unwrap()
    }

    #[test]
    fn required_optional_order_and_sdk_boundaries_follow_updater_sequence() {
        for (sdk, expected) in [
            (
                27,
                vec![
                    TEST_BASE,
                    TEST_MOCK,
                    HIDL_MANAGER,
                    HIDL_BASE,
                    HTTP,
                    TEST_RUNNER,
                ],
            ),
            (
                28,
                vec![TEST_BASE, TEST_MOCK, HIDL_MANAGER, HIDL_BASE, TEST_RUNNER],
            ),
            (29, vec![TEST_BASE, TEST_MOCK, TEST_RUNNER]),
            (30, vec![TEST_BASE, TEST_MOCK, TEST_RUNNER]),
        ] {
            let mut pkg = AndroidPackage {
                target_sdk_version: sdk,
                uses_libraries: vec![
                    TEST_RUNNER.into(),
                    "wear-sdk".into(),
                    "com.google.android.maps".into(),
                ],
                uses_optional_libraries: vec!["android.net.ipsec.ike".into()],
                ..Default::default()
            };
            policy(false)
                .apply(&mut pkg, true, false, Some(false))
                .unwrap();
            assert_eq!(pkg.uses_libraries, expected);
            assert!(pkg.uses_optional_libraries.is_empty());
        }
        let mut pkg = AndroidPackage {
            target_sdk_version: 30,
            uses_optional_libraries: vec![TEST_RUNNER.into()],
            ..Default::default()
        };
        policy(false)
            .apply(&mut pkg, false, false, Some(true))
            .unwrap();
        assert_eq!(
            pkg.uses_optional_libraries,
            [TEST_BASE, TEST_MOCK, TEST_RUNNER]
        );
        assert!(pkg.uses_libraries.is_empty());
        policy(true).apply(&mut pkg, false, false, None).unwrap();
        assert_eq!(pkg.uses_optional_libraries, [TEST_MOCK, TEST_RUNNER]);
        pkg.target_sdk_version = 27;
        pkg.uses_optional_libraries.push(HTTP.into());
        policy(true).apply(&mut pkg, false, false, None).unwrap();
        assert!(pkg.uses_libraries.is_empty()); // already optional never promoted
    }

    #[test]
    fn missing_decisions_and_invalid_sdk_policy_reject_without_mutation() {
        let mut pkg = AndroidPackage {
            target_sdk_version: 27,
            uses_libraries: vec!["wear-sdk".into()],
            ..Default::default()
        };
        let before = pkg.clone();
        assert!(policy(false).apply(&mut pkg, false, false, None).is_err());
        assert_eq!(pkg, before);
        let mut config = SystemConfig::default();
        let lib = |name: &str, version: &str| Library {
            name: name.into(),
            filename: "/system/lib.jar".into(),
            dependencies: vec![],
            on_bootclasspath_since: None,
            on_bootclasspath_before: Some(version.into()),
            can_be_safely_ignored: false,
            native: false,
        };
        config.library_order = vec!["BB".into(), "Aa".into()];
        config.libraries.insert("BB".into(), lib("BB", "28"));
        config.libraries.insert("Aa".into(), lib("Aa", "28"));
        let props = |p: &str| (p == "ro.build.version.sdk").then(|| "36".into());
        let updater = LibraryCompatibility::new(&config, &props, true).unwrap();
        updater.apply(&mut pkg, true, false, None).unwrap();
        assert_eq!(pkg.uses_libraries[..2], ["Aa", "BB"]); // equal hash, reverse insertion
        config
            .libraries
            .get_mut("Aa")
            .unwrap()
            .on_bootclasspath_before = Some("invalid".into());
        let updater = LibraryCompatibility::new(&config, &props, true).unwrap();
        let before = pkg.clone();
        assert!(updater.apply(&mut pkg, true, false, None).is_err());
        assert_eq!(pkg, before);
        config.library_order.pop();
        assert!(LibraryCompatibility::new(&config, &props, true).is_err());
    }
}
