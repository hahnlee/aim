//! The APK Signing Block: finding a scheme's block before the ZIP Central
//! Directory (`ApkSigningBlockUtils`, `ZipUtils`), the signature
//! algorithms, and the content digests a signer signs, over 1 MB chunks or
//! the APK's verity tree (`VerityBuilder`).

use std::collections::BTreeMap;

use android_image_extract::source::ReadAt;

use super::Fail;
use super::crypto::{Algorithm, Hash, Key};

pub(super) const SIGNATURE_RSA_PSS_WITH_SHA256: u32 = 0x0101;
pub(super) const SIGNATURE_RSA_PSS_WITH_SHA512: u32 = 0x0102;
pub(super) const SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA256: u32 = 0x0103;
pub(super) const SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA512: u32 = 0x0104;
pub(super) const SIGNATURE_ECDSA_WITH_SHA256: u32 = 0x0201;
pub(super) const SIGNATURE_ECDSA_WITH_SHA512: u32 = 0x0202;
pub(super) const SIGNATURE_DSA_WITH_SHA256: u32 = 0x0301;
pub(super) const SIGNATURE_VERITY_RSA_PKCS1_V1_5_WITH_SHA256: u32 = 0x0421;
pub(super) const SIGNATURE_VERITY_ECDSA_WITH_SHA256: u32 = 0x0423;
pub(super) const SIGNATURE_VERITY_DSA_WITH_SHA256: u32 = 0x0425;

pub(super) const CONTENT_DIGEST_CHUNKED_SHA256: i32 = 1;
pub(super) const CONTENT_DIGEST_CHUNKED_SHA512: i32 = 2;
pub(super) const CONTENT_DIGEST_VERITY_CHUNKED_SHA256: i32 = 3;

/// The content digests the signers sign, by content digest algorithm.
pub(super) type ContentDigests = BTreeMap<i32, Vec<u8>>;

const CHUNK_SIZE_BYTES: u64 = 1024 * 1024;
const APK_SIG_BLOCK_MAGIC_HI: u64 = 0x3234206b636f6c42;
const APK_SIG_BLOCK_MAGIC_LO: u64 = 0x20676953204b5041;
const APK_SIG_BLOCK_MIN_SIZE: u64 = 32;
const ZIP_EOCD_REC_MIN_SIZE: u64 = 22;
const ZIP_EOCD_REC_SIG: u32 = 0x06054b50;
const ZIP64_EOCD_LOCATOR_SIG: u32 = 0x07064b50;
const ZIP64_EOCD_LOCATOR_SIZE: u64 = 20;
const ZIP_EOCD_CENTRAL_DIR_SIZE_FIELD_OFFSET: usize = 12;
const ZIP_EOCD_CENTRAL_DIR_OFFSET_FIELD_OFFSET: usize = 16;

/// A scheme's block and where the APK's sections are (`SignatureInfo`).
pub(super) struct SignatureInfo {
    pub block: Vec<u8>,
    pub signing_block_offset: u64,
    pub central_dir_offset: u64,
    pub eocd_offset: u64,
    pub eocd: Vec<u8>,
}

/// A little-endian `ByteBuffer` over a block.
#[derive(Clone, Copy)]
pub(super) struct Buf<'a>(pub &'a [u8]);

impl<'a> Buf<'a> {
    pub fn has_remaining(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.0.len()
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let (v, rest) = self.0.split_first_chunk::<4>().ok_or("buffer underflow")?;
        self.0 = rest;
        Ok(u32::from_le_bytes(*v))
    }

    pub fn i32(&mut self) -> Result<i32, String> {
        self.u32().map(|v| v as i32)
    }

    /// `getLengthPrefixedSlice`.
    pub fn slice(&mut self) -> Result<Buf<'a>, String> {
        if self.remaining() < 4 {
            return Err(format!(
                "remaining buffer too short to contain length of length-prefixed field: {}",
                self.remaining()
            ));
        }
        self.bytes()
            .map(Buf)
            .map_err(|e| format!("length-prefixed field: {e}"))
    }

    /// `readLengthPrefixedByteArray`.
    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = self.i32()?;
        if len < 0 || len as usize > self.remaining() {
            return Err(format!("bad length {len}, {} available", self.remaining()));
        }
        let (v, rest) = self.0.split_at(len as usize);
        self.0 = rest;
        Ok(v)
    }
}

