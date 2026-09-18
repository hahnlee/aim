use super::*;

pub(crate) struct PrivateApkNativeFixture {
    pub(crate) temporary_root: PathBuf,
    pub(crate) extracted_root: PathBuf,
    pub(crate) apk_sha256: String,
    pub(crate) root_sha256: String,
}

impl Drop for PrivateApkNativeFixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.extracted_root, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.temporary_root);
    }
}

pub(crate) struct PrivateNetworkFixture {
    pub(crate) root: PathBuf,
    pub(crate) library: PathBuf,
}

impl Drop for PrivateNetworkFixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn prepare_private_network_fixture(root: &Path) -> Result<PrivateNetworkFixture> {
    let source = root.join("_build/network-runtime-probe/libdarwin_art_network_runtime.so");
    if !source.is_file() {
        return Err(format!("network runtime fixture is missing: {}", source.display()).into());
    }
    let temporary_base = env::temp_dir();
    let mut private_root = None;
    for attempt in 0..128_u32 {
        let candidate = temporary_base.join(format!(
            "darwin-art-network-runtime.{}.{}",
            std::process::id(),
            attempt
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                private_root = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let private_root = private_root.ok_or("could not allocate private network fixture")?;
    let library = private_root.join("libdarwin_art_network_runtime.so");
    fs::copy(&source, &library)?;
    fs::set_permissions(&library, fs::Permissions::from_mode(0o400))?;
    fs::set_permissions(&private_root, fs::Permissions::from_mode(0o500))?;
    Ok(PrivateNetworkFixture {
        root: private_root,
        library,
    })
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn prepare_private_apk_native_fixture(root: &Path) -> Result<PrivateApkNativeFixture> {
    let fixture_root = root.join("_build/android-elf-jni-fixture");
    let root_soname = "libdarwin-art-generic-root.so";
    let sonames = [
        root_soname,
        "libdarwin-art-generic-child.so",
        "libdarwin-art-generic-grandchild.so",
        "libdarwin-art-jni-fixture.so",
        "libdarwin-art-jni-child.so",
        "libdarwin-art-jni-grandchild.so",
        "libdarwin-art-jni-host.so",
    ];
    let temporary_base = env::temp_dir();
    let mut temporary_root = None;
    for attempt in 0..128_u32 {
        let candidate = temporary_base.join(format!(
            "darwin-art-apk-native-runtime.{}.{}",
            std::process::id(),
            attempt
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700))?;
                temporary_root = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let temporary_root =
        temporary_root.ok_or("could not allocate private APK native fixture directory")?;
    let mut cleanup = PrivateApkNativeFixture {
        extracted_root: temporary_root.join("extracted"),
        temporary_root,
        apk_sha256: String::new(),
        root_sha256: String::new(),
    };
    let apk_source = cleanup.temporary_root.join("apk/lib/arm64-v8a");
    fs::create_dir_all(&apk_source)?;
    for soname in sonames {
        let source = fixture_root.join(soname);
        if !source.is_file() {
            return Err(format!("APK native fixture is missing: {}", source.display()).into());
        }
        fs::copy(source, apk_source.join(soname))?;
    }
    let apk = cleanup.temporary_root.join("fixture.apk");
    run_command(
        Command::new("zip")
            .current_dir(cleanup.temporary_root.join("apk"))
            .args(["-q", "-9", "-X", "-r"])
            .arg(&apk)
            .arg("."),
    )?;
    cleanup.apk_sha256 = sha256_file(&apk)?;

    let extractor = root.join("tools/android-apk-native-extract/Cargo.toml");
    if !extractor.is_file() {
        return Err(format!("APK native extractor is missing: {}", extractor.display()).into());
    }
    let extraction_output = command_output(
        Command::new("cargo")
            .args(["run", "--quiet", "--release", "--manifest-path"])
            .arg(&extractor)
            .arg("--")
            .arg(&apk)
            .arg(&cleanup.extracted_root)
            .arg(root_soname),
    )?;
    if !extraction_output.starts_with("apk-native-extract: PASS files=7 stored=0 deflated=7 ")
        || !extraction_output.contains(
            "crc=verified mode=dir0500+file0400 publish=atomic root=libdarwin-art-generic-root.so",
        )
    {
        return Err(
            format!("unexpected APK native extraction output: {extraction_output:?}").into(),
        );
    }
    if fs::metadata(&cleanup.extracted_root)?.mode() & 0o777 != 0o500 {
        return Err("APK native extraction directory is not sealed to mode 0500".into());
    }
    for soname in sonames {
        let original = fixture_root.join(soname);
        let extracted = cleanup.extracted_root.join(soname);
        if fs::metadata(&extracted)?.mode() & 0o777 != 0o400 {
            return Err(format!("extracted native fixture is not mode 0400: {soname}").into());
        }
        let original_sha256 = sha256_file(&original)?;
        let extracted_sha256 = sha256_file(&extracted)?;
        if original_sha256 != extracted_sha256 {
            return Err(format!("APK extraction changed native fixture bytes: {soname}").into());
        }
        if soname == root_soname {
            cleanup.root_sha256 = extracted_sha256;
        }
    }
    Ok(cleanup)
}
