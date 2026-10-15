//! AndroidPackageUtils.getHiddenApiEnforcementPolicy, android-16.0.0_r1.
//! Copyright AOSP, Apache License 2.0. The allowlist belongs to image SystemConfig.
use super::SigningScan;
use crate::package::{
    pkg::{AndroidPackage, booleans},
    settings,
};

fn policy(code: Option<&AndroidPackage>, system: bool, allowlisted: bool) -> i32 {
    if code.is_some_and(|pkg| {
        pkg.is(booleans::SIGNED_WITH_PLATFORM_KEY)
            || system && (pkg.is(booleans::USES_NON_SDK_API) || allowlisted)
    }) {
        0 // ApplicationInfo.HIDDEN_API_ENFORCEMENT_DISABLED
    } else {
        2 // ApplicationInfo.HIDDEN_API_ENFORCEMENT_ENABLED
    }
}

impl SigningScan {
    pub fn hidden_api_enforcement_policy(
        &self,
        name: &str,
        factory: bool,
    ) -> Result<Option<i32>, String> {
        if !self.capture_ready() {
            return Err("scan metadata is not finalized".into());
        }
        let (packages, code) = if factory {
            (
                &self.settings.disabled_system_packages,
                &self.disabled_loaded,
            )
        } else {
            (&self.settings.packages, &self.loaded)
        };
        Ok(packages.iter().find(|p| p.name == name).map(|setting| {
            policy(
                code.get(name).map(|p| &p.package),
                setting.flags & settings::FLAG_SYSTEM != 0,
                self.hidden_api_allowlist.contains(name),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_code_platform_signing_system_request_and_allowlist_are_distinct() {
        for system in [false, true] {
            for allowlisted in [false, true] {
                assert_eq!(policy(None, system, allowlisted), 2);
                for signed in [false, true] {
                    for requested in [false, true] {
                        let pkg = AndroidPackage {
                            booleans: if signed {
                                booleans::SIGNED_WITH_PLATFORM_KEY
                            } else {
                                0
                            } | if requested {
                                booleans::USES_NON_SDK_API
                            } else {
                                0
                            },
                            ..Default::default()
                        };
                        assert_eq!(
                            policy(Some(&pkg), system, allowlisted),
                            if signed || system && (requested || allowlisted) {
                                0
                            } else {
                                2
                            }
                        );
                    }
                }
            }
        }
    }
}