fn read(apk: &dyn ReadAt, offset: u64, len: u64) -> Result<Vec<u8>, Fail> {
    apk.read_vec(offset, len as usize)
        .map_err(|e| Fail::Invalid(e.to_string()))
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The block `block_id` of the APK Signing Block, and where the sections
/// are (`ApkSigningBlockUtils.findSignature`).
pub(super) fn find_signature(apk: &dyn ReadAt, block_id: u32) -> Result<SignatureInfo, Fail> {
    let not_found = |s: &str| Fail::NotFound(s.to_owned());
    let (eocd, eocd_offset) = find_eocd(apk)?.ok_or_else(|| {
        not_found("not an APK file: ZIP End of Central Directory record not found")
    })?;
    if eocd_offset >= ZIP64_EOCD_LOCATOR_SIZE
        && le32(&read(apk, eocd_offset - ZIP64_EOCD_LOCATOR_SIZE, 4)?, 0) == ZIP64_EOCD_LOCATOR_SIG
    {
        return Err(not_found("ZIP64 APK not supported"));
    }
    let central_dir_offset = le32(&eocd, ZIP_EOCD_CENTRAL_DIR_OFFSET_FIELD_OFFSET) as u64;
    if central_dir_offset > eocd_offset {
        return Err(not_found("ZIP Central Directory offset out of range"));
    }
    let central_dir_size = le32(&eocd, ZIP_EOCD_CENTRAL_DIR_SIZE_FIELD_OFFSET) as u64;
    if central_dir_offset + central_dir_size != eocd_offset {
        return Err(not_found(
            "ZIP Central Directory is not immediately followed by End of Central Directory",
        ));
    }
    if central_dir_offset < APK_SIG_BLOCK_MIN_SIZE {
        return Err(not_found("APK too small for APK Signing Block"));
    }
    let footer = read(apk, central_dir_offset - 24, 24)?;
    if le64(&footer, 8) != APK_SIG_BLOCK_MAGIC_LO || le64(&footer, 16) != APK_SIG_BLOCK_MAGIC_HI {
        return Err(not_found(
            "no APK Signing Block before ZIP Central Directory",
        ));
    }
    let size = le64(&footer, 0);
    if size < 24 || size > i32::MAX as u64 - 8 {
        return Err(not_found("APK Signing Block size out of range"));
    }
    let total = size + 8;
    let signing_block_offset = central_dir_offset
        .checked_sub(total)
        .ok_or_else(|| not_found("APK Signing Block offset out of range"))?;
    let signing_block = read(apk, signing_block_offset, total)?;
    if le64(&signing_block, 0) != size {
        return Err(not_found(
            "APK Signing Block sizes in header and footer do not match",
        ));
    }
    let mut pairs = &signing_block[8..signing_block.len() - 24];
    let mut entry = 0;
    while !pairs.is_empty() {
        entry += 1;
        if pairs.len() < 8 {
            return Err(not_found(&format!(
                "insufficient data to read size of APK Signing Block entry #{entry}"
            )));
        }
        let len = le64(pairs, 0);
        if !(4..=i32::MAX as u64).contains(&len) || len > (pairs.len() - 8) as u64 {
            return Err(not_found(&format!(
                "APK Signing Block entry #{entry} size out of range: {len}"
            )));
        }
        let len = len as usize;
        if le32(pairs, 8) == block_id {
            return Ok(SignatureInfo {
                block: pairs[12..8 + len].to_vec(),
                signing_block_offset,
                central_dir_offset,
                eocd_offset,
                eocd,
            });
        }
        pairs = &pairs[8 + len..];
    }
    Err(not_found(&format!(
        "no block with ID {block_id} in APK Signing Block"
    )))
}

/// The ZIP End of Central Directory record and its offset
/// (`ZipUtils.findZipEndOfCentralDirectoryRecord`): first where a record
/// without a comment would be, then anywhere a comment would fit.
fn find_eocd(apk: &dyn ReadAt) -> Result<Option<(Vec<u8>, u64)>, Fail> {
    let size = apk.size();
    if size < ZIP_EOCD_REC_MIN_SIZE {
        return Ok(None);
    }
    for max_comment in [0, 0xffff] {
        let max_comment = (max_comment as u64).min(size - ZIP_EOCD_REC_MIN_SIZE);
        let at = size - ZIP_EOCD_REC_MIN_SIZE - max_comment;
        let buf = read(apk, at, ZIP_EOCD_REC_MIN_SIZE + max_comment)?;
        let start = buf.len() - ZIP_EOCD_REC_MIN_SIZE as usize;
        for comment in 0..=max_comment as usize {
            let pos = start - comment;
            if le32(&buf, pos) == ZIP_EOCD_REC_SIG
                && u16::from_le_bytes([buf[pos + 20], buf[pos + 21]]) as usize == comment
            {
                return Ok(Some((buf[pos..].to_vec(), at + pos as u64)));
            }
        }
    }
    Ok(None)
}

pub(super) fn is_supported_signature_algorithm(alg: u32) -> bool {
    matches!(
        alg,
        SIGNATURE_RSA_PSS_WITH_SHA256
            | SIGNATURE_RSA_PSS_WITH_SHA512
            | SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA256
            | SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA512
            | SIGNATURE_ECDSA_WITH_SHA256
            | SIGNATURE_ECDSA_WITH_SHA512
            | SIGNATURE_DSA_WITH_SHA256
            | SIGNATURE_VERITY_RSA_PKCS1_V1_5_WITH_SHA256
            | SIGNATURE_VERITY_ECDSA_WITH_SHA256
            | SIGNATURE_VERITY_DSA_WITH_SHA256
    )
}

/// `getSignatureAlgorithmContentDigestAlgorithm`.
pub(super) fn content_digest_algorithm(alg: u32) -> Result<i32, String> {
    Ok(match alg {
        SIGNATURE_RSA_PSS_WITH_SHA256
        | SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA256
        | SIGNATURE_ECDSA_WITH_SHA256
        | SIGNATURE_DSA_WITH_SHA256 => CONTENT_DIGEST_CHUNKED_SHA256,
        SIGNATURE_RSA_PSS_WITH_SHA512
        | SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA512
        | SIGNATURE_ECDSA_WITH_SHA512 => CONTENT_DIGEST_CHUNKED_SHA512,
        SIGNATURE_VERITY_RSA_PKCS1_V1_5_WITH_SHA256
        | SIGNATURE_VERITY_ECDSA_WITH_SHA256
        | SIGNATURE_VERITY_DSA_WITH_SHA256 => CONTENT_DIGEST_VERITY_CHUNKED_SHA256,
        _ => return Err(format!("unknown signature algorithm: {alg:#x}")),
    })
}

/// `compareSignatureAlgorithm`: whether `a`'s digest is stronger than
/// `b`'s (SHA-512 chunks over verity over SHA-256 chunks).
pub(super) fn stronger(a: u32, b: u32) -> Result<bool, String> {
    let rank = |d| match d {
        CONTENT_DIGEST_CHUNKED_SHA256 => 0,
        CONTENT_DIGEST_VERITY_CHUNKED_SHA256 => 1,
        _ => 2,
    };
    Ok(rank(content_digest_algorithm(a)?) > rank(content_digest_algorithm(b)?))
}

/// `getSignatureAlgorithmJcaKeyAlgorithm` and
/// `getSignatureAlgorithmJcaSignatureAlgorithm`.
pub(super) fn jca_algorithm(alg: u32) -> Result<Algorithm, String> {
    let (key, hash, pss) = match alg {
        SIGNATURE_RSA_PSS_WITH_SHA256 => (Key::Rsa, Hash::Sha256, true),
        SIGNATURE_RSA_PSS_WITH_SHA512 => (Key::Rsa, Hash::Sha512, true),
        SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA256 | SIGNATURE_VERITY_RSA_PKCS1_V1_5_WITH_SHA256 => {
            (Key::Rsa, Hash::Sha256, false)
        }
        SIGNATURE_RSA_PKCS1_V1_5_WITH_SHA512 => (Key::Rsa, Hash::Sha512, false),
        SIGNATURE_ECDSA_WITH_SHA256 | SIGNATURE_VERITY_ECDSA_WITH_SHA256 => {
            (Key::Ec, Hash::Sha256, false)
        }
        SIGNATURE_ECDSA_WITH_SHA512 => (Key::Ec, Hash::Sha512, false),
        SIGNATURE_DSA_WITH_SHA256 | SIGNATURE_VERITY_DSA_WITH_SHA256 => {
            (Key::Dsa, Hash::Sha256, false)
        }
        _ => return Err(format!("unknown signature algorithm: {alg:#x}")),
    };
    Ok(Algorithm { key, hash, pss })
}

/// `ApkSigningBlockUtils.verifyIntegrity`: the APK's contents against
/// each digest the signers signed.
pub(super) fn verify_integrity(
    digests: &ContentDigests,
    apk: &dyn ReadAt,
    info: &SignatureInfo,
) -> Result<(), String> {
    if digests.is_empty() {
        return Err("no digests provided".into());
    }
    let chunked: Vec<i32> = [CONTENT_DIGEST_CHUNKED_SHA256, CONTENT_DIGEST_CHUNKED_SHA512]
        .into_iter()
        .filter(|d| digests.contains_key(d))
        .collect();
    if !chunked.is_empty() {
        for (alg, actual) in chunked.iter().zip(chunked_digests(&chunked, apk, info)?) {
            if digests[alg] != actual {
                return Err(format!("{alg} digest of contents did not verify"));
            }
        }
    }
    if let Some(verity) = digests.get(&CONTENT_DIGEST_VERITY_CHUNKED_SHA256) {
        let expected = verity_root_hash(verity, apk.size(), info)?;
        if expected != apk_verity_root_hash(apk, info)? {
            return Err("APK verity digest of contents did not verify".into());
        }
    } else if chunked.is_empty() {
        return Err("no known digest exists for integrity check".into());
    }
    Ok(())
}

/// The sections a signer's digest covers: everything before the APK
/// Signing Block, the Central Directory, and the End of Central Directory
/// with the Central Directory's offset as the Signing Block's.
fn sections<'a>(apk: &'a dyn ReadAt, info: &SignatureInfo) -> [Section<'a>; 3] {
    let mut eocd = info.eocd.clone();
    eocd[ZIP_EOCD_CENTRAL_DIR_OFFSET_FIELD_OFFSET..ZIP_EOCD_CENTRAL_DIR_OFFSET_FIELD_OFFSET + 4]
        .copy_from_slice(&(info.signing_block_offset as u32).to_le_bytes());
    [
        Section::File(apk, 0, info.signing_block_offset),
        Section::File(
            apk,
            info.central_dir_offset,
            info.eocd_offset - info.central_dir_offset,
        ),
        Section::Memory(eocd),
    ]
}

