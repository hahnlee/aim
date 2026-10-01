//! APK Signature Scheme v4 (`ApkSignatureSchemeV4Verifier`,
//! `V4Signature`): a signature over the APK's fs-verity Merkle tree, kept
//! by IncFS for an incremental install or beside the APK in `.idsig`,
//! which names the v2 or v3 signer it extends.

use super::asn1::Certificate;
use super::block::{self, Buf};
use super::{Fail, crypto};

/// The v4 signature of an APK, where the platform keeps one.
pub enum V4Source<'a> {
    /// The signature IncFS holds for the file
    /// (`IncrementalManager.unsafeGetFileSignature`).
    Incfs(&'a [u8]),
    /// The `.idsig` beside the APK, and the file's fs-verity digest
    /// (`FS_IOC_MEASURE_VERITY`) if fs-verity protects it.
    IdSig {
        signature: &'a [u8],
        fs_verity_digest: Option<&'a [u8]>,
    },
}

const SUPPORTED_VERSION: i32 = 2;
const HASHING_ALGORITHM_SHA256: i32 = 1;
const LOG2_BLOCK_SIZE_4096_BYTES: u8 = 12;
/// `incrementalfs.h`'s `INCFS_MAX_SIGNATURE_SIZE`.
const INCFS_MAX_SIGNATURE_SIZE: usize = 8096;
/// The v3 block ID that means the default signer.
pub(super) const APK_SIGNATURE_SCHEME_DEFAULT: u32 = 0xffffffff;

/// `V4Signature.HashingInfo`.
pub(super) struct HashingInfo<'a> {
    hash_algorithm: i32,
    log2_block_size: u8,
    salt: &'a [u8],
    raw_root_hash: &'a [u8],
}

/// `V4Signature.SigningInfo`.
struct SigningInfo<'a> {
    apk_digest: &'a [u8],
    certificate: &'a [u8],
    additional_data: &'a [u8],
    public_key: &'a [u8],
    signature_algorithm: u32,
    signature: &'a [u8],
}

/// `V4Signature.SigningInfos`: the default signer, and the signers of the
/// extended v3 blocks by block ID.
pub(super) struct SigningInfos<'a> {
    default: &'a [u8],
    blocks: Vec<(u32, &'a [u8])>,
}

impl SigningInfos<'_> {
    pub fn has_blocks(&self) -> bool {
        !self.blocks.is_empty()
    }
}

pub(super) struct Verified {
    pub cert: Vec<u8>,
    pub apk_digest: Vec<u8>,
}

/// `extractSignature`: the signature's hashing and signing infos, once a
/// `.idsig`'s tree is the one fs-verity measured.
pub(super) fn extract<'a>(
    source: Option<&V4Source<'a>>,
    file_size: u64,
) -> Result<(HashingInfo<'a>, SigningInfos<'a>), Fail> {
    let not_found = || Fail::NotFound("no APK Signature Scheme v4 signature".into());
    let (bytes, fs_verity_digest) = match source.ok_or_else(not_found)? {
        V4Source::Incfs(b) if !b.is_empty() => (*b, None),
        V4Source::Incfs(_) => return Err(not_found()),
        V4Source::IdSig {
            signature,
            fs_verity_digest,
        } => (*signature, Some(*fs_verity_digest)),
    };
    // A truncated `.idsig` is no signature; a truncated one from IncFS is
    // an invalid one.
    let (version, hashing, signing) = read_signature(bytes).ok_or_else(|| {
        if fs_verity_digest.is_some() {
            not_found()
        } else {
            Fail::Invalid("V4 signature is invalid".into())
        }
    })??;
    if version != SUPPORTED_VERSION {
        return Err(Fail::Invalid(format!(
            "v4 signature version {version} is not supported"
        )));
    }
    let invalid = |e: String| Fail::Invalid(format!("V4 signature is invalid: {e}"));
    let hashing = hashing_info(hashing).map_err(invalid)?;
    let signing = signing_infos(signing).map_err(invalid)?;
    if let Some(actual) = fs_verity_digest {
        let actual =
            actual.ok_or_else(|| Fail::Invalid("the APK does not have fs-verity".into()))?;
        if fs_verity_digest_of(file_size, &hashing).map_err(Fail::Invalid)? != actual {
            return Err(Fail::Invalid(
                "actual digest does not match the v4 signature".into(),
            ));
        }
    }
    Ok((hashing, signing))
}

/// A v4 signature's version, hashing info and signing infos.
type Parts<'a> = (i32, &'a [u8], &'a [u8]);

/// `V4Signature.readFrom`: a version, then the hashing info and the
/// signing infos, each prefixed with its size; a truncated one is absent.
/// None if even the version is truncated.
fn read_signature(b: &[u8]) -> Option<Result<Parts<'_>, Fail>> {
    let mut buf = Buf(b);
    let version = buf.i32().ok()?;
    let mut max = INCFS_MAX_SIGNATURE_SIZE as i32;
    let parts = part(&mut buf, &mut max).and_then(|h| Ok((h, part(&mut buf, &mut max)?)));
    Some(parts.map(|(h, s)| (version, h, s)))
}

