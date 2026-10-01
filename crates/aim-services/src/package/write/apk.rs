//! An installed package's APKs, read from the guest's files: their
//! signers, verified as the original's `ApkSignatureVerifier` verifies an
//! install (`package::sign`).

use std::path::PathBuf;

use android_image_extract::source::FileSource;

use crate::package::model::PackageState;
use crate::package::pkg::AndroidPackage;
use crate::package::settings::Signatures;
use crate::package::sign::{self, Apk, Build};

/// Where the service host reads a guest path, if others may read it.
pub type Files = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// The guest's files and the platform the verifier depends on.
pub struct Apks {
    pub files: Files,
    pub build: Build,
}

impl Apks {
    /// The signers of the package installed at `ps`'s code path: its base
    /// APK's, which each split shares (`getSigningDetails`, verified in
    /// full).
    pub fn signatures(
        &self,
        ps: &PackageState,
        pkg: &AndroidPackage,
    ) -> Result<Signatures, String> {
        let dir = (self.files)(&ps.path).ok_or_else(|| format!("{}: not readable", ps.path))?;
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", ps.path))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".apk"))
            .collect();
        names.sort();
        let base = names
            .iter()
            .position(|n| n == "base.apk")
            .ok_or_else(|| format!("{}: no base.apk", ps.path))?;
        names.swap(0, base);
        let paths: Vec<String> = names.iter().map(|n| format!("{}/{n}", ps.path)).collect();
        let sources = names
            .iter()
            .map(|n| FileSource::open(&dir.join(n)).map_err(|e| format!("{}/{n}: {e}", ps.path)))
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
            &self.build,
        )
        .map_err(|e| e.to_string())?;
        Ok(Signatures {
            scheme_version: details.scheme_version,
            signatures: details.signatures,
            past_signatures: details.past_signing_certificates,
        })
    }
}