enum Section<'a> {
    File(&'a dyn ReadAt, u64, u64),
    Memory(Vec<u8>),
}

impl Section<'_> {
    fn size(&self) -> u64 {
        match self {
            Section::File(_, _, len) => *len,
            Section::Memory(b) => b.len() as u64,
        }
    }

    /// The bytes at `offset`, `len` of them.
    fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        match self {
            Section::File(apk, start, _) => apk
                .read_vec(start + offset, len as usize)
                .map_err(|e| e.to_string()),
            Section::Memory(b) => Ok(b[offset as usize..(offset + len) as usize].to_vec()),
        }
    }
}

/// `computeContentDigestsPer1MbChunk`: each section's 1 MB chunks digested
/// with a prefix of 0xa5 and their size, then the chunks' digests with a
/// prefix of 0x5a and their count.
fn chunked_digests(
    algorithms: &[i32],
    apk: &dyn ReadAt,
    info: &SignatureInfo,
) -> Result<Vec<Vec<u8>>, String> {
    let hash = |a: &i32| match *a {
        CONTENT_DIGEST_CHUNKED_SHA512 => Hash::Sha512,
        _ => Hash::Sha256,
    };
    let sections = sections(apk, info);
    let count: u64 = sections
        .iter()
        .map(|s| s.size().div_ceil(CHUNK_SIZE_BYTES))
        .sum();
    if count >= (i32::MAX / 1024) as u64 {
        return Err(format!("too many chunks: {count}"));
    }
    let mut tops: Vec<Vec<u8>> = algorithms
        .iter()
        .map(|_| {
            let mut v = vec![0x5a];
            v.extend_from_slice(&(count as u32).to_le_bytes());
            v
        })
        .collect();
    for section in &sections {
        let mut offset = 0;
        while offset < section.size() {
            let len = (section.size() - offset).min(CHUNK_SIZE_BYTES);
            let chunk = section.read(offset, len)?;
            for (a, top) in algorithms.iter().zip(&mut tops) {
                let mut h = hash(a).hasher();
                h.update(&[0xa5]);
                h.update(&(len as u32).to_le_bytes());
                h.update(&chunk);
                top.extend_from_slice(&h.finalize());
            }
            offset += len;
        }
    }
    Ok(algorithms
        .iter()
        .zip(tops)
        .map(|(a, top)| hash(a).digest(&top))
        .collect())
}

