use std::fs;
use std::path::PathBuf;

use super::*;

/// Signed with the JAR scheme, v2 and v3, the `.SF` naming v2 and v3.
const OVERLAY: &str = "product/overlay/EmulationPixel4/EmulationPixel4Overlay.apk";
/// Signed with v3 and a proof-of-rotation lineage of two certificates.
const ROTATED: &str = "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk";

fn build() -> Build {
    Build {
        sdk_int: 36,
        release: true,
        always_load_past_certs_v4: true,
    }
}

fn image_apk(file: &str) -> Option<Vec<u8>> {
    let root = aim_paths::original_image_with(file)?;
    Some(fs::read(root.join(file)).unwrap())
}

/// A test APK of the CTS release (`tools/cts-module.py` fetches them).
fn cts(file: &str) -> Option<PathBuf> {
    let path = aim_paths::fetched().join("cts").join(file);
    if !path.exists() {
        aim_paths::skip(&format!(
            "no _build/cts/{file} (python3 tools/cts-module.py)"
        ));
        return None;
    }
    Some(path)
}

fn verify_bytes(
    data: &Vec<u8>,
    v4: Option<V4Source>,
    min: i32,
    full: bool,
) -> Result<SigningDetails, Error> {
    let apk = Apk {
        path: "/data/app/test.apk",
        data,
        v4,
    };
    verify(&apk, min, full, &build())
}

fn le32(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
}

fn le64(b: &[u8], at: usize) -> usize {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) as usize
}

/// `apk` with only the APK Signing Block's pairs whose IDs `keep` holds,
/// and no block when it keeps none. What precedes the block keeps its
/// offsets, so the remaining signers' digests still hold.
fn strip(apk: &[u8], keep: &[u32]) -> Vec<u8> {
    let eocd = apk.len() - 22;
    assert_eq!(&apk[eocd..eocd + 4], b"PK\x05\x06");
    let cd = le32(apk, eocd + 16);
    let start = cd - (le64(apk, cd - 24) + 8);
    let mut pairs = Vec::new();
    let mut p = start + 8;
    while p < cd - 24 {
        let len = le64(apk, p);
        if keep.contains(&(le32(apk, p + 8) as u32)) {
            pairs.extend_from_slice(&apk[p..p + 8 + len]);
        }
        p += 8 + len;
    }
    let mut out = apk[..start].to_vec();
    if !pairs.is_empty() {
        let size = (pairs.len() + 24) as u64;
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&pairs);
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&apk[cd - 16..cd]);
    }
    let new_cd = out.len() as u32;
    out.extend_from_slice(&apk[cd..]);
    let eocd = out.len() - 22;
    out[eocd + 16..eocd + 20].copy_from_slice(&new_cd.to_le_bytes());
    out
}

const V2: u32 = 0x7109871a;

#[test]
fn verifies_v3_and_collects_the_same_without_verifying() {
    let Some(apk) = image_apk(OVERLAY) else {
        return;
    };
    let full = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap();
    assert_eq!(full.scheme_version, SIGNING_BLOCK_V3);
    assert_eq!(full.signatures.len(), 1);
    assert_eq!(full.public_keys.len(), 1);
    assert_eq!(full.past_signing_certificates, None);
    assert_eq!(
        verify_bytes(&apk, None, SIGNING_BLOCK_V2, false).unwrap(),
        full
    );
}

#[test]
fn reads_the_rotation_lineage() {
    let Some(apk) = image_apk(ROTATED) else {
        return;
    };
    let d = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap();
    let lineage = d.past_signing_certificates.unwrap();
    // The first signer may install, share a uid, use a permission and
    // roll back (`CertCapabilities`); the current one may also be
    // used for authentication.
    assert_eq!(
        lineage.iter().map(|(_, f)| *f).collect::<Vec<_>>(),
        [21, 23]
    );
    assert_eq!(lineage[1].0, d.signatures[0]);
    assert_ne!(lineage[0].0, d.signatures[0]);
}

