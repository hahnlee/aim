//! `ApkChecksums.getChecksums` (`android-16.0.0_r1`): each file's
//! checksums, first the ones readily available (the fs-verity digest the
//! kernel keeps, the v3 or v2 signer's per-1 MB content digests), then the
//! required ones still missing, computed (the fs-verity Merkle root, the
//! whole file's MD5, SHA-1, SHA-256 and SHA-512, the content digests over
//! the signing block's sections). A file not on IncFS has no v4 signature
//! and nothing to wait for. Installer-provided digests (`.digests` beside
//! the APK) are not read here: the caller refuses a request that would
//! read them (#1626). Errors are logged and the checksum left out, as the
//! original catches them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
use android_image_extract::source::{FileSource, ReadAt};
use sha2::Digest;

use super::installer::checksums::Checksum;
use super::sign::{self, Build};

/// `Checksum.TYPE_*`.
pub const TYPE_WHOLE_MERKLE_ROOT_4K_SHA256: i32 = 0x01;
pub const TYPE_WHOLE_MD5: i32 = 0x02;
pub const TYPE_WHOLE_SHA1: i32 = 0x04;
pub const TYPE_WHOLE_SHA256: i32 = 0x08;
pub const TYPE_WHOLE_SHA512: i32 = 0x10;
pub const TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256: i32 = 0x20;
pub const TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512: i32 = 0x40;

/// `getApkChecksum`'s read buffer, between these sizes.
const MIN_BUFFER_SIZE: u64 = 4 * 1024;
const MAX_BUFFER_SIZE: u64 = 128 * 1024;

/// A file to checksum: its split (none for the base), where it is, and
/// its fs-verity digest if the kernel keeps one (`VerityUtils.hasFsverity`
/// and `getFsverityDigest`).
pub struct ApkFile {
    pub split: Option<String>,
    pub path: PathBuf,
    pub fs_verity_digest: Option<Vec<u8>>,
}

/// `ApkChecksum`: a file's checksum, and the installer that gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApkChecksum {
    pub split: Option<String>,
    pub checksum: Checksum,
    pub installer_package: Option<String>,
    pub installer_certificate: Option<Vec<u8>>,
}

impl ApkChecksum {
    fn new(split: &Option<String>, kind: i32, value: Vec<u8>) -> ApkChecksum {
        ApkChecksum {
            split: split.clone(),
            checksum: Checksum { kind, value },
            installer_package: None,
            installer_certificate: None,
        }
    }
}

impl WriteParcelable for ApkChecksum {
    fn write_to(&self, p: &mut Parcel) {
        let mut flags = 0;
        if self.split.is_some() {
            flags |= 0x1;
        }
        if self.installer_package.is_some() {
            flags |= 0x4;
        }
        if self.installer_certificate.is_some() {
            flags |= 0x8;
        }
        p.write_i32(flags);
        if let Some(split) = &self.split {
            p.write_string16(Some(split));
        }
        p.write_i32(1);
        self.checksum.write_to(p);
        if let Some(installer) = &self.installer_package {
            p.write_string16(Some(installer));
        }
        if let Some(certificate) = &self.installer_certificate {
            aim_service_aidl::write_byte_array(p, Some(certificate));
        }
    }
}

/// `getChecksums` then `processRequiredChecksums`: every file's checksums,
/// each file's by type.
pub fn checksums(files: &[ApkFile], optional: i32, required: i32, build: &Build) -> Vec<ApkChecksum> {
    let mut all = Vec::new();
    for file in files {
        let mut found = BTreeMap::new();
        available(file, optional | required, build, &mut found);
        computed(file, required, &mut found);
        all.extend(found.into_values());
    }
    all
}

/// `isRequired`.
fn is_required(kind: i32, types: i32, found: &BTreeMap<i32, ApkChecksum>) -> bool {
    types & kind != 0 && !found.contains_key(&kind)
}

fn log(file: &Path, what: &str, error: impl std::fmt::Display) {
    eprintln!("ApkChecksums: {what} of {}: {error}", file.display());
}

/// `getAvailableApkChecksums`.
fn available(file: &ApkFile, types: i32, build: &Build, found: &mut BTreeMap<i32, ApkChecksum>) {
    if !file.path.exists() {
        return;
    }
    // `extractHashFromFS`: fs-verity; a v4 signature is IncFS's.
    if is_required(TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, types, found)
        && let Some(digest) = &file.fs_verity_digest
    {
        found.insert(TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, ApkChecksum::new(&file.split, TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, digest.clone()));
    }
    // `extractHashFromV2V3Signature`.
    if is_required(TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256, types, found) || is_required(TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512, types, found) {
        let digests = FileSource::open(&file.path)
            .map_err(|e| e.to_string())
            .and_then(|apk| sign::signed_content_digests(&apk, build));
        match digests {
            Ok(Some(digests)) => {
                for (kind, algorithm) in [
                    (TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256, sign::CONTENT_DIGEST_CHUNKED_SHA256),
                    (TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512, sign::CONTENT_DIGEST_CHUNKED_SHA512),
                ] {
                    if types & kind != 0
                        && let Some(digest) = digests.get(&algorithm)
                    {
                        found.insert(kind, ApkChecksum::new(&file.split, kind, digest.clone()));
                    }
                }
            }
            Ok(None) => {}
            Err(e) => log(&file.path, "Signature verification error", e),
        }
    }
}