/// `parseVerityDigestAndVerifySourceLength`: a verity digest's root hash,
/// once the source length it records is the APK's without its Signing
/// Block.
pub(super) fn verity_root_hash(
    data: &[u8],
    file_size: u64,
    info: &SignatureInfo,
) -> Result<Vec<u8>, String> {
    if data.len() != 40 {
        return Err(format!("verity digest size is wrong: {}", data.len()));
    }
    let source_len = le64(data, 32);
    let signing_block = info.central_dir_offset - info.signing_block_offset;
    if source_len != file_size - signing_block {
        return Err("APK content size did not verify".into());
    }
    Ok(data[..32].to_vec())
}

/// The fs-verity block size, and the salt of the APK's verity tree.
const VERITY_BLOCK: usize = 4096;
const VERITY_SALT: [u8; 8] = [0; 8];

/// `VerityBuilder.generateApkVerityTree`'s root hash: the Merkle tree over
/// the APK without its Signing Block, with the End of Central Directory
/// pointing at where the Signing Block was.
fn apk_verity_root_hash(apk: &dyn ReadAt, info: &SignatureInfo) -> Result<Vec<u8>, String> {
    if !info
        .signing_block_offset
        .is_multiple_of(VERITY_BLOCK as u64)
        || !(info.central_dir_offset - info.signing_block_offset)
            .is_multiple_of(VERITY_BLOCK as u64)
    {
        return Err("APK Signing Block is not page aligned".into());
    }
    let mut leaves = Merkle::default();
    for section in sections(apk, info) {
        let mut offset = 0;
        while offset < section.size() {
            let len = (section.size() - offset).min(CHUNK_SIZE_BYTES);
            leaves.update(&section.read(offset, len)?);
            offset += len;
        }
    }
    let mut level = leaves.finish();
    loop {
        if level.len() <= VERITY_BLOCK {
            let mut padded = level;
            padded.resize(VERITY_BLOCK, 0);
            return Ok(salted(&padded));
        }
        let mut next = Merkle::default();
        next.update(&level);
        level = next.finish();
    }
}

/// A level of a Merkle tree: the salted SHA-256 of each block of its input.
#[derive(Default)]
struct Merkle {
    block: Vec<u8>,
    hashes: Vec<u8>,
}

impl Merkle {
    fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = (VERITY_BLOCK - self.block.len()).min(data.len());
            self.block.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.block.len() == VERITY_BLOCK {
                self.hashes.extend_from_slice(&salted(&self.block));
                self.block.clear();
            }
        }
    }

    /// The hashes, the last block padded with zeros.
    fn finish(mut self) -> Vec<u8> {
        if !self.block.is_empty() {
            self.block.resize(VERITY_BLOCK, 0);
            self.hashes.extend_from_slice(&salted(&self.block));
        }
        self.hashes
    }
}

fn salted(block: &[u8]) -> Vec<u8> {
    let mut h = Hash::Sha256.hasher();
    h.update(&VERITY_SALT);
    h.update(block);
    h.finalize().into_vec()
}
