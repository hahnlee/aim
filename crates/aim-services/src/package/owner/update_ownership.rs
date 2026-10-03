//! UpdateOwnershipHelper contributor relations at android-16.0.0_r1 (#825).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! The resource loader supplies validated list contents; pending eligible
//! providers prevent reads from treating an incomplete boot index as empty.
use crate::package::{pkg::AndroidPackage, settings::Package};
use std::collections::{BTreeMap, BTreeSet};

/// Read the selected XML asset from the caller's complete ResourcesManager
/// asset inventory (tables followed by overlays). A missing asset is an error;
/// it must not complete a pending provider read with an empty contribution.
pub fn read_denylist(
    resources: &crate::package::parse::resources::Resources<'_>,
    id: u32,
    file: impl FnOnce(usize, &str) -> aim_apps::res::Result<Vec<u8>>,
) -> aim_apps::res::Result<Vec<String>> {
    let (table, path) = resources
        .resource_string_source(id)
        .ok_or_else(|| aim_apps::res::bad("update ownership XML resource is unresolved"))?;
    let bytes = file(table, path)?;
    let mut events = aim_apps::res::XmlEvents::new(&bytes)?;
    let mut contents = Vec::new();
    while let Some(event) = events.next() {
        if event? == aim_apps::res::XmlEvent::Start("deny-ownership".into()) {
            if let Some(aim_apps::res::XmlEvent::Text(text)) = events.next().transpose()? {
                if !text.chars().all(java_whitespace) && !contents.contains(&text) {
                    contents.push(text);
                    // The original checks after adding, so it retains 501.
                    if contents.len() > 500 {
                        break;
                    }
                }
            }
        }
    }
    Ok(crate::package::info::array_order(contents, |s| s))
}

// String.isBlank uses Character.isWhitespace, which excludes nonbreaking
// spaces and includes the four information-separator control characters.
fn java_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{0020}'
        | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}')
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateOwnership {
    contributors: BTreeMap<String, BTreeSet<String>>,
    pending: BTreeSet<String>,
}

impl UpdateOwnership {
    /// The posted InstallPackageHelper read, after package settings commit.
    /// A failed read preserves both the pending provider and saved ownership.
    /// The caller must serialize this with removals and publish changed records.
    pub fn complete_resource_read(
        &mut self,
        provider: &str,
        settings: &mut crate::package::settings::Settings,
        config: &crate::package::system_config::SystemConfig,
        resources: &crate::package::parse::resources::Resources<'_>,
        id: u32,
        file: impl FnOnce(usize, &str) -> aim_apps::res::Result<Vec<u8>>,
    ) -> aim_apps::res::Result<Vec<String>> {
        if !self.pending.contains(provider) {
            return Err(aim_apps::res::bad(
                "update ownership provider read is not queued",
            ));
        }
        let contents = read_denylist(resources, id, file)?;
        Ok(self.apply_contents(provider, &contents, settings, config))
    }

    fn apply_contents(
        &mut self,
        provider: &str,
        contents: &[String],
        settings: &mut crate::package::settings::Settings,
        config: &crate::package::system_config::SystemConfig,
    ) -> Vec<String> {
        self.add(provider, contents);
        let mut changed = Vec::new();
        for name in contents {
            if config.system_app_update_owners.contains_key(name) {
                continue;
            }
            if let Some(package) = settings.packages.iter_mut().find(|p| &p.name == name)
                && package.install_source.update_owner.take().is_some()
            {
                changed.push(name.clone());
            }
        }
        changed
    }

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
    fn completed_lists_clear_only_active_targets_without_system_config_owner() {
        use crate::package::{settings::Settings, system_config::SystemConfig};
        let package = |name: &str| {
            let mut package = Package {
                name: name.into(),
                ..Package::default()
            };
            package.install_source.update_owner = Some("saved.installer".into());
            package.install_source.installer = Some("installer".into());
            package
        };
        let mut settings = Settings {
            packages: ["free", "fixed", "unlisted", "same-as-provider"]
                .map(package)
                .to_vec(),
            disabled_system_packages: vec![package("free")],
            ..Settings::default()
        };
        let mut config = SystemConfig::default();
        config
            .system_app_update_owners
            .insert("fixed".into(), "different.installer".into());
        let mut owner = UpdateOwnership::default();
        owner.pending.insert("same-as-provider".into());
        assert_eq!(
            owner.apply_contents(
                "same-as-provider",
                &[
                    "free".into(),
                    "fixed".into(),
                    "missing".into(),
                    "same-as-provider".into()
                ],
                &mut settings,
                &config
            ),
            ["free", "same-as-provider"]
        );
        assert_eq!(owner.is_denylisted("fixed"), Ok(true));
        assert_eq!(owner.is_denylisted("missing"), Ok(true));
        assert_eq!(settings.packages[0].install_source.update_owner, None);
        assert_eq!(
            settings.packages[0].install_source.installer.as_deref(),
            Some("installer")
        );
        assert_eq!(
            settings.packages[1].install_source.update_owner.as_deref(),
            Some("saved.installer")
        );
        assert_eq!(
            settings.packages[2].install_source.update_owner.as_deref(),
            Some("saved.installer")
        );
        assert_eq!(
            settings.disabled_system_packages[0]
                .install_source
                .update_owner
                .as_deref(),
            Some("saved.installer")
        );
        let before = settings.clone();
        assert!(
            owner
                .apply_contents("same-as-provider", &[], &mut settings, &config)
                .is_empty()
        );
        assert_eq!(settings, before);
        assert_eq!(owner.is_provider(Some("same-as-provider")), Ok(true));
    }

    #[test]
    fn blank_text_uses_java_character_whitespace() {
        for c in [
            '\t', '\n', '\u{001c}', ' ', '\u{1680}', '\u{2007}', '\u{202f}', '\u{3000}',
        ] {
            assert_eq!(java_whitespace(c), !matches!(c, '\u{2007}' | '\u{202f}'));
        }
        for c in ['\u{0085}', '\u{00a0}', '\u{200b}', '\u{feff}', 'a'] {
            assert!(!java_whitespace(c));
        }
    }

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
