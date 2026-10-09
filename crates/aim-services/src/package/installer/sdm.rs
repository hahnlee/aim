//! PackageInstallerSession.verifySdmSignatures at android-16.0.0_r1
//! (AOSP, Apache-2.0): full v3 verification bound to the APK's current signers.
use super::pipeline::Failure;
use crate::package::sign::{self, Apk, Build, SigningDetails};
use android_image_extract::source::FileSource;
use std::{fs, path::Path};

fn failure(status: i32, message: impl Into<String>) -> Failure {
    Failure { legacy_status: status, committed: false, message: message.into() }
}
pub(super) fn verify(stage: &Path, expected: &SigningDetails, build: &Build, overrides: Option<&sign::Overrides>) -> Result<(), Failure> {
    if expected.unknown || expected.signatures.is_empty() {
        return Err(failure(-110, "SDM verification APK signing owner unavailable"));
    }
    for entry in fs::read_dir(stage).map_err(|error| failure(-110, error.to_string()))? {
        let entry = entry.map_err(|error| failure(-110, error.to_string()))?;
        if !entry.file_name().as_encoded_bytes().ends_with(b".sdm") { continue; }
        if !entry.file_type().map_err(|error| failure(-110, error.to_string()))?.is_file() {
            return Err(failure(-2, "Failed to verify SDM signatures"));
        }
        let path = entry.path();
        let path_name = path.to_str().ok_or_else(|| failure(-2, "Failed to verify SDM signatures"))?;
        let source = FileSource::open(&path).map_err(|error| failure(-110, error.to_string()))?;
        let verified = sign::verify(&Apk { path: path_name, data: &source, v4: None }, sign::SIGNING_BLOCK_V3, true, build)
            .map_err(|_| failure(-2, "Failed to verify SDM signatures"))?;
        let verified = overrides.map_or(verified.clone(), |owner| owner.apply(&verified));
        if expected.signatures.len() != verified.signatures.len()
            || !expected.signatures.iter().all(|certificate| verified.signatures.contains(certificate))
            || !verified.signatures.iter().all(|certificate| expected.signatures.contains(certificate))
        {
            return Err(failure(-2, "SDM signatures are inconsistent with APK"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("aim-sdm-signature-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir(&path).unwrap(); Self(path)
        }
    }
    impl Drop for Data { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
    fn build() -> Build { Build { sdk_int:36, release:true, always_load_past_certs_v4:false } }
    fn inputs() -> std::path::PathBuf {
        let root = aim_paths::fetched().join("cts-tradefed/android-cts/testcases/CtsCompilationTestCases");
        for name in ["CtsCompilationTestCases.jar", "apksigner.jar", "testkey.pk8", "testkey.x509.pem", "testkey2.pk8", "testkey2.x509.pem"] {
            assert!(root.join(name).is_file(), "Pinned signing inputs required: {name}");
        }
        root
    }
    fn unsigned(root: &Path) -> Vec<u8> {
        let source = FileSource::open(&root.join("CtsCompilationTestCases.jar")).unwrap();
        let archive = android_image_extract::zip::Archive::open(&source).unwrap();
        // This verifies the signature owner only. ART parses cloud artifacts;
        // its validity is not inferred from a successful signing admission.
        archive.read(archive.find(b"CtsCompilationApp.dm").unwrap(), 32 << 20).unwrap()
    }
    fn signed(root: &Path, data: &Data, key: &str, output: &Path) {
        let input = data.0.join(format!("{key}-unsigned.zip"));
        fs::write(&input, unsigned(root)).unwrap();
        let result = std::process::Command::new("java").arg("-jar").arg(root.join("apksigner.jar"))
            .arg("sign").arg("--key").arg(root.join(format!("{key}.pk8")))
            .arg("--cert").arg(root.join(format!("{key}.x509.pem")))
            .args(["--min-sdk-version", "36", "--v1-signing-enabled", "false", "--v2-signing-enabled", "false", "--v3-signing-enabled", "true", "--v4-signing-enabled", "false"])
            .arg("--out").arg(output).arg(&input).output().unwrap();
        assert!(result.status.success(), "Pinned signing tool failed: {}", String::from_utf8_lossy(&result.stderr));
    }
    fn expected(root: &Path) -> SigningDetails {
        let source = FileSource::open(&root.join("CtsCompilationTestCases.jar")).unwrap();
        let archive = android_image_extract::zip::Archive::open(&source).unwrap();
        let bytes = archive.read(archive.find(b"StatusCheckerApp.apk").unwrap(), 32 << 20).unwrap();
        sign::verify(&Apk {path:"StatusCheckerApp.apk", data:&bytes, v4:None}, sign::SIGNING_BLOCK_V2, true, &build()).unwrap()
    }
    #[test]
    fn genuine_v3_crypto_rejects_unsigned_tampered_and_other_signer_sdm() {
        let root = inputs(); let data = Data::new(); let stage = data.0.join("stage"); fs::create_dir(&stage).unwrap();
        let expected = expected(&root);
        let sdm = stage.join("base.arm64.sdm");
        fs::write(&sdm, unsigned(&root)).unwrap();
        let error = verify(&stage, &expected, &build(), None).unwrap_err();
        assert_eq!(error.legacy_status, -2); assert_eq!(error.message, "Failed to verify SDM signatures"); assert!(!error.committed);
        signed(&root, &data, "testkey", &sdm);
        verify(&stage, &expected, &build(), None).unwrap();
        let bytes = fs::read(&sdm).unwrap(); let mut tampered = bytes.clone(); tampered[30] ^= 1;
        fs::write(&sdm, &tampered).unwrap();
        assert_eq!(verify(&stage, &expected, &build(), None).unwrap_err().message, "Failed to verify SDM signatures");
        fs::write(&sdm, &bytes).unwrap();
        // Any .sdm in the immutable stage is checked, not just an associated
        // ISA filename, as the original loop requires.
        let other = stage.join("unassociated.sdm");
        signed(&root, &data, "testkey2", &other);
        let error = verify(&stage, &expected, &build(), None).unwrap_err();
        assert_eq!(error.legacy_status, -2); assert_eq!(error.message, "SDM signatures are inconsistent with APK");
        let source = FileSource::open(&other).unwrap();
        let other_signing = sign::verify(&Apk {path:other.to_str().unwrap(), data:&source, v4:None}, sign::SIGNING_BLOCK_V3, true, &build()).unwrap();
        let overrides = sign::Overrides::new(true);
        overrides.add(other_signing, expected.clone()).unwrap();
        verify(&stage, &expected, &build(), Some(&overrides)).unwrap();
        fs::write(&other, unsigned(&root)).unwrap();
        assert_eq!(verify(&stage, &expected, &build(), Some(&overrides)).unwrap_err().message, "Failed to verify SDM signatures");
    }
    #[test]
    fn absent_trust_and_stage_owners_fail_without_admission() {
        let data = Data::new();
        let unknown = SigningDetails::unknown();
        let error = verify(&data.0, &unknown, &build(), None).unwrap_err();
        assert_eq!(error.legacy_status, -110); assert!(!error.committed);
        let root = inputs(); let expected = expected(&root);
        let signed_metadata = data.0.join("signed.zip"); signed(&root, &data, "testkey", &signed_metadata);
        let error = verify(&data.0.join("missing-stage"), &expected, &build(), None).unwrap_err();
        assert_eq!(error.legacy_status, -110); assert!(!error.committed);
        let stage = data.0.join("stage"); fs::create_dir(&stage).unwrap();
        std::os::unix::fs::symlink(&signed_metadata, stage.join("base.arm64.sdm")).unwrap();
        assert_eq!(verify(&stage, &expected, &build(), None).unwrap_err().legacy_status, -2);
    }
    #[test]
    fn real_install_pipeline_checks_sdm_before_verified_code_admission_using_selected_flag() {
        use crate::package::{installer::{native::LitePolicy, storage::Store}, parse::Platform, write::Apks};
        use aim_storage::guest_inode::GuestInode;
        let root = inputs(); let data = Data::new();
        let stage = data.0.join("app/vmdl7.tmp"); fs::create_dir_all(&stage).unwrap();
        fs::create_dir(data.0.join("system")).unwrap();
        fs::write(data.0.join("system/install_sessions.xml"), "<sessions><session sessionId='7' userId='0' installerUid='10100' createdMillis='1' mode='1' installFlags='16' installLocation='1' sizeBytes='-1' installRason='0' packageSource='0' prepared='true' sealed='true' sessionStageDir='/data/app/vmdl7.tmp'/></sessions>").unwrap();
        let inode = GuestInode {uid:Some(1000),gid:Some(1000),mode:Some(0o600)};
        let store = Store::open(data.0.clone(), inode, GuestInode {mode:Some(0o775),..inode}, std::sync::Arc::new(|_,_|panic!("read-only recovered staging fixture must not create or relabel files"))).unwrap();
        let records = store.recovered().unwrap(); let (session,record) = &records[0];
        let source = FileSource::open(&root.join("CtsCompilationTestCases.jar")).unwrap();
        let archive = android_image_extract::zip::Archive::open(&source).unwrap();
        let apk_bytes = archive.read(archive.find(b"StatusCheckerApp.apk").unwrap(), 32 << 20).unwrap();
        fs::write(stage.join("base.apk"), &apk_bytes).unwrap();
        let image = aim_paths::original_image_with("system/framework/framework-res.apk").expect("Pinned original image required");
        let _lease = aim_storage::system::ImageLease::read_root(&image).unwrap();
        let platform = Platform::load(&image, Default::default()).unwrap();
        let environment = crate::package::parse::lite::Environment {sdk:platform.sdk,codenames:platform.codenames.clone(),properties:Default::default()};
        let files = data.0.clone();
        let apks = Apks {signing_overrides:None, platform, files:Box::new(move |guest|guest.strip_prefix("/data/").map(|path|files.join(path)))};
        let sdm = stage.join("base.arm64.sdm"); fs::write(&sdm, unsigned(&root)).unwrap();
        for enabled in [false,true] {
            let policy = LitePolicy {environment:crate::package::parse::lite::Environment {sdk:environment.sdk,codenames:environment.codenames.clone(),properties:environment.properties.clone()}, art_managed_extensions:vec![".dm".into(),".prof".into(),".sdm".into()], cloud_compilation_verification:enabled};
            let result = super::super::pipeline::verify_batch(&apks, vec![(session.clone(),record.clone(),"/data/app/vmdl7.tmp".into())], &policy);
            if enabled {
                let error = result.err().expect("unsigned SDM must not become VerifiedCode");
                assert_eq!(error.legacy_status,-2); assert_eq!(error.message,"Failed to verify SDM signatures"); assert!(!error.committed);
            } else { assert_eq!(result.unwrap().len(),1); }
        }
        signed(&root,&data,"testkey",&sdm);
        let policy = LitePolicy {environment, art_managed_extensions:vec![".sdm".into()],cloud_compilation_verification:true};
        assert_eq!(super::super::pipeline::verify_batch(&apks,vec![(session.clone(),record.clone(),"/data/app/vmdl7.tmp".into())],&policy).unwrap().len(),1);
        assert_eq!(fs::read(stage.join("base.apk")).unwrap(),apk_bytes);
    }
}
