//! UpdateOwnershipHelper contributor relations at android-16.0.0_r1 (#825).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! The resource loader supplies validated list contents; pending eligible
//! providers prevent reads from treating an incomplete boot index as empty.
use crate::package::{pkg::AndroidPackage, settings::Package};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateOwnership {
    contributors: BTreeMap<String, BTreeSet<String>>,
    pending: BTreeSet<String>,
}

impl UpdateOwnership {
    /// InstallPackageHelper queues reads only for eligible system providers.
    /// Rescanning a provider retains existing contributions until removal;
    /// addToUpdateOwnerDenyList accumulates rather than replacing its list.
    pub fn queue(&mut self, package: &Package, parsed: &AndroidPackage) {
        if package.flags
            & (crate::package::settings::FLAG_SYSTEM
                | crate::package::info::FLAG_UPDATED_SYSTEM_APP)
            != 0
            && parsed.properties.as_ref().is_some_and(|properties| {
                properties.iter().any(|(name, _)| {
                    name == "android.app.PROPERTY_LEGACY_UPDATE_OWNERSHIP_DENYLIST"
                })
            })
            && parsed.uses_permissions.iter().any(|permission| {
                matches!(
                    permission.name.as_deref(),
                    Some(
                        "android.permission.INSTALL_PACKAGES"
                            | "android.permission.INSTALL_PACKAGE_UPDATES"
                    )
                )
            })
        {
            self.pending.insert(package.name.clone());
        }
    }

    /// Record a completed, validated resource read. Empty lists complete the
    /// read without withdrawing older contributions, as the original does.
    pub fn add(&mut self, provider: &str, contents: &[String]) {
        for package in contents {
            self.contributors
                .entry(package.clone())
                .or_default()
                .insert(provider.into());
        }
        self.pending.remove(provider);
    }

    /// Remove this provider's contributions; other providers keep the opt-out.
    /// Retire its pending read in this owner; asynchronous work ordering
    /// belongs to the boot/commit caller (#825).
    pub fn remove(&mut self, provider: &str) {
        self.contributors.retain(|_, providers| {
            providers.remove(provider);
            !providers.is_empty()
        });
        self.pending.remove(provider);
    }

    pub fn is_denylisted(&self, package: &str) -> Result<bool, &'static str> {
        self.ready()?;
        Ok(self.contributors.contains_key(package))
    }

    pub fn is_provider(&self, package: Option<&str>) -> Result<bool, &'static str> {
        let Some(package) = package else {
            return Ok(false);
        };
        self.ready()?;
        Ok(self
            .contributors
            .values()
            .any(|providers| providers.contains(package)))
    }

    fn ready(&self) -> Result<(), &'static str> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err("update ownership provider resource reads are incomplete (#825)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::{Property, PropertyValue, UsesPermission};

    #[test]
    fn contributors_overlap_accumulate_and_retire_idempotently() {
        let mut owner = UpdateOwnership::default();
        owner.add("a", &["one".into(), "shared".into(), "shared".into()]);
        owner.add("b", &["shared".into()]);
        owner.add("a", &["two".into()]);
        owner.add("a", &[]);
        for name in ["one", "two", "shared"] {
            assert_eq!(owner.is_denylisted(name), Ok(true));
        }
        owner.remove("a");
        assert_eq!(owner.is_denylisted("one"), Ok(false));
        assert_eq!(owner.is_denylisted("two"), Ok(false));
        assert_eq!(owner.is_denylisted("shared"), Ok(true));
        assert_eq!(owner.is_provider(Some("a")), Ok(false));
        assert_eq!(owner.is_provider(Some("b")), Ok(true));
        owner.remove("a");
        owner.remove("b");
        assert_eq!(owner.is_denylisted("shared"), Ok(false));
        assert_eq!(owner.is_provider(None), Ok(false));
    }

    #[test]
    fn eligible_pending_reads_block_queries_until_completed_or_removed() {
        let parsed = AndroidPackage {
            properties: Some(vec![(
                "android.app.PROPERTY_LEGACY_UPDATE_OWNERSHIP_DENYLIST".into(),
                Property {
                    name: None,
                    package_name: None,
                    class_name: None,
                    value: PropertyValue::Resource(123),
                },
            )]),
            uses_permissions: vec![UsesPermission {
                name: Some("android.permission.INSTALL_PACKAGES".into()),
                flags: 0,
            }],
            ..AndroidPackage::default()
        };
        let mut package = Package {
            name: "provider".into(),
            ..Package::default()
        };
        let mut owner = UpdateOwnership::default();
        owner.queue(&package, &parsed);
        assert_eq!(owner.is_denylisted("one"), Ok(false));
        package.flags = crate::package::info::FLAG_UPDATED_SYSTEM_APP;
        let mut missing = parsed.clone();
        missing.uses_permissions.clear();
        owner.queue(&package, &missing);
        missing = parsed.clone();
        missing.properties = None;
        owner.queue(&package, &missing);
        assert_eq!(owner.is_denylisted("one"), Ok(false));
        owner.queue(&package, &parsed);
        assert!(owner.is_denylisted("one").is_err());
        owner.add("provider", &[]);
        package.flags = crate::package::settings::FLAG_SYSTEM;
        owner.queue(&package, &parsed);
        assert!(owner.is_denylisted("one").is_err());
        owner.add("provider", &["one".into()]);
        assert_eq!(owner.is_denylisted("one"), Ok(true));
        owner.queue(&package, &parsed);
        owner.remove("provider");
        assert_eq!(owner.is_denylisted("one"), Ok(false));
    }
}
