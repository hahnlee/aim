//! APK Signature Scheme v2 (`ApkSignatureSchemeV2Verifier`), and the
//! signer record v2 and v3 share.

use android_image_extract::source::ReadAt;

use super::asn1::Certificate;
use super::block::{self, Buf, ContentDigests, SignatureInfo};
use super::{Fail, crypto};

const APK_SIGNATURE_SCHEME_V2_BLOCK_ID: u32 = 0x7109871a;
/// The ID of this scheme in a JAR signature's `X-Android-APK-Signed`.
pub(super) const SF_ATTRIBUTE_ANDROID_APK_SIGNED_ID: i32 = 2;
const MAX_V2_SIGNERS: usize = 10;
/// The attribute that says which newer scheme signed the APK too.
const STRIPPING_PROTECTION_ATTR_ID: u32 = 0xbeeff00d;

pub(super) struct Verified {
    /// Each signer's certificate chain.
    pub certs: Vec<Vec<Vec<u8>>>,
    pub content_digests: ContentDigests,
}

pub(super) fn verify(apk: &dyn ReadAt, full: bool) -> Result<Verified, Fail> {
    let info = block::find_signature(apk, APK_SIGNATURE_SCHEME_V2_BLOCK_ID)?;
    verify_block(apk, &info, full).map_err(Fail::Invalid)
}

fn verify_block(apk: &dyn ReadAt, info: &SignatureInfo, full: bool) -> Result<Verified, String> {
    let mut content_digests = ContentDigests::new();
    let mut certs = Vec::new();
    let mut signers = Buf(&info.block)
        .slice()
        .map_err(|e| format!("failed to read list of signers: {e}"))?;
    while signers.has_remaining() {
        if certs.len() == MAX_V2_SIGNERS {
            return Err(format!(
                "APK Signature Scheme v2 only supports a maximum of {MAX_V2_SIGNERS} signers"
            ));
        }
        let n = certs.len() + 1;
        let signer = (|| {
            let mut signer = signers.slice()?;
            let signed_data = signer.slice()?;
            let signed = verify_signed_data(signed_data, &mut signer, &mut content_digests)?;
            verify_additional_attributes(signed.rest)?;
            Ok::<_, String>(signed.certs)
        })()
        .map_err(|e| format!("failed to parse/verify signer #{n} block: {e}"))?;
        certs.push(signer);
    }
    if certs.is_empty() {
        return Err("no signers found".into());
    }
    finish(apk, info, &content_digests, full)?;
    Ok(Verified {
        certs,
        content_digests,
    })
}

/// The checks after the signers: the contents' integrity, if asked for,
/// and a verity digest's source length.
pub(super) fn finish(
    apk: &dyn ReadAt,
    info: &SignatureInfo,
    digests: &ContentDigests,
    full: bool,
) -> Result<(), String> {
    if digests.is_empty() {
        return Err("no content digests found".into());
    }
    if full {
        block::verify_integrity(digests, apk, info)?;
    }
    if let Some(verity) = digests.get(&block::CONTENT_DIGEST_VERITY_CHUNKED_SHA256) {
        block::verity_root_hash(verity, apk.size(), info)?;
    }
    Ok(())
}

/// A signer's signed data, once its signature verified: its certificate
/// chain, and what follows the certificates.
pub(super) struct Signed<'a> {
    pub certs: Vec<Vec<u8>>,
    pub rest: Buf<'a>,
}

