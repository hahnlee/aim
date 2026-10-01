//! APK Signature Scheme v3 and v3.1 (`ApkSignatureSchemeV3Verifier`):
//! one signer for the device's SDK level, and its proof-of-rotation
//! lineage. A v3.1 block holds the rotated signer for newer platforms; the
//! v3.0 block then names the SDK level the rotation starts at, so the v3.1
//! block cannot be stripped.

use android_image_extract::source::ReadAt;

use super::asn1::Certificate;
use super::block::{self, Buf, ContentDigests, SignatureInfo};
use super::{Build, Fail, Lineage, crypto, v2};

/// The ID of this scheme in a JAR signature's `X-Android-APK-Signed`.
pub(super) const SF_ATTRIBUTE_ANDROID_APK_SIGNED_ID: i32 = 3;
pub(super) const APK_SIGNATURE_SCHEME_V3_BLOCK_ID: u32 = 0xf05368c0;
pub(super) const APK_SIGNATURE_SCHEME_V31_BLOCK_ID: u32 = 0x1b93ad61;
const PROOF_OF_ROTATION_ATTR_ID: u32 = 0x3ba06f8c;
const ROTATION_MIN_SDK_VERSION_ATTR_ID: u32 = 0x559f8b02;
const ROTATION_ON_DEV_RELEASE_ATTR_ID: u32 = 0xc2a6b3ba;

pub(super) struct Verified {
    pub certs: Vec<Vec<u8>>,
    /// The proof-of-rotation lineage.
    pub lineage: Option<Lineage>,
    pub content_digests: ContentDigests,
    /// The block the signer came from.
    pub block_id: u32,
}

enum Error {
    /// `PlatformNotSupportedException`: no signer is for this SDK level.
    NotSupported,
    Invalid(String),
}

impl From<String> for Error {
    fn from(e: String) -> Error {
        Error::Invalid(e)
    }
}

/// The verifier's state across the v3.1 and the v3.0 block.
struct Verifier<'a> {
    apk: &'a dyn ReadAt,
    full: bool,
    build: &'a Build,
    /// The lowest SDK level a v3.1 signer is for, which the v3.0 signer's
    /// rotation attribute must name.
    rotation_min_sdk: Option<i32>,
    signer_min_sdk: i32,
    block_id: u32,
}