/// `getRequiredApkChecksums`.
fn computed(file: &ApkFile, required: i32, found: &mut BTreeMap<i32, ApkChecksum>) {
    if is_required(TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, required, found) {
        let hash = FileSource::open(&file.path)
            .map_err(|e| e.to_string())
            .and_then(|apk| Ok((apk.size(), sign::fs_verity_root_hash(&apk)?)));
        match hash {
            Ok((size, root)) => {
                let value = verity_hash_for_file(size, &root);
                found.insert(TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, ApkChecksum::new(&file.split, TYPE_WHOLE_MERKLE_ROOT_4K_SHA256, value));
            }
            Err(e) => log(&file.path, "Error calculating WHOLE_MERKLE_ROOT_4K_SHA256", e),
        }
    }
    for kind in [TYPE_WHOLE_MD5, TYPE_WHOLE_SHA1, TYPE_WHOLE_SHA256, TYPE_WHOLE_SHA512] {
        if required & kind != 0 && !found.contains_key(&kind) {
            match whole_file_digest(&file.path, kind) {
                Ok(value) => {
                    found.insert(kind, ApkChecksum::new(&file.split, kind, value));
                }
                Err(e) => log(&file.path, "Error reading to compute hash", e),
            }
        }
    }
    partial(file, required, found);
}

/// `calculatePartialChecksumsIfRequested`.
fn partial(file: &ApkFile, required: i32, found: &mut BTreeMap<i32, ApkChecksum>) {
    let sha256 = is_required(TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256, required, found);
    let sha512 = is_required(TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512, required, found);
    // `getContentDigestAlgos`.
    let algorithms: &[(i32, i32)] = match (sha256, sha512) {
        (false, false) => return,
        (true, true) => &[
            (sign::CONTENT_DIGEST_CHUNKED_SHA256, TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256),
            (sign::CONTENT_DIGEST_CHUNKED_SHA512, TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512),
        ],
        (true, false) => &[(sign::CONTENT_DIGEST_CHUNKED_SHA256, TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256)],
        (false, true) => &[(sign::CONTENT_DIGEST_CHUNKED_SHA512, TYPE_PARTIAL_MERKLE_ROOT_1M_SHA512)],
    };
    let ids: Vec<i32> = algorithms.iter().map(|(id, _)| *id).collect();
    let digests = FileSource::open(&file.path)
        .map_err(|e| e.to_string())
        .and_then(|apk| sign::chunked_content_digests(&apk, &ids));
    match digests {
        Ok(Some(digests)) => {
            for ((_, kind), digest) in algorithms.iter().zip(digests) {
                found.insert(*kind, ApkChecksum::new(&file.split, *kind, digest));
            }
        }
        Ok(None) => log(&file.path, "V2/V3 signatures", "not found"),
        Err(e) => log(&file.path, "Error computing hash", e),
    }
}

/// `getApkChecksum`: the whole file's digest.
fn whole_file_digest(path: &Path, kind: i32) -> Result<Vec<u8>, String> {
    let file = FileSource::open(path).map_err(|e| e.to_string())?;
    let mut digest: Box<dyn sha2::digest::DynDigest> = match kind {
        TYPE_WHOLE_MD5 => Box::new(md5::Md5::default()),
        TYPE_WHOLE_SHA1 => Box::new(sha1::Sha1::default()),
        TYPE_WHOLE_SHA256 => Box::new(sha2::Sha256::default()),
        _ => Box::new(sha2::Sha512::default()),
    };
    let buffer = file.size().clamp(MIN_BUFFER_SIZE, MAX_BUFFER_SIZE);
    let mut offset = 0;
    while offset < file.size() {
        let len = (file.size() - offset).min(buffer);
        digest.update(&file.read_vec(offset, len as usize).map_err(|e| e.to_string())?);
        offset += len;
    }
    Ok(digest.finalize().into_vec())
}

/// `verityHashForFile`: the SHA-256 of the file's `fsverity_descriptor`
/// (version 1, SHA-256, 4096-byte blocks, no salt), its fs-verity digest.
fn verity_hash_for_file(size: u64, root: &[u8]) -> Vec<u8> {
    let mut descriptor = vec![1, 1, 12, 0, 0, 0, 0, 0];
    descriptor.extend_from_slice(&size.to_le_bytes());
    descriptor.extend_from_slice(root);
    descriptor.resize(256, 0);
    sha2::Sha256::digest(&descriptor).to_vec()
}

#[cfg(test)]
mod tests;
