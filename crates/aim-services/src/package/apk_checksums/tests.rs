use std::os::fd::AsFd;
use std::process::Command;

use super::*;

/// An APK of the image the app host runs, signed with v2 or v3.
fn image_apk() -> Option<PathBuf> {
    aim_paths::input(
        aim_paths::out().join("app-host/root/system/priv-app/Telecom/Telecom.apk"),
        "app-host",
    )
}

fn build() -> Build {
    Build {
        sdk_int: 36,
        release: true,
        always_load_past_certs_v4: false,
    }
}

fn apk(path: &Path, fs_verity_digest: Option<Vec<u8>>) -> ApkFile {
    ApkFile {
        split: None,
        path: path.to_owned(),
        fs_verity_digest,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `ApkChecksum.writeToParcel`: its flags, the present fields, the
/// checksum as a typed object.
#[test]
fn apk_checksums_parcel_as_their_writer_writes_them() {
    let mut checksum =
        ApkChecksum::new(&Some("config.en".into()), TYPE_WHOLE_SHA256, vec![1, 2, 3]);
    let mut p = Parcel::new();
    checksum.write_to(&mut p);
    let mut r = p.reader();
    assert_eq!(r.read_i32().unwrap(), 0x1);
    assert_eq!(r.read_string16().unwrap().as_deref(), Some("config.en"));
    assert_eq!(r.read_i32().unwrap(), 1);
    assert_eq!(r.read_i32().unwrap(), TYPE_WHOLE_SHA256);
    assert_eq!(
        aim_service_aidl::read_byte_array(&mut r).unwrap(),
        Some(vec![1, 2, 3])
    );
    assert_eq!(r.remaining(), 0);

    checksum.split = None;
    checksum.installer_package = Some("com.example.installer".into());
    checksum.installer_certificate = Some(vec![9]);
    let mut p = Parcel::new();
    checksum.write_to(&mut p);
    let mut r = p.reader();
    assert_eq!(r.read_i32().unwrap(), 0xc);
    assert_eq!(r.read_i32().unwrap(), 1);
    assert_eq!(r.read_i32().unwrap(), TYPE_WHOLE_SHA256);
    assert_eq!(
        aim_service_aidl::read_byte_array(&mut r).unwrap(),
        Some(vec![1, 2, 3])
    );
    assert_eq!(
        r.read_string16().unwrap().as_deref(),
        Some("com.example.installer")
    );
    assert_eq!(
        aim_service_aidl::read_byte_array(&mut r).unwrap(),
        Some(vec![9])
    );
}

/// The whole file's digests, required, are the file's as `shasum` and
/// `md5` compute them, and come in type order.
#[test]
fn whole_file_digests_are_the_files() {
    let Some(path) = image_apk() else { return };
    let required = TYPE_WHOLE_MD5 | TYPE_WHOLE_SHA1 | TYPE_WHOLE_SHA256 | TYPE_WHOLE_SHA512;
    let found = checksums(&[apk(&path, None)], 0, required, &build());
    let kinds: Vec<i32> = found.iter().map(|c| c.checksum.kind).collect();
    assert_eq!(
        kinds,
        [
            TYPE_WHOLE_MD5,
            TYPE_WHOLE_SHA1,
            TYPE_WHOLE_SHA256,
            TYPE_WHOLE_SHA512
        ]
    );
    let tool = |program: &str, args: &[&str]| {
        let out = aim_storage::inherited::stdio_only(&mut Command::new(program))
            .args(args)
            .arg(&path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{program}");
        String::from_utf8(out.stdout).unwrap()
    };
    let shasum = |bits: &str| {
        tool("shasum", &["-a", bits])
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    assert_eq!(hex(&found[0].checksum.value), tool("md5", &["-q"]).trim());
    assert_eq!(hex(&found[1].checksum.value), shasum("1"));
    assert_eq!(hex(&found[2].checksum.value), shasum("256"));
    assert_eq!(hex(&found[3].checksum.value), shasum("512"));
    assert!(
        found
            .iter()
            .all(|c| c.split.is_none() && c.installer_package.is_none())
    );
}

/// The signer's per-1 MB content digest, readily available, is the one
/// computed over the signing block's sections when it is required.
#[test]
fn signed_content_digests_are_the_computed_ones() {
    let Some(path) = image_apk() else { return };
    let kind = TYPE_PARTIAL_MERKLE_ROOT_1M_SHA256;
    let signed = checksums(&[apk(&path, None)], kind, 0, &build());
    assert_eq!(signed.len(), 1);
    let digests = sign::chunked_content_digests(
        &FileSource::open(&path).unwrap(),
        &[sign::CONTENT_DIGEST_CHUNKED_SHA256],
    )
    .unwrap()
    .unwrap();
    assert_eq!(signed[0].checksum.value, digests[0]);
    // Required but available: taken from the signer, not computed.
    assert_eq!(checksums(&[apk(&path, None)], 0, kind, &build()), signed);
}

/// The fs-verity digest the kernel keeps is taken as is; computed, it is
/// the one aim-storage's fs-verity tree builder measures for the file.
#[test]
fn fs_verity_digests_are_the_kernels() {
    let Some(path) = image_apk() else { return };
    let kind = TYPE_WHOLE_MERKLE_ROOT_4K_SHA256;
    let kept = checksums(&[apk(&path, Some(vec![7; 32]))], kind, 0, &build());
    assert_eq!(kept[0].checksum.value, vec![7; 32]);
    assert!(checksums(&[apk(&path, None)], kind, 0, &build()).is_empty());

    let computed = checksums(&[apk(&path, None)], 0, kind, &build());
    let dir = std::env::temp_dir().join(format!("aim-apk-checksums-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tree = std::fs::File::options()
        .create_new(true)
        .read(true)
        .write(true)
        .open(dir.join("tree"))
        .unwrap();
    let data = std::fs::File::open(&path).unwrap();
    let options =
        aim_storage::fsverity::BuildOptions::new(1, 4096, Vec::new(), 16384, 4096).unwrap();
    let descriptor =
        aim_storage::fsverity::build(&data.as_fd(), &tree.as_fd(), 0, options, || false).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(computed[0].checksum.value, descriptor.digest());
}

/// A missing file has no checksum.
#[test]
fn a_missing_file_has_none() {
    let path = std::env::temp_dir().join("aim-apk-checksums-missing.apk");
    assert!(
        checksums(
            &[apk(&path, None)],
            TYPE_WHOLE_SHA256,
            TYPE_WHOLE_SHA256,
            &build()
        )
        .is_empty()
    );
}
