//! An installed package's APKs, read from the guest's files: their
//! signers, verified as the original's `ApkSignatureVerifier` verifies an
//! install (`package::sign`), and the package the native parser makes of
//! them (`package::parse`), the parser's oracle on installs.

use std::path::PathBuf;

use android_image_extract::source::FileSource;

use crate::package::model::PackageState;
use crate::package::parse::{self, Platform};
use crate::package::pkg::AndroidPackage;
use crate::package::settings::Signatures;
use crate::package::sign::{self, Apk, Build};

/// Where the service host reads a guest path, if others may read it.
pub type Files = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// The guest's files and the platform the parser and the verifier
/// depend on.
pub struct Apks {
    pub files: Files,
    pub platform: Platform,
}

impl Apks {
    /// The signers of the parsed package's APK paths: its base
    /// APK's, which each split shares (`getSigningDetails`, verified in
    /// full).
    pub fn signatures(&self, pkg: &AndroidPackage) -> Result<Signatures, String> {
        let base = pkg
            .base_apk_path
            .as_ref()
            .ok_or("no parsed base APK path")?;
        let mut paths = vec![base.clone()];
        if let Some(splits) = &pkg.split_code_paths {
            for path in splits {
                paths.push(path.clone().ok_or("null parsed split APK path")?);
            }
        }
        let sources = paths
            .iter()
            .map(|path| {
                let host = (self.files)(path).ok_or_else(|| format!("{path}: not readable"))?;
                FileSource::open(&host).map_err(|e| format!("{path}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let apk = |i: usize| Apk {
            path: &paths[i],
            data: &sources[i],
            v4: None,
        };
        let splits: Vec<Apk> = (1..sources.len()).map(apk).collect();
        let details = sign::package_signing_details(
            &apk(0),
            &splits,
            pkg.static_shared_library_name.is_some(),
            pkg.target_sdk_version,
            false,
            &Build::of(&self.platform),
        )
        .map_err(|e| e.to_string())?;
        Ok(Signatures {
            scheme_version: details.scheme_version,
            signatures: details.signatures,
            public_keys: None,
            past_signatures: details.past_signing_certificates,
        })
    }

    /// The package the native parser makes of the APK at `ps`'s code path,
    /// read back as the original's parcel reads.
    pub fn parsed(&self, ps: &PackageState) -> Result<AndroidPackage, String> {
        let dir = (self.files)(&ps.path).ok_or_else(|| format!("{}: not readable", ps.path))?;
        let package = parse::parse(&dir, &ps.path, 0, &self.platform).map_err(|e| e.to_string())?;
        AndroidPackage::read_cache_entry(&package.to_cache_entry().bytes)
            .map_err(|s| format!("the parser's entry does not read: status {s}"))
    }
}
