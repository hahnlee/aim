//! APK signature verification: the signing certificates of an APK, the
//! signing lineage's past certificates with their capabilities, and the
//! scheme they were verified with, the `SigningDetails` the original's
//! `ApkSignatureVerifier` makes (docs/m4-packagemanager.md, D4). The
//! schemes are tried newest first: v4 (with the v2 or v3 signer it
//! extends), v3.1 and v3 (one signer for this SDK level, and a
//! proof-of-rotation lineage), v2, then the JAR scheme; a scheme that
//! is present but does not verify fails the APK, and one older than the
//! package's minimum is not tried.
//!
//! The digests and signature algorithms are RustCrypto's; certificates
//! keep their encoding verbatim, as `VerbatimX509Certificate` does.
//!
//! Ported from the Android Open Source Project (`android-16.0.0_r1`,
//! `android.util.apk.ApkSignatureVerifier`, `ApkSignatureSchemeV2Verifier`,
//! `ApkSignatureSchemeV3Verifier`, `ApkSignatureSchemeV4Verifier`,
//! `ApkSigningBlockUtils`, `VerityBuilder`, `ZipUtils`,
//! `android.os.incremental.V4Signature`,
//! `com.android.internal.security.VerityUtils`, `android.util.jar`'s
//! `StrictJarFile`, `StrictJarVerifier`, `StrictJarManifest` and
//! `StrictJarManifestReader`, `ParsingPackageUtils.getSigningDetails`, and
//! libcore's `sun.security.pkcs.PKCS7`, `SignerInfo` and
//! `PKCS9Attributes`), Copyright (C) The Android Open Source Project,
//! Licensed under the Apache License, Version 2.0.

mod asn1;
mod block;
mod crypto;
mod history;
mod merge;
pub use history::{History, INSTALLED_DATA, JoinType, ROLLBACK, SHARED_USER_ID};
pub use merge::MergeRule;
mod jar;
mod serialize;
pub(crate) use serialize::canonical_public_keys;
pub use serialize::decode_public_key as deserialize_public_key;
pub use serialize::public_keys as serialize_public_keys;

pub(crate) fn saved_certificate_keys(
    certificates: &[Vec<u8>],
) -> Result<Option<Vec<super::pkg::Serialized>>, String> {
    let mut keys = Vec::new();
    for bytes in certificates {
        let Ok(certificate) = asn1::Certificate::parse(bytes) else {
            return Ok(None);
        };
        keys.push(certificate.public_key.to_vec());
    }
    // A missing native serialization implementation is not an invalid guest
    // certificate: report it rather than publishing SigningDetails.UNKNOWN.
    serialize_public_keys(&keys).map(Some)
}
#[cfg(test)]
mod tests;
mod v2;
mod v3;
mod v4;

use std::fmt;

use android_image_extract::source::ReadAt;

pub use v4::V4Source;

/// `SigningDetails.SignatureSchemeVersion`.
pub const UNKNOWN: i32 = 0;
pub const JAR: i32 = 1;
pub const SIGNING_BLOCK_V2: i32 = 2;
pub const SIGNING_BLOCK_V3: i32 = 3;
pub const SIGNING_BLOCK_V4: i32 = 4;

/// `PackageManager.INSTALL_PARSE_FAILED_*`.
pub const INSTALL_PARSE_FAILED_BAD_MANIFEST: i32 = -101;
pub const INSTALL_PARSE_FAILED_UNEXPECTED_EXCEPTION: i32 = -102;
pub const INSTALL_PARSE_FAILED_NO_CERTIFICATES: i32 = -103;
pub const INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES: i32 = -104;

/// The path the original compares a base APK's with: framework-res's
/// splits are not verified.
const FRAMEWORK_RES: &str = "/system/framework/framework-res.apk";

/// What the verifier depends on of the platform (`Build.VERSION`, and the
/// `android.content.pm.always_load_past_certs_v4` flag).
pub struct Build {
    pub sdk_int: i32,
    /// `Build.VERSION.CODENAME` is `REL`.
    pub release: bool,
    pub always_load_past_certs_v4: bool,
}

impl Build {
    /// What the verifier needs of `platform`.
    pub fn of(platform: &super::parse::Platform) -> Build {
        Build {
            sdk_int: platform.sdk,
            release: platform.codenames.is_empty(),
            always_load_past_certs_v4: platform
                .flags
                .get("android.content.pm.always_load_past_certs_v4")
                .copied()
                .unwrap_or(false),
        }
    }
}

/// A signing lineage: certificates oldest first, ending with the signer,
/// with each one's capabilities (`SigningDetails.CertCapabilities`).
pub type Lineage = Vec<(Vec<u8>, i32)>;

