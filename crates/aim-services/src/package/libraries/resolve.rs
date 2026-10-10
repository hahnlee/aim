//! Dependency selection and class-path order from android-16.0.0_r1
//! `SharedLibrariesImpl.collectSharedLibraryInfos` / `addSharedLibraryLPr`,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::*;
use crate::package::pkg::AndroidPackage;
use crate::package::settings::Signatures;
use sha2::{Digest, Sha256};

/// Native enforcement comes from PlatformCompat; SDK behavior from the
/// pinned image's compiled policy (`Policy::pinned`).
pub struct Policy {
    pub enforce_native_dependencies: bool,
    pub sdk_library_independence: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    MissingLibrary(String),
    DifferentSigners(String),
    BadCertificateDigest(String),
    Incomplete(&'static str),
}

/// Direct dependencies, in the original collection order. Provider
/// snapshots must already contain their computed transitive file paths.
#[derive(Debug)]
pub struct Selection {
    pub libraries: Vec<SharedLibrary>,
}

impl Registry {
    pub fn collect(
        &self,
        pkg: &AndroidPackage,
        available: &BTreeMap<String, PackageState>,
        policy: Policy,
    ) -> Result<Selection, ResolveError> {
        self.collect_packages(pkg, available, policy)
    }

    pub(super) fn collect_packages<P: LibraryPackage>(
        &self,
        pkg: &AndroidPackage,
        available: &BTreeMap<String, P>,
        policy: Policy,
    ) -> Result<Selection, ResolveError> {
        let mut libraries = Vec::new();
        let mut add = |names: &[String],
                       versions: Option<&[i64]>,
                       digests: Option<&[Option<Vec<Option<String>>>]>,
                       required: bool,
                       optional: Option<&[bool]>| {
            for (i, name) in names.iter().enumerate() {
                let version = match versions {
                    None => VERSION_UNDEFINED,
                    Some(v) => *v
                        .get(i)
                        .ok_or(ResolveError::Incomplete("library versions"))?,
                };
                let is_optional = optional
                    .map(|v| {
                        v.get(i)
                            .copied()
                            .ok_or(ResolveError::Incomplete("library optional flags"))
                    })
                    .transpose()?;
                let must_exist = required || is_optional == Some(false);
                let Some(library) = self.get(name, version) else {
                    if must_exist {
                        return Err(ResolveError::MissingLibrary(name.clone()));
                    }
                    continue;
                };
                if versions.is_some() {
                    let expected = digests
                        .and_then(|v| v.get(i))
                        .and_then(Option::as_ref)
                        .ok_or(ResolveError::Incomplete("library certificate digests"))?;
                    let provider = library
                        .package_name
                        .as_ref()
                        .and_then(|n| available.get(n))
                        .ok_or_else(|| ResolveError::MissingLibrary(name.clone()))?;
                    let signatures = provider
                        .signatures()
                        .ok_or(ResolveError::Incomplete("verified library signing details"))?;
                    check_signers(name, expected, signatures, pkg.target_sdk_version)?;
                }
                libraries.push(library.clone());
            }
            Ok(())
        };
        add(&pkg.uses_libraries, None, None, true, None)?;
        if !pkg.uses_static_libraries.is_empty() && pkg.uses_static_libraries_versions.is_none() {
            return Err(ResolveError::Incomplete("static library versions"));
        }
        add(
            &pkg.uses_static_libraries,
            pkg.uses_static_libraries_versions.as_deref(),
            pkg.uses_static_libraries_cert_digests.as_deref(),
            true,
            None,
        )?;
        add(&pkg.uses_optional_libraries, None, None, false, None)?;
        if policy.enforce_native_dependencies {
            add(&pkg.uses_native_libraries, None, None, true, None)?;
            add(&pkg.uses_optional_native_libraries, None, None, false, None)?;
        }
        if !pkg.uses_sdk_libraries.is_empty()
            && (pkg.uses_sdk_libraries_versions_major.is_none()
                || pkg.uses_sdk_libraries_optional.is_none())
        {
            return Err(ResolveError::Incomplete(
                "SDK library versions or optional flags",
            ));
        }
        add(
            &pkg.uses_sdk_libraries,
            pkg.uses_sdk_libraries_versions_major.as_deref(),
            pkg.uses_sdk_libraries_cert_digests.as_deref(),
            !policy.sdk_library_independence,
            pkg.uses_sdk_libraries_optional.as_deref(),
        )?;
        Ok(Selection { libraries })
    }
}

impl Selection {
    /// `LinkedHashSet` ordering: own code paths precede a provider's
    /// resolved dependencies, with duplicates removed at first occurrence.
    pub fn files(
        &self,
        available: &BTreeMap<String, PackageState>,
    ) -> Result<Vec<Option<String>>, ResolveError> {
        self.files_packages(available)
    }

