//! Initial scan APEX registration rules from android-16.0.0_r1
//! InstallPackageHelper. Copyright (C) The Android Open Source Project,
//! Apache License 2.0.
use super::{Code, Location, NewPackageOutcome, SigningError, SigningScan};
use crate::package::{owner::transient::State, settings::Package};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ApexScan {
    module_name: Option<String>,
    updated: Option<bool>,
}

impl ApexScan {
    fn for_init(
        location: &Location,
        disabled: Option<&Package>,
        existing: Option<&Package>,
    ) -> Self {
        Self {
            module_name: match &location.apex {
                Some(apex) => apex.module_name.clone(),
                None => match disabled {
                    Some(setting) => setting.transient.apex_module_name.clone(),
                    None => existing.and_then(|setting| setting.transient.apex_module_name.clone()),
                },
            },
            updated: location.apex.as_ref().map(|apex| !apex.factory),
        }
    }

    fn apply(self, state: &mut State) {
        // commitPackageSettings changes the update bit only for APK_IN_APEX.
        if let Some(updated) = self.updated {
            state.apk_in_updated_apex = updated;
        }
        state.apex_module_name = self.module_name;
    }
}

impl SigningScan {
    /// addForInitLI refreshes a disabled factory before version/signature
    /// selection can skip it. This effect survives later scan rejection.
    pub(super) fn refresh_init_apex(&mut self, code: &Code) {
        // With ActiveApexInfo, addForInitLI uses the raw package name here,
        // before the later settings/adoption identity selection.
        let name = &code.parsed.package_name;
        if let Some(apex) = &code.location.apex {
            if let Some(disabled) = self
                .settings
                .disabled_system_packages
                .iter_mut()
                .find(|p| &p.name == name)
            {
                disabled
                    .transient
                    .apex_module_name
                    .clone_from(&apex.module_name);
            }
        }
    }

    pub(super) fn finish_init_apex(
        &mut self,
        mut candidate: NewPackageOutcome,
        location: &Location,
    ) -> Result<NewPackageOutcome, SigningError> {
        let at = self.accepted_slot(&candidate.record, "apex-registration")?;
        let name = &candidate.record.settings.name;
        let apex = ApexScan::for_init(
            location,
            self.settings
                .disabled_system_packages
                .iter()
                .find(|p| &p.name == name),
            self.settings.packages.iter().find(|p| &p.name == name),
        );
        apex.apply(&mut candidate.record.settings.transient);
        self.settings.packages[at] = candidate.record.settings.clone();
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::scan::{Apex, Kind, Partition};

    #[test]
    fn disabled_refresh_precedes_identity_selection_and_preserves_other_state() {
        let package = |name: &str, id| Package {
            name: name.into(),
            app_id: id,
            transient: State {
                hidden_until_installed: true,
                updated_system_app: true,
                apk_in_updated_apex: true,
                apex_module_name: Some("old".into()),
            },
            ..Default::default()
        };
        let packages = vec![package("incoming", 10000), package("renamed", 10001)];
        let settings = crate::package::settings::Settings {
            disabled_system_packages: packages.clone(),
            packages,
            renamed_packages: vec![("incoming".into(), "renamed".into())],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        let mut code = Code {
            location: Location {
                path: "/apex/mount/app/p".into(),
                partition: Partition::Product,
                kind: Kind::App,
                apex: Some(Apex {
                    module_name: Some("raw.module".into()),
                    mount_path: "/apex/mount".into(),
                    partition: Partition::Product,
                    factory: true,
                    active_changed: false,
                }),
            },
            parsed: crate::package::pkg::AndroidPackage {
                package_name: "incoming".into(),
                static_shared_library_name: Some("library".into()),
                static_shared_lib_version: 7,
                original_packages: Some(vec![Some("renamed".into())]),
                ..Default::default()
            },
            signing: crate::package::sign::SigningDetails {
                unknown: false,
                current_flags: Vec::new(),
                signatures: Vec::new(),
                scheme_version: 0,
                public_keys: Some(Vec::new()),
                past_signing_certificates: None,
            },
        };
        let active = owner.settings.packages.clone();
        let identities = owner.identities.clone();
        for module in [Some("raw.module".into()), None] {
            code.location.apex.as_mut().unwrap().module_name = module.clone();
            owner.refresh_init_apex(&code);
            let refreshed = &owner.settings.disabled_system_packages[0].transient;
            assert_eq!(refreshed.apex_module_name, module);
            assert!(
                refreshed.hidden_until_installed
                    && refreshed.updated_system_app
                    && refreshed.apk_in_updated_apex
            );
            assert_eq!(
                owner.settings.disabled_system_packages[1],
                settings.disabled_system_packages[1]
            );
            assert_eq!(owner.settings.packages, active);
            assert_eq!(owner.identities, identities);
        }
    }

    #[test]
    fn init_module_priority_preserves_nullable_owner_and_non_apex_update_state() {
        let setting = |name: Option<&str>| Package {
            transient: State {
                hidden_until_installed: true,
                updated_system_app: true,
                apk_in_updated_apex: true,
                apex_module_name: name.map(str::to_owned),
            },
            ..Default::default()
        };
        let disabled = setting(Some("factory"));
        let existing = setting(Some("existing"));
        let mut location = Location {
            path: "/data/app/p".into(),
            partition: Partition::Data,
            kind: Kind::App,
            apex: None,
        };
        for (disabled, existing, expected) in [
            (Some(&disabled), Some(&existing), Some("factory")),
            (Some(&setting(None)), Some(&existing), None),
            (None, Some(&existing), Some("existing")),
            (None, None, None),
        ] {
            let mut state = setting(Some("previous")).transient;
            ApexScan::for_init(&location, disabled, existing).apply(&mut state);
            assert_eq!(state.apex_module_name.as_deref(), expected);
            assert!(
                state.apk_in_updated_apex
                    && state.hidden_until_installed
                    && state.updated_system_app
            );
        }
        for module in [None, Some("raw.owner")] {
            for factory in [false, true] {
                location.apex = Some(Apex {
                    module_name: module.map(str::to_owned),
                    mount_path: "/apex/different-mount".into(),
                    partition: Partition::Vendor,
                    factory,
                    active_changed: !factory,
                });
                let mut state = setting(Some("previous")).transient;
                ApexScan::for_init(&location, Some(&disabled), Some(&existing)).apply(&mut state);
                assert_eq!(state.apex_module_name.as_deref(), module);
                assert_eq!(state.apk_in_updated_apex, !factory);
                assert!(state.hidden_until_installed && state.updated_system_app);
            }
        }
    }
}