/// `SigningDetails`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningDetails {
    /// The signers' certificates, as encoded.
    pub signatures: Vec<Vec<u8>>,
    pub scheme_version: i32,
    /// The signers' public keys (`SubjectPublicKeyInfo`), each once.
    pub public_keys: Vec<Vec<u8>>,
    pub past_signing_certificates: Option<Lineage>,
}

impl SigningDetails {
    /// Original PackageImpl's SigningDetails parcel representation. Keep the
    /// collected package lineage separate from reconciled settings signatures.
    pub fn parcel_details(&self) -> Result<super::pkg::SigningDetails, String> {
        Ok(super::pkg::SigningDetails {
            signatures: Some(self.signatures.clone()),
            scheme_version: self.scheme_version,
            public_keys: Some(
                serialize_public_keys(&self.public_keys)?
                    .into_iter()
                    .map(Some)
                    .collect(),
            ),
            past_signing_certificates: self.past_signing_certificates.as_ref().map(|past| {
                past.iter()
                    .map(|(certificate, _)| certificate.clone())
                    .collect()
            }),
        })
    }

    fn new(
        signatures: Vec<Vec<u8>>,
        scheme_version: i32,
        past_signing_certificates: Option<Lineage>,
    ) -> Result<SigningDetails, String> {
        let mut public_keys: Vec<Vec<u8>> = Vec::new();
        for s in &signatures {
            let key = asn1::Certificate::parse(s)?.public_key.to_vec();
            if !public_keys.contains(&key) {
                public_keys.push(key);
            }
        }
        Ok(SigningDetails {
            signatures,
            scheme_version,
            public_keys,
            past_signing_certificates,
        })
    }

    /// `Signature.areExactMatch`: the same signers, in any order.
    fn signatures_match(&self, other: &SigningDetails) -> bool {
        self.signatures.len() == other.signatures.len()
            && self.signatures.iter().all(|s| other.signatures.contains(s))
            && other.signatures.iter().all(|s| self.signatures.contains(s))
    }
}

/// An APK to verify: its path (for messages, and to recognize
/// framework-res), its contents, and its v4 signature if it has one.
pub struct Apk<'a> {
    pub path: &'a str,
    pub data: &'a dyn ReadAt,
    pub v4: Option<V4Source<'a>>,
}

/// A failed verification: the `INSTALL_PARSE_FAILED_*` code the original
/// fails with, and why.
#[derive(Debug)]
pub struct Error {
    pub code: i32,
    pub message: String,
}