/// `verifySigner` up to the additional attributes, from the signatures and
/// public key that follow the signed data in `signer`: the strongest
/// supported signature over the signed data, the digest of that algorithm
/// (which must agree with an earlier signer's), and the certificates, the
/// first of which must hold the signer's public key.
pub(super) fn verify_signed_data<'a>(
    signed_data: Buf<'a>,
    signer: &mut Buf<'a>,
    content_digests: &mut ContentDigests,
) -> Result<Signed<'a>, String> {
    let mut signatures = signer.slice()?;
    let public_key = signer.bytes()?;
    let mut count = 0;
    let mut best: Option<(u32, &[u8])> = None;
    let mut signature_algorithms = Vec::new();
    while signatures.has_remaining() {
        count += 1;
        let mut signature = signatures
            .slice()
            .map_err(|e| format!("failed to parse signature record #{count}: {e}"))?;
        if signature.remaining() < 8 {
            return Err("signature record too short".into());
        }
        let alg = signature.u32()?;
        signature_algorithms.push(alg);
        if !block::is_supported_signature_algorithm(alg) {
            continue;
        }
        if best.is_none_or(|(b, _)| block::stronger(alg, b).unwrap_or(false)) {
            let bytes = signature
                .bytes()
                .map_err(|e| format!("failed to parse signature record #{count}: {e}"))?;
            best = Some((alg, bytes));
        }
    }
    let Some((best_alg, best_signature)) = best else {
        return Err(if count == 0 {
            "no signatures found".into()
        } else {
            "no supported signatures found".into()
        });
    };
    let jca = block::jca_algorithm(best_alg)?;
    crypto::verify(jca, public_key, signed_data.0, best_signature)
        .map_err(|e| format!("failed to verify {jca:?} signature: {e}"))?;

    let mut signed = signed_data;
    let mut digests = signed.slice()?;
    let mut content_digest = None;
    let mut digest_algorithms = Vec::new();
    let mut n = 0;
    while digests.has_remaining() {
        n += 1;
        let mut digest = digests
            .slice()
            .map_err(|e| format!("failed to parse digest record #{n}: {e}"))?;
        if digest.remaining() < 8 {
            return Err(format!(
                "failed to parse digest record #{n}: record too short"
            ));
        }
        let alg = digest.u32()?;
        digest_algorithms.push(alg);
        if alg == best_alg {
            content_digest = Some(digest.bytes()?.to_vec());
        }
    }
    if signature_algorithms != digest_algorithms {
        return Err(
            "signature algorithms don't match between digests and signatures records".into(),
        );
    }
    let digest_alg = block::content_digest_algorithm(best_alg)?;
    let content_digest = content_digest.unwrap_or_default();
    if let Some(previous) = content_digests.insert(digest_alg, content_digest.clone())
        && previous != content_digest
    {
        return Err(
            "contents digest does not match the digest specified by a preceding signer".into(),
        );
    }

    let mut certificates = signed.slice()?;
    let mut certs = Vec::new();
    while certificates.has_remaining() {
        let encoded = certificates.bytes()?;
        Certificate::parse(encoded)
            .map_err(|e| format!("failed to decode certificate #{}: {e}", certs.len() + 1))?;
        certs.push(encoded.to_vec());
    }
    let Some(main) = certs.first() else {
        return Err("no certificates listed".into());
    };
    if Certificate::parse(main)?.public_key != public_key {
        return Err("public key mismatch between certificate and signature record".into());
    }
    Ok(Signed {
        certs,
        rest: signed,
    })
}

fn verify_additional_attributes(mut rest: Buf) -> Result<(), String> {
    let mut attrs = rest.slice()?;
    while attrs.has_remaining() {
        let mut attr = attrs.slice()?;
        if attr.remaining() < 4 {
            return Err("remaining buffer too short to contain additional attribute ID".into());
        }
        if attr.u32()? == STRIPPING_PROTECTION_ATTR_ID {
            if attr.remaining() < 4 {
                return Err(
                    "V2 Signature Scheme Stripping Protection Attribute value too small".into(),
                );
            }
            if attr.i32()? == super::v3::SF_ATTRIBUTE_ANDROID_APK_SIGNED_ID {
                return Err(
                    "V2 signature indicates APK is signed using APK Signature Scheme v3, \
                     but none was found. Signature stripped?"
                        .into(),
                );
            }
        }
    }
    Ok(())
}