#[test]
fn rejects_tampered_contents_only_when_verifying() {
    let Some(mut apk) = image_apk(OVERLAY) else {
        return;
    };
    let signer = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap();
    apk[100] ^= 1;
    let e = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap_err();
    assert_eq!(e.code, INSTALL_PARSE_FAILED_NO_CERTIFICATES);
    assert!(
        e.message.contains("Scheme v3") && e.message.contains("did not verify"),
        "{e}"
    );
    // Collecting certificates trusts the contents, as the scan of the
    // system partitions does.
    assert_eq!(
        verify_bytes(&apk, None, SIGNING_BLOCK_V2, false).unwrap(),
        signer
    );
}

#[test]
fn v2_detects_a_stripped_v3_signature() {
    let Some(apk) = image_apk(OVERLAY) else {
        return;
    };
    let stripped = strip(&apk, &[V2]);
    for full in [true, false] {
        let e = verify_bytes(&stripped, None, JAR, full).unwrap_err();
        assert!(
            e.message.contains("Scheme v2") && e.message.contains("stripped"),
            "{e}"
        );
    }
}

#[test]
fn jar_signature_detects_a_stripped_signing_block() {
    let Some(apk) = image_apk(OVERLAY) else {
        return;
    };
    let signer = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap();
    let stripped = strip(&apk, &[]);
    let e = verify_bytes(&stripped, None, JAR, true).unwrap_err();
    assert_eq!(e.code, INSTALL_PARSE_FAILED_NO_CERTIFICATES);
    assert!(e.message.contains("Signature stripped?"), "{e}");
    // Without verifying, the JAR signature gives the same signer; a
    // package targeting R or later needs v2 or newer.
    let jar = verify_bytes(&stripped, None, JAR, false).unwrap();
    assert_eq!(jar.scheme_version, JAR);
    assert_eq!(jar.signatures, signer.signatures);
    let e = verify_bytes(&stripped, None, minimum_signature_scheme(36), false).unwrap_err();
    assert!(
        e.message.contains("no APK Signature Scheme v2 signature"),
        "{e}"
    );
}

#[test]
fn verifies_jar_signed_entries() {
    let Some(path) = cts("CtsPkgInstallTinyAppV1.apk") else {
        return;
    };
    let mut apk = fs::read(path).unwrap();
    let d = verify_bytes(&apk, None, JAR, true).unwrap();
    assert_eq!(d.scheme_version, JAR);
    assert_eq!(d.signatures.len(), 1);
    // An entry other than the manifest is checked only when verifying.
    let dex = apk.windows(11).position(|w| w == b"classes.dex").unwrap();
    let data = dex + 11 + le16(&apk, dex - 2);
    apk[data + 16] ^= 1;
    assert!(verify_bytes(&apk, None, JAR, true).is_err());
    assert_eq!(verify_bytes(&apk, None, JAR, false).unwrap(), d);
}

fn le16(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

#[test]
fn verifies_v4_against_its_v3_signer() {
    let (Some(path), Some(other)) = (
        cts("CtsPkgInstallTinyAppV2V3V4.apk"),
        cts("CtsPkgInstallTinyAppV2V3V4-Sha512withEC.apk.idsig"),
    ) else {
        return;
    };
    let apk = fs::read(&path).unwrap();
    let mut idsig_path = path.into_os_string();
    idsig_path.push(".idsig");
    let idsig = fs::read(idsig_path).unwrap();
    let v3 = verify_bytes(&apk, None, SIGNING_BLOCK_V2, true).unwrap();
    let v4 = verify_bytes(&apk, Some(V4Source::Incfs(&idsig)), SIGNING_BLOCK_V2, true).unwrap();
    assert_eq!(v4.scheme_version, SIGNING_BLOCK_V4);
    assert_eq!(v4.signatures, v3.signatures);
    // Another APK's v4 signature, and one fs-verity does not protect.
    let other = fs::read(other).unwrap();
    assert!(verify_bytes(&apk, Some(V4Source::Incfs(&other)), SIGNING_BLOCK_V2, true).is_err());
    let unprotected = V4Source::IdSig {
        signature: &idsig,
        fs_verity_digest: None,
    };
    let e = verify_bytes(&apk, Some(unprotected), SIGNING_BLOCK_V2, true).unwrap_err();
    assert!(e.message.contains("fs-verity"), "{e}");
}
