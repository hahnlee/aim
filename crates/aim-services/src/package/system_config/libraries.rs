//! SystemConfig's built-in library declarations, ported from AOSP
//! android-16.0.0_r1 `SystemConfig` and `UnboundedSdkLevel`, Copyright
//! (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;

use aim_android_xml::Element;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub filename: String,
    pub dependencies: Vec<String>,
    pub on_bootclasspath_since: Option<String>,
    pub on_bootclasspath_before: Option<String>,
    pub can_be_safely_ignored: bool,
    pub native: bool,
}

impl Library {
    pub(super) fn read(
        e: &Element,
        root: &Path,
        prop: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        let string = |key| e.string(key).map(|s| s.into_owned());
        let name = string("name")?;
        let filename = string("file")?;
        let sdk = Sdk::new(prop);
        if string("min-device-sdk").is_some_and(|v| !sdk.at_least(&v))
            || string("max-device-sdk").is_some_and(|v| !sdk.at_most(&v))
            || !root.join(filename.trim_start_matches('/')).exists()
        {
            return None;
        }
        let on_bootclasspath_since = string("on-bootclasspath-since");
        let on_bootclasspath_before = string("on-bootclasspath-before");
        let can_be_safely_ignored = on_bootclasspath_since
            .as_ref()
            .is_some_and(|v| sdk.at_least(v))
            || on_bootclasspath_before
                .as_ref()
                .is_some_and(|v| !sdk.at_least(v));
        let dependencies = string("dependency").map_or_else(Vec::new, |s| {
            // Java String.split drops trailing empty strings; the empty
            // input itself is a one-element array.
            if s.is_empty() {
                vec![s]
            } else {
                let s = s.trim_end_matches(':');
                if s.is_empty() {
                    Vec::new()
                } else {
                    s.split(':').map(str::to_owned).collect()
                }
            }
        });
        Some(Self {
            name,
            filename,
            dependencies,
            on_bootclasspath_since,
            on_bootclasspath_before,
            can_be_safely_ignored,
            native: false,
        })
    }

    pub(super) fn native(name: String) -> Self {
        Self {
            filename: name.clone(),
            name,
            dependencies: Vec::new(),
            on_bootclasspath_since: None,
            on_bootclasspath_before: None,
            can_be_safely_ignored: false,
            native: true,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Sdk {
    version: i32,
    codename: String,
    known: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_limits_follow_release_and_preview_semantics() {
        let sdk = |codename: &str| Sdk {
            version: 36,
            codename: codename.into(),
            known: ["VanillaIceCream".into(), "Baklava".into()].into(),
        };
        let release = sdk("REL");
        for (version, least, most) in [
            ("35", true, false),
            ("36", true, true),
            ("37", false, true),
            ("Baklava", false, true),
            ("Future.fingerprint", false, true),
            ("", false, true),
            ("invalid", false, true),
        ] {
            assert_eq!(
                (release.at_least(version), release.at_most(version)),
                (least, most),
                "{version}"
            );
        }
        let preview = sdk("Baklava");
        assert!(release.at_least_checked("VanillaIceCream").is_err());
        assert!(!release.at_least_checked("Future").unwrap());
        assert!(release.at_least_checked("").is_err());
        assert!(release.at_least_checked("３６").unwrap());
        assert!(preview.at_least("36"));
        assert!(!preview.at_most("36"));
        assert!(preview.at_most("37"));
        assert!(preview.at_least("Baklava.fingerprint"));
        assert!(preview.at_most("Baklava.fingerprint"));
        assert!(preview.at_least("VanillaIceCream"));
        assert!(!preview.at_most("VanillaIceCream"));
        assert!(!preview.at_least("Future"));
        assert!(preview.at_most("Future"));
    }
}

impl Sdk {
    pub(crate) fn new(prop: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            version: prop("ro.build.version.sdk")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            codename: prop("ro.build.version.codename").unwrap_or_else(|| "REL".into()),
            known: prop("ro.build.version.known_codenames")
                .unwrap_or_default()
                .split(',')
                .map(str::to_owned)
                .collect(),
        }
    }

    fn at_least(&self, version: &str) -> bool {
        self.at_least_checked(version).unwrap_or(false)
    }

    /// UnboundedSdkLevel without SystemConfig's caught-error fallback.
    pub(crate) fn at_least_checked(&self, version: &str) -> Result<bool, String> {
        let first = version.encode_utf16().next().ok_or("empty SDK version")?;
        if char::from_u32(u32::from(first)).is_some_and(char::is_uppercase) {
            let version = version.split('.').next().unwrap();
            let known = self.known.iter().any(|v| v == version);
            if self.codename == "REL" && known {
                return Err(format!("known codename {version} requires a finalized SDK"));
            }
            Ok(self.codename != "REL" && known)
        } else {
            let sdk =
                super::decimal_uid(version).ok_or_else(|| format!("invalid SDK {version}"))?;
            Ok(self.version >= sdk)
        }
    }

    fn at_most(&self, version: &str) -> bool {
        if version.starts_with(char::is_uppercase) {
            let version = version.split('.').next().unwrap();
            self.codename == "REL"
                || !self.known.iter().any(|v| v == version)
                || self.codename == version
        } else {
            // SystemConfig catches IllegalArgumentException as true.
            version.parse::<i32>().map_or(true, |v| {
                if self.codename == "REL" {
                    self.version <= v
                } else {
                    self.version < v
                }
            })
        }
    }
}