    pub(super) fn files_packages<P: LibraryPackage>(
        &self,
        available: &BTreeMap<String, P>,
    ) -> Result<Vec<Option<String>>, ResolveError> {
        let mut files = Vec::new();
        let mut add = |path: Option<&str>| {
            let path = path.map(str::to_owned);
            if !files.contains(&path) {
                files.push(path);
            }
        };
        for library in &self.libraries {
            if let Some(path) = &library.path {
                add(Some(path));
            } else {
                let provider = library
                    .package_name
                    .as_ref()
                    .and_then(|n| available.get(n))
                    .ok_or(ResolveError::Incomplete("library provider snapshot"))?;
                let pkg = provider
                    .code()
                    .ok_or(ResolveError::Incomplete("library provider APK"))?;
                add(Some(
                    pkg.base_apk_path
                        .as_deref()
                        .ok_or(ResolveError::Incomplete("library base APK path"))?,
                ));
                if let Some(paths) = &pkg.split_code_paths {
                    for path in paths {
                        add(Some(path.as_deref().ok_or(ResolveError::Incomplete(
                            "library split APK path",
                        ))?));
                    }
                }
                for path in provider.files() {
                    add(path.as_deref());
                }
            }
        }
        Ok(files)
    }
}

fn check_signers(
    name: &str,
    expected: &[Option<String>],
    signatures: &Signatures,
    target_sdk: i32,
) -> Result<(), ResolveError> {
    if expected.len() > 1 {
        let certs = if target_sdk >= 27 {
            signatures.signatures.as_slice()
        } else {
            signatures
                .signatures
                .get(..1)
                .ok_or(ResolveError::Incomplete("library signatures"))?
        };
        let mut actual: Vec<_> = certs.iter().map(|c| hex(&Sha256::digest(c))).collect();
        let mut expected = expected
            .iter()
            .map(|s| {
                s.as_deref()
                    .ok_or(ResolveError::Incomplete("null certificate digest"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        actual.sort();
        expected.sort();
        if actual.len() != expected.len()
            || actual
                .iter()
                .zip(expected)
                .any(|(a, e)| !a.eq_ignore_ascii_case(e))
        {
            return Err(ResolveError::DifferentSigners(name.into()));
        }
    } else {
        let value = expected
            .first()
            .and_then(Option::as_deref)
            .ok_or(ResolveError::Incomplete("empty certificate digests"))?;
        let digest =
            decode(value).ok_or_else(|| ResolveError::BadCertificateDigest(name.into()))?;
        let past_matches = signatures.past_signatures.as_ref().is_some_and(|past| {
            past.len() > 1
                && past[..past.len() - 1]
                    .iter()
                    .any(|(der, _)| Sha256::digest(der).as_slice() == digest)
        });
        let current_matches = matches!(signatures.signatures.as_slice(), [one] if Sha256::digest(one).as_slice() == digest);
        if !past_matches && !current_matches {
            return Err(ResolveError::DifferentSigners(name.into()));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| (b as char).to_digit(16).map(|n| n as u8);
            Some((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}