pub(super) fn verify(apk: &dyn ReadAt, full: bool, build: &Build) -> Result<Verified, Fail> {
    let mut v = Verifier {
        apk,
        full,
        build,
        rotation_min_sdk: None,
        signer_min_sdk: 0,
        block_id: 0,
    };
    match block::find_signature(apk, APK_SIGNATURE_SCHEME_V31_BLOCK_ID) {
        Ok(info) => match v.verify(&info, APK_SIGNATURE_SCHEME_V31_BLOCK_ID) {
            Ok(r) => return Ok(r),
            Err(Error::NotSupported) => {}
            Err(Error::Invalid(e)) => return Err(Fail::Invalid(e)),
        },
        Err(Fail::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    let info = block::find_signature(apk, APK_SIGNATURE_SCHEME_V3_BLOCK_ID)?;
    v.verify(&info, APK_SIGNATURE_SCHEME_V3_BLOCK_ID)
        .map_err(|e| {
            Fail::Invalid(match e {
                Error::NotSupported => "no signer supports this platform".into(),
                Error::Invalid(e) => e,
            })
        })
}

impl Verifier<'_> {
    fn verify(&mut self, info: &SignatureInfo, block_id: u32) -> Result<Verified, Error> {
        self.block_id = block_id;
        let mut content_digests = ContentDigests::new();
        let mut count = 0;
        let mut result = None;
        let mut signers = Buf(&info.block)
            .slice()
            .map_err(|e| format!("failed to read list of signers: {e}"))?;
        while signers.has_remaining() {
            let signer = signers
                .slice()
                .map_err(Error::Invalid)
                .and_then(|mut s| self.verify_signer(&mut s, &mut content_digests));
            match signer {
                Ok(r) => {
                    result = Some(r);
                    count += 1;
                }
                Err(Error::NotSupported) => {}
                Err(Error::Invalid(e)) => {
                    return Err(format!("failed to parse/verify signer #{count} block: {e}").into());
                }
            }
        }
        let Some((certs, lineage)) = result else {
            if block_id == APK_SIGNATURE_SCHEME_V3_BLOCK_ID {
                return Err("no signers found".to_owned().into());
            }
            return Err(Error::NotSupported);
        };
        if count != 1 {
            return Err(
                "APK Signature Scheme V3 only supports one signer: multiple signers found"
                    .to_owned()
                    .into(),
            );
        }
        v2::finish(self.apk, info, &content_digests, self.full)?;
        Ok(Verified {
            certs,
            lineage,
            content_digests,
            block_id,
        })
    }

    fn verify_signer(
        &mut self,
        signer: &mut Buf,
        content_digests: &mut ContentDigests,
    ) -> Result<(Vec<Vec<u8>>, Option<Lineage>), Error> {
        let signed_data = signer.slice()?;
        let min_sdk = signer.i32()?;
        let max_sdk = signer.i32()?;
        let sdk = self.build.sdk_int;
        if sdk < min_sdk || sdk > max_sdk {
            if self.block_id == APK_SIGNATURE_SCHEME_V31_BLOCK_ID
                && self.rotation_min_sdk.is_none_or(|r| r > min_sdk)
            {
                self.rotation_min_sdk = Some(min_sdk);
            }
            return Err(Error::NotSupported);
        }
        let signed = v2::verify_signed_data(signed_data, signer, content_digests)?;
        let mut rest = signed.rest;
        if rest.i32()? != min_sdk {
            return Err(
                "minSdkVersion mismatch between signed and unsigned in v3 signer block"
                    .to_owned()
                    .into(),
            );
        }
        self.signer_min_sdk = min_sdk;
        if rest.i32()? != max_sdk {
            return Err(
                "maxSdkVersion mismatch between signed and unsigned in v3 signer block"
                    .to_owned()
                    .into(),
            );
        }
        let attrs = rest.slice()?;
        let lineage = self.verify_additional_attributes(attrs, &signed.certs[0])?;
        Ok((signed.certs, lineage))
    }

    fn verify_additional_attributes(
        &mut self,
        mut attrs: Buf,
        signing_cert: &[u8],
    ) -> Result<Option<Lineage>, Error> {
        let mut lineage = None;
        while attrs.has_remaining() {
            let mut attr = attrs.slice()?;
            if attr.remaining() < 4 {
                return Err(
                    "remaining buffer too short to contain additional attribute ID"
                        .to_owned()
                        .into(),
                );
            }
            match attr.u32()? {
                PROOF_OF_ROTATION_ATTR_ID => {
                    if lineage.is_some() {
                        return Err("encountered multiple Proof-of-rotation records"
                            .to_owned()
                            .into());
                    }
                    let por = proof_of_rotation(attr)?;
                    if por.last().is_some_and(|(c, _)| c != signing_cert) {
                        return Err("terminal certificate in Proof-of-rotation record does \
                             not match APK signing certificate"
                            .to_owned()
                            .into());
                    }
                    lineage = Some(por);
                }
                ROTATION_MIN_SDK_VERSION_ATTR_ID => {
                    if attr.remaining() < 4 {
                        return Err("remaining buffer too short to contain rotation \
                             minSdkVersion value"
                            .to_owned()
                            .into());
                    }
                    let expected = attr.i32()?;
                    match self.rotation_min_sdk {
                        None => {
                            return Err(format!(
                                "expected a v3.1 signing block targeting SDK version \
                                 {expected}, but a v3.1 block was not found"
                            )
                            .into());
                        }
                        Some(r) if r != expected => {
                            return Err(format!(
                                "expected a v3.1 signing block targeting SDK version \
                                 {expected}, but the v3.1 block was targeting {r}"
                            )
                            .into());
                        }
                        Some(_) => {}
                    }
                }
                ROTATION_ON_DEV_RELEASE_ATTR_ID
                    if self.block_id == APK_SIGNATURE_SCHEME_V31_BLOCK_ID
                        && self.build.sdk_int == self.signer_min_sdk
                        && self.build.release =>
                {
                    // A platform in development has the last release's SDK
                    // level; the signer is for that, not for the release.
                    self.rotation_min_sdk = Some(self.signer_min_sdk);
                    return Err(Error::NotSupported);
                }
                _ => {}
            }
        }
        Ok(lineage)
    }
}

/// `verifyProofOfRotationStruct`: a version, then levels each holding a
/// certificate and the signature algorithm the next level is signed with,
/// signed by the previous level's certificate; each certificate once.
fn proof_of_rotation(mut por: Buf) -> Result<Lineage, String> {
    let mut lineage = Lineage::new();
    let mut last: Option<(&[u8], u32)> = None;
    por.u32()?;
    while por.has_remaining() {
        let n = lineage.len() + 1;
        let fail = |e: String| format!("failed to parse Proof-of-rotation record: {e}");
        let mut level = por.slice().map_err(fail)?;
        let signed_data = level.slice().map_err(fail)?;
        let flags = level.i32().map_err(fail)?;
        let sig_alg = level.u32().map_err(fail)?;
        let signature = level.bytes().map_err(fail)?;
        if let Some((cert, alg)) = last {
            let key = Certificate::parse(cert)?.public_key;
            crypto::verify(block::jca_algorithm(alg)?, key, signed_data.0, signature)
                .map_err(|e| format!("unable to verify signature of certificate #{n}: {e}"))?;
        }
        let mut signed = signed_data;
        let encoded = signed.bytes().map_err(fail)?;
        let signed_alg = signed.u32().map_err(fail)?;
        if last.is_some_and(|(_, alg)| alg != signed_alg) {
            return Err(format!(
                "signing algorithm ID mismatch for certificate #{n}"
            ));
        }
        Certificate::parse(encoded)
            .map_err(|e| format!("failed to decode certificate #{n}: {e}"))?;
        if lineage.iter().any(|(c, _)| c == encoded) {
            return Err(format!(
                "encountered duplicate entries in Proof-of-rotation record at certificate #{n}"
            ));
        }
        lineage.push((encoded.to_vec(), flags));
        last = Some((encoded, sig_alg));
    }
    Ok(lineage)
}