impl Error {
    fn new(code: i32, message: String) -> Error {
        Error { code, message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// A scheme's outcome other than a verified signer: the APK has no
/// signature of the scheme (`SignatureNotFoundException`), or one that does
/// not verify.
#[derive(Debug)]
enum Fail {
    NotFound(String),
    Invalid(String),
}

/// `ApkSignatureVerifier.getMinimumSignatureSchemeVersionForTargetSdk`.
pub fn minimum_signature_scheme(target_sdk: i32) -> i32 {
    if target_sdk >= 30 {
        SIGNING_BLOCK_V2
    } else {
        JAR
    }
}

/// `ParsingPackageUtils.getSigningDetails`: the signers of a package's
/// base APK, which each split must share (framework-res's splits are not
/// checked). A static shared library needs v2 or newer. `skip_verify`
/// collects the certificates without checking the contents, as the scan
/// does for the system partitions' trusted APKs.
pub fn package_signing_details(
    base: &Apk,
    splits: &[Apk],
    static_shared_library: bool,
    target_sdk: i32,
    skip_verify: bool,
    build: &Build,
) -> Result<SigningDetails, Error> {
    let min_scheme = if static_shared_library {
        SIGNING_BLOCK_V2
    } else {
        minimum_signature_scheme(target_sdk)
    };
    let details = verify(base, min_scheme, !skip_verify, build)?;
    if base.path != FRAMEWORK_RES {
        for split in splits {
            if !verify(split, min_scheme, !skip_verify, build)?.signatures_match(&details) {
                return Err(Error::new(
                    INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES,
                    format!("{} has mismatched certificates", split.path),
                ));
            }
        }
    }
    Ok(details)
}

/// `ApkSignatureVerifier.verify` (`full`) or
/// `unsafeGetCertsWithoutVerification`: the APK's signers by the newest
/// scheme it has, and no older one than `min_scheme`.
pub fn verify(
    apk: &Apk,
    min_scheme: i32,
    full: bool,
    build: &Build,
) -> Result<SigningDetails, Error> {
    let no_certs = |m: String| Error::new(INSTALL_PARSE_FAILED_NO_CERTIFICATES, m);
    let older = |scheme| {
        no_certs(format!(
            "no signature found in package of version {scheme} or newer for package {}",
            apk.path
        ))
    };
    let failed = |scheme, e: String| {
        no_certs(format!(
            "failed to collect certificates from {} using APK Signature Scheme v{scheme}: {e}",
            apk.path
        ))
    };
    let missing = |scheme, why| {
        no_certs(format!(
            "no APK Signature Scheme v{scheme} signature in package {}: {why}",
            apk.path
        ))
    };
    if min_scheme > SIGNING_BLOCK_V4 {
        return Err(older(min_scheme));
    }
    match verify_v4(apk, full, build) {
        Ok(d) => return d.map_err(|e| failed(4, e)),
        Err(Fail::Invalid(e)) => return Err(failed(4, e)),
        Err(Fail::NotFound(why)) if min_scheme >= SIGNING_BLOCK_V4 => {
            return Err(missing(4, why));
        }
        Err(Fail::NotFound(_)) => {}
    }
    if min_scheme > SIGNING_BLOCK_V3 {
        return Err(older(min_scheme));
    }
    match v3::verify(apk.data, full, build) {
        Ok(v) => {
            return SigningDetails::new(vec![v.certs[0].clone()], SIGNING_BLOCK_V3, v.lineage)
                .map_err(|e| failed(3, e));
        }
        Err(Fail::Invalid(e)) => return Err(failed(3, e)),
        Err(Fail::NotFound(why)) if min_scheme >= SIGNING_BLOCK_V3 => {
            return Err(missing(3, why));
        }
        Err(Fail::NotFound(_)) => {}
    }
    if min_scheme > SIGNING_BLOCK_V2 {
        return Err(older(min_scheme));
    }
    match v2::verify(apk.data, full) {
        Ok(v) => {
            let signers = v.certs.into_iter().map(|c| c[0].clone()).collect();
            return SigningDetails::new(signers, SIGNING_BLOCK_V2, None).map_err(|e| failed(2, e));
        }
        Err(Fail::Invalid(e)) => return Err(failed(2, e)),
        Err(Fail::NotFound(why)) if min_scheme >= SIGNING_BLOCK_V2 => {
            return Err(missing(2, why));
        }
        Err(Fail::NotFound(_)) => {}
    }
    if min_scheme > JAR {
        return Err(older(min_scheme));
    }
    let signers = jar::verify(apk.data, apk.path, full)?;
    SigningDetails::new(signers, JAR, None).map_err(|e| failed(1, e))
}

/// `verifyV4Signature`: the v4 signer, with the lineage of the v3 signer
/// it extends; when `full`, its certificate and digest must be the v2 or
/// v3 signer's.
fn verify_v4(apk: &Apk, full: bool, build: &Build) -> Result<Result<SigningDetails, String>, Fail> {
    let size = apk.data.size();
    let (hashing, infos) = v4::extract(apk.v4.as_ref(), size)?;
    let mut lineage = None;
    let mut nonstreaming = None;
    let mut v3_block_id = v4::APK_SIGNATURE_SCHEME_DEFAULT;
    if build.always_load_past_certs_v4 || full || infos.has_blocks() {
        match v3::verify(apk.data, false, build) {
            Ok(v) => {
                nonstreaming = Some((v.content_digests, vec![v.certs[0].clone()]));
                lineage = v.lineage;
                v3_block_id = v.block_id;
            }
            Err(Fail::NotFound(_)) => match v2::verify(apk.data, false) {
                Ok(v) => {
                    let signers = v.certs.into_iter().map(|c| c[0].clone()).collect();
                    nonstreaming = Some((v.content_digests, signers));
                }
                Err(Fail::NotFound(_)) => {
                    return Ok(Err(
                        "V4 verification failed to collect V2/V3 certificates".into()
                    ));
                }
                Err(Fail::Invalid(e)) => return Ok(Err(e)),
            },
            Err(Fail::Invalid(e)) => return Ok(Err(e)),
        }
    }
    let signer = match v4::verify(size, &hashing, &infos, v3_block_id) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };
    if full && let Some((digests, signers)) = &nonstreaming {
        if signers.len() != 1 {
            return Ok(Err(format!(
                "invalid number of certificates: {}",
                signers.len()
            )));
        }
        if signers[0] != signer.cert {
            return Ok(Err("V4 signature certificate does not match V2/V3".into()));
        }
        let n = signer.apk_digest.len();
        if !digests
            .values()
            .any(|d| d.len() >= n && d[..n] == signer.apk_digest[..])
        {
            return Ok(Err("APK digest in V4 signature does not match V2/V3".into()));
        }
    }
    Ok(SigningDetails::new(
        vec![signer.cert],
        SIGNING_BLOCK_V4,
        lineage,
    ))
}