/// `readBytes`: a part of at most `max` bytes, empty if truncated.
fn part<'a>(buf: &mut Buf<'a>, max: &mut i32) -> Result<&'a [u8], Fail> {
    let Ok(size) = Buf(buf.0).i32() else {
        buf.0 = &[];
        return Ok(&[]);
    };
    if size > *max {
        return Err(Fail::NotFound(format!(
            "signature is too long; max allowed is {INCFS_MAX_SIGNATURE_SIZE}"
        )));
    }
    if size < 0 {
        return Err(Fail::Invalid(format!("negative signature size {size}")));
    }
    let Ok(v) = buf.bytes() else {
        buf.0 = &[];
        return Ok(&[]);
    };
    *max -= size;
    Ok(v)
}

fn hashing_info(b: &[u8]) -> Result<HashingInfo<'_>, String> {
    let mut buf = Buf(b);
    let hash_algorithm = buf.i32()?;
    let (&log2_block_size, rest) = buf.0.split_first().ok_or("truncated hashing info")?;
    buf.0 = rest;
    Ok(HashingInfo {
        hash_algorithm,
        log2_block_size,
        salt: buf.bytes()?,
        raw_root_hash: buf.bytes()?,
    })
}

fn signing_info<'a>(buf: &mut Buf<'a>) -> Result<SigningInfo<'a>, String> {
    Ok(SigningInfo {
        apk_digest: buf.bytes()?,
        certificate: buf.bytes()?,
        additional_data: buf.bytes()?,
        public_key: buf.bytes()?,
        signature_algorithm: buf.u32()?,
        signature: buf.bytes()?,
    })
}

fn signing_infos(b: &[u8]) -> Result<SigningInfos<'_>, String> {
    let mut buf = Buf(b);
    signing_info(&mut buf)?;
    let default = &b[..b.len() - buf.remaining()];
    let mut blocks = Vec::new();
    while buf.has_remaining() {
        blocks.push((buf.u32()?, buf.bytes()?));
    }
    Ok(SigningInfos { default, blocks })
}

/// `VerityUtils.generateFsVerityDigest`: the digest of the fs-verity
/// descriptor for the tree the hashing info describes.
fn fs_verity_digest_of(file_size: u64, h: &HashingInfo) -> Result<Vec<u8>, String> {
    if h.raw_root_hash.len() != 32 {
        return Err("expect a 32-byte rootHash for SHA256".into());
    }
    if h.log2_block_size != LOG2_BLOCK_SIZE_4096_BYTES {
        return Err(format!("unsupported log2BlockSize: {}", h.log2_block_size));
    }
    let mut d = vec![0u8; 256];
    d[..4].copy_from_slice(&[1, 1, h.log2_block_size, 0]);
    d[8..16].copy_from_slice(&file_size.to_le_bytes());
    d[16..48].copy_from_slice(h.raw_root_hash);
    Ok(crypto::Hash::Sha256.digest(&d))
}

/// `ApkSignatureSchemeV4Verifier.verify`: the signer for `v3_block_id`
/// (the default one for none or v3.0), its signature over the tree and
/// the signer's records, and its certificate.
pub(super) fn verify(
    file_size: u64,
    hashing: &HashingInfo,
    infos: &SigningInfos,
    v3_block_id: u32,
) -> Result<Verified, String> {
    let bytes = if v3_block_id == APK_SIGNATURE_SCHEME_DEFAULT
        || v3_block_id == super::v3::APK_SIGNATURE_SCHEME_V3_BLOCK_ID
    {
        infos.default
    } else {
        infos
            .blocks
            .iter()
            .find(|(id, _)| *id == v3_block_id)
            .map(|(_, b)| *b)
            .ok_or_else(|| {
                format!(
                    "failed to find V4 signature block corresponding to V3 blockId: {v3_block_id}"
                )
            })?
    };
    let info = signing_info(&mut Buf(bytes))
        .map_err(|e| format!("failed to read V4 signature block: {e}"))?;
    let signed_data = signed_data(file_size, hashing, &info);
    if !block::is_supported_signature_algorithm(info.signature_algorithm) {
        return Err("no supported signatures found".into());
    }
    let jca = block::jca_algorithm(info.signature_algorithm)?;
    crypto::verify(jca, info.public_key, &signed_data, info.signature)
        .map_err(|e| format!("failed to verify {jca:?} signature: {e}"))?;
    let cert = Certificate::parse(info.certificate)
        .map_err(|e| format!("failed to decode certificate: {e}"))?;
    if cert.public_key != info.public_key {
        return Err("public key mismatch between certificate and signature record".into());
    }
    if hashing.hash_algorithm != HASHING_ALGORITHM_SHA256 {
        return Err(format!(
            "unsupported hashAlgorithm: {}",
            hashing.hash_algorithm
        ));
    }
    Ok(Verified {
        cert: info.certificate.to_vec(),
        apk_digest: info.apk_digest.to_vec(),
    })
}

/// `V4Signature.getSignedData`.
fn signed_data(file_size: u64, h: &HashingInfo, s: &SigningInfo) -> Vec<u8> {
    let parts = [
        h.salt,
        h.raw_root_hash,
        s.apk_digest,
        s.certificate,
        s.additional_data,
    ];
    let size = 4 + 8 + 4 + 1 + parts.iter().map(|p| 4 + p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&h.hash_algorithm.to_le_bytes());
    out.push(h.log2_block_size);
    for p in parts {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}
