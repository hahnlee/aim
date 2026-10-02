//! APK inputs for the native boot scan (#707). Reads persisted active and
//! disabled-system code without the original parser cache or package feed.
//! Active records pass normal saved package/shared-UID signature gates.
//! Reconciliation, new/removed image packages, APEX state and publication
//! are separate phases; these records are not a query snapshot.

use super::{
    State, owner::shared_users::ScanOrigin, parse, pkg::AndroidPackage, settings, sign, write::Apks,
};
use std::collections::BTreeMap;

mod authorize;
mod enrich;
pub use enrich::{ScanClock, ScanTime};
mod identity;
mod image;
mod policy;
pub use policy::{ScanPolicy, application_flags};
mod compatibility;
pub use compatibility::LibraryCompatibility;
mod bridge;
pub use bridge::PolicyBridgeError;
mod abi;
pub use abi::{
    AbiPolicy, AbiScanContext, AbiScanMode, AbiSelectionError, BundledAbis, NativeLibraryAbiCopy,
    NativeLibraryCopy, NativeLibraryDestination, NativeLibraryEntry, NativeLibraryEnvironment,
    NativeLibraryError, NativeLibraryInstallError, NativeLibraryInstallPolicy,
    NativeLibraryPackageCopy, NativeLibraryPaths, NativeLibraryScan, PackageAbis,
    PageSizeCompatPolicy, SharedUserAbi, SharedUserAbiMismatch, SupportedAbi, SupportedAbis,
    ZipNativeLibraries,
};
mod completion;
mod setting;
mod signing;
pub use completion::{CompletedScanMetadata, ScanMetadataCompletion};
mod uids;
mod validate;
pub use identity::Identity;
pub use image::{Apex, Code, Image, Kind, Location, Partition, Rejected};
pub use setting::{NewSetting, SettingMetadata, SettingUpdate, User, UserPolicy};
pub use signing::{
    NewPackageOutcome, SharedUidMigration, SigningError, SigningOutcome, SigningScan,
};
pub use uids::{Uid, UidScan};

#[derive(Debug)]
pub struct Record {
    pub settings: settings::Package,
    pub parsed: AndroidPackage,
    pub signing: sign::SigningDetails,
    pub identity: Identity,
    pub origin: ScanOrigin,
}

#[derive(Debug, Default)]
pub struct Inputs {
    pub active: BTreeMap<String, Record>,
    pub disabled: BTreeMap<String, Record>,
    /// APEX packages are verified and supplied by apexd, not the APK
    /// signature verifier. They still require reconciliation (#707).
    pub apex: Vec<settings::Package>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub package: String,
    pub path: String,
    pub phase: &'static str,
    pub message: String,
}

impl Inputs {
    pub fn load(state: &State, apks: &Apks) -> Result<Self, Error> {
        let inputs = Self::load_verified_code(state, apks)?;
        for record in inputs.active.values() {
            authorize::saved(&record.settings, &record.signing, &state.settings).map_err(
                |message| Error {
                    package: record.settings.name.clone(),
                    path: record.settings.code_path.clone(),
                    phase: "authorization",
                    message,
                },
            )?;
        }
        Ok(inputs)
    }

    /// Integrity-verified code for ordered owner reconciliation. These
    /// records have selected names but no saved signer/UID authorization;
    /// callers must pass active records through SigningScan before commit.
    pub fn load_verified_code(state: &State, apks: &Apks) -> Result<Self, Error> {
        let mut inputs = Self::default();
        for (packages, disabled) in [
            (&state.settings.packages, false),
            (&state.settings.disabled_system_packages, true),
        ] {
            for ps in packages {
                if ps.code_path.ends_with(".apex") || ps.code_path.ends_with(".capex") {
                    inputs.apex.push(ps.clone());
                    continue;
                }
                let fail = |phase, message| Error {
                    package: ps.name.clone(),
                    path: ps.code_path.clone(),
                    phase,
                    message,
                };
                let flags = physical_parse_flags(&ps.code_path).map_err(|e| fail("location", e))?;
                let mut parsed = apks
                    .parsed_path(&ps.code_path, flags)
                    .map_err(|e| fail("parse", e))?;
                let identity = Identity::select(
                    &parsed,
                    &state.settings,
                    ps.flags & settings::FLAG_SYSTEM != 0,
                );
                if identity.internal_name != ps.name {
                    return Err(fail(
                        "identity",
                        format!(
                            "manifest {} selects {}, settings claim {}",
                            identity.manifest_name, identity.internal_name, ps.name
                        ),
                    ));
                }
                let signing = apks
                    .signing_details(&parsed)
                    .map_err(|e| fail("signatures", e))?;
                identity.apply(&mut parsed);
                let records = if disabled {
                    &mut inputs.disabled
                } else {
                    &mut inputs.active
                };
                if records
                    .insert(
                        ps.name.clone(),
                        Record {
                            settings: ps.clone(),
                            parsed,
                            signing,
                            identity,
                            origin: if flags & parse::PARSE_IS_SYSTEM_DIR != 0 {
                                ScanOrigin::SystemDirectory
                            } else {
                                ScanOrigin::Data
                            },
                        },
                    )
                    .is_some()
                {
                    return Err(fail("settings", "duplicate package record".into()));
                }
            }
        }
        Ok(inputs)
    }
}

// Parse flags describe the physical scan location. An updated system
// package under /data does not get PARSE_IS_SYSTEM_DIR from its flags.
fn physical_parse_flags(path: &str) -> Result<i32, String> {
    if path.starts_with("/data/app/") {
        return Ok(0);
    }
    if path
        .strip_prefix("/mnt/expand/")
        .and_then(|p| p.split_once('/'))
        .is_some_and(|(volume, relative)| !volume.is_empty() && relative.starts_with("app/"))
    {
        return Ok(0);
    }
    if path.starts_with("/apex/") {
        return Ok(parse::PARSE_IS_SYSTEM_DIR | parse::PARSE_APK_IN_APEX);
    }
    if ["system", "system_ext", "product", "vendor", "odm", "oem"]
        .iter()
        .any(|partition| path.starts_with(&format!("/{partition}/")))
    {
        return Ok(parse::PARSE_IS_SYSTEM_DIR);
    }
    Err("scan location has no APK parse policy (#707)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_flags_follow_location_instead_of_saved_system_status() {
        assert_eq!(
            physical_parse_flags("/data/app/updated-system/base.apk"),
            Ok(0)
        );
        assert_eq!(
            physical_parse_flags("/product/priv-app/SystemApp"),
            Ok(parse::PARSE_IS_SYSTEM_DIR)
        );
        assert_eq!(
            physical_parse_flags("/apex/module/app/App"),
            Ok(parse::PARSE_IS_SYSTEM_DIR | parse::PARSE_APK_IN_APEX)
        );
        assert_eq!(physical_parse_flags("/mnt/expand/volume/app/pkg"), Ok(0));
        assert!(physical_parse_flags("/mnt/expand/volume/not-app/pkg").is_err());
    }
}
