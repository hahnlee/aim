//! InstallPackageHelper.addForInitLI loading completion after code admission
//! (android-16.0.0_r1, #1220). Incremental backing remains its owner's decision.
use super::{Error, SigningError, SigningScan};

impl SigningScan {
    /// Complete admitted ordinary code before the constructor settings write.
    /// An unresolved backing owner aborts before changing any progress.
    pub fn complete_boot_loading(
        &mut self,
        is_incremental: &dyn Fn(&str) -> Result<bool, String>,
    ) -> Result<(), SigningError> {
        let mut completed = Vec::new();
        for (index, setting) in self.settings.packages.iter().enumerate() {
            let Some(code) = self.loaded.get(&setting.name) else {
                continue;
            };
            let failure = |message| {
                SigningError::Fatal(Error {
                    package: setting.name.clone(),
                    path: setting.code_path.clone(),
                    phase: "loading-completion",
                    message,
                })
            };
            code.validate_setting(setting, false).map_err(failure)?;
            if !is_incremental(&setting.code_path).map_err(failure)? {
                completed.push(index);
            }
        }
        for index in completed {
            self.settings.packages[index].set_loading_progress(1.0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        owner::shared_users::ScanOrigin,
        pkg::AndroidPackage,
        scan::{Identity, LoadedPackage, Record},
        settings::{Package, Settings},
        sign::SigningDetails,
    };
    use std::sync::Arc;

    fn admitted(entries: &[(&str, f32)]) -> SigningScan {
        let packages: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, (name, progress))| Package {
                name: (*name).into(),
                code_path: format!("/system/app/{name}/base.apk"),
                app_id: 10100 + index as i32,
                flags: crate::package::settings::FLAG_SYSTEM,
                loading_progress: *progress,
                ..Default::default()
            })
            .collect();
        let mut owner = SigningScan::new(
            &Default::default(),
            &Settings {
                packages: packages.clone(),
                ..Default::default()
            },
            36,
        )
        .unwrap();
        for package in packages {
            let signing = SigningDetails {
                unknown: false,
                signatures: vec![vec![3]],
                scheme_version: 3,
                public_keys: Some(Vec::new()),
                current_flags: Vec::new(),
                past_signing_certificates: None,
            };
            let parsed = AndroidPackage {
                package_name: package.name.clone(),
                path: Some(package.code_path.clone()),
                uid: package.app_id,
                signing_details: signing.package_details().unwrap(),
                ..Default::default()
            };
            let record = Record {
                settings: package.clone(),
                parsed: parsed.clone(),
                signing: signing.clone(),
                identity: Identity {
                    manifest_name: package.name.clone(),
                    internal_name: package.name.clone(),
                    real_name: None,
                },
                origin: ScanOrigin::SystemDirectory,
            };
            owner.apply(&record).unwrap();
            owner.loaded.insert(
                package.name,
                Arc::new(LoadedPackage::new(parsed, signing).unwrap()),
            );
        }
        owner
    }

    #[test]
    fn accepted_ordinary_code_completes_without_changing_incremental_or_saved_owners() {
        let mut owner = admitted(&[("ordinary", 0.0), ("incremental", 0.375)]);
        let ordinary = owner.settings.packages[0].clone();
        let partial = owner.settings.packages[1].clone();
        owner.settings.packages.push(Package {
            name: "saved-unadmitted".into(),
            code_path: "/data/app/absent".into(),
            loading_progress: 0.25,
            ..Default::default()
        });
        owner
            .settings
            .disabled_system_packages
            .push(ordinary.clone());
        let previous = owner.clone();
        owner
            .complete_boot_loading(&|path| {
                if path == ordinary.code_path {
                    Ok(false)
                } else if path == partial.code_path {
                    Ok(true)
                } else {
                    panic!("unadmitted settings must not query code backing")
                }
            })
            .unwrap();
        assert_eq!(owner.settings.packages[0].loading_progress, 1.0);
        assert!(!owner.settings.packages[0].is_loading());
        assert_eq!(owner.settings.packages[1].loading_progress, 0.375);
        assert!(owner.settings.packages[1].is_loading());
        assert_eq!(owner.settings.packages[2].loading_progress, 0.25);
        assert_eq!(
            owner.settings.disabled_system_packages[0].loading_progress,
            0.0
        );
        assert_eq!(previous.settings.packages[0].loading_progress, 0.0);
    }

    #[test]
    fn unavailable_incremental_owner_does_not_complete_prior_candidates() {
        let mut owner = admitted(&[("first", 0.0), ("second", 0.5)]);
        let first = owner.settings.packages[0].clone();
        let before = owner.clone();
        assert!(matches!(
            owner.complete_boot_loading(&|path| {
                if path == first.code_path {
                    Ok(false)
                } else {
                    Err("backing receipt unavailable".into())
                }
            }),
            Err(SigningError::Fatal(Error {
                phase: "loading-completion",
                ..
            }))
        ));
        assert_eq!(owner, before);
    }

    #[test]
    fn mismatched_admitted_code_fails_without_completing_loading() {
        let mut owner = admitted(&[("changed", 0.5)]);
        owner.settings.packages[0].code_path = "/system/app/replaced/base.apk".into();
        let before = owner.clone();
        assert!(
            owner
                .complete_boot_loading(&|_| panic!("identity mismatch precedes backing lookup"))
                .is_err()
        );
        assert_eq!(owner, before);
    }
}
