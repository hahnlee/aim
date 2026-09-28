use super::*;

pub(crate) fn prepare_icu_bootclasspath(root: &Path) -> Result<()> {
    let source_lock = read_lock(root)?;
    let source = root.join("_prebuilt/android-16/bootclasspath/core-icu4j.jar");
    if !source.is_file() {
        return Err(format!(
            "{} is missing; run `art-bootstrap sync` first",
            source.display()
        )
        .into());
    }
    verify_sha256(&source, lock_value(&source_lock, "CORE_ICU4J_SHA256")?)?;

    let build_dir = root.join("_build/bootclasspath/core-icu4j");
    let output = root.join("_build/bootclasspath/core-icu4j.jar");
    fs::create_dir_all(&build_dir)?;
    if !output.is_file() {
        run_command(
            Command::new(find_d8()?)
                .args(["--min-api", "36", "--output"])
                .arg(&output)
                .arg(&source),
        )?;
    }
    run_command(
        Command::new("unzip")
            .args(["-o", "-q"])
            .arg(&output)
            .arg("classes.dex")
            .arg("-d")
            .arg(&build_dir),
    )?;
    let probe = root.join("_build/dex-probe/dex-probe");
    if !probe.is_file() {
        return Err("DEX probe is missing; run `build-dex` first".into());
    }
    let summary = command_output(
        Command::new(&probe)
            .arg("--summary")
            .arg(build_dir.join("classes.dex")),
    )?;
    let expected = "AOSP DEX: verified=yes version=39 classes=1596 methods=14990 \
                    class[0]=Landroid/icu/impl/Assert;";
    if summary.trim() != expected {
        return Err(format!("unexpected core-icu4j DEX summary: {summary:?}").into());
    }
    Ok(())
}

pub(crate) fn materialize_api36_core_icu4j(root: &Path) -> Result<PathBuf> {
    if let Some(value) = env::var_os("AIM_CORE_ICU4J_JAR") {
        let source = PathBuf::from(value);
        if source.is_file() {
            return Ok(source);
        }
        return Err(format!(
            "AIM_CORE_ICU4J_JAR does not name a file: {}",
            source.display()
        )
        .into());
    }

    let prebuilt = root.join("_prebuilt/android-16/i18n/core-icu4j-api36.jar");
    if prebuilt.is_file() {
        return Ok(prebuilt);
    }

    let sdk = android_sdk_root()?;
    let default_images = [
        sdk.join("system-images/android-36/google_apis_playstore/arm64-v8a/system.img"),
        sdk.join("system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img"),
    ];
    let system_image = if let Some(value) = env::var_os("AIM_ANDROID16_SYSTEM_IMAGE") {
        let path = PathBuf::from(value);
        if !path.is_file() {
            return Err(format!(
                "AIM_ANDROID16_SYSTEM_IMAGE does not name a file: {}",
                path.display()
            )
            .into());
        }
        path
    } else {
        default_images
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| {
                format!(
                    "Android 16 ICU76 runtime JAR is missing. Set \
                     AIM_CORE_ICU4J_JAR or install an API 36 ARM64 Play Store system \
                     image under {}",
                    sdk.join("system-images").display()
                )
            })?
    };

    let build_dir = root.join("_build/bootclasspath/api36-i18n-extract");
    let apex = build_dir.join("com.android.i18n.apex");
    let output = build_dir.join("core-icu4j.jar");
    fs::create_dir_all(&build_dir)?;
    if !apex.is_file() {
        run_command(
            Command::new("cargo")
                .args(["run", "--quiet", "--release", "--manifest-path"])
                .arg(root.join("tools/super-i18n-apex-extract/Cargo.toml"))
                .arg("--")
                .arg(&system_image)
                .arg(&apex),
        )?;
    }
    if !output.is_file() {
        run_command(
            Command::new("cargo")
                .args(["run", "--quiet", "--release", "--manifest-path"])
                .arg(root.join("tools/apex-ext2-extract/Cargo.toml"))
                .arg("--")
                .arg(&apex)
                .arg(&output),
        )?;
    }
    Ok(output)
}

pub(crate) fn prepare_icu_runtime_bootclasspath(root: &Path) -> Result<PathBuf> {
    let source = materialize_api36_core_icu4j(root)?;
    run_command(
        Command::new("bash")
            .arg(root.join("tools/audit-android16-core-icu4j-runtime.sh"))
            .arg(&source),
    )?;
    let output = root.join("_build/bootclasspath/core-icu4j-api36.jar");
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    if source != output {
        fs::copy(source, &output)?;
    }
    Ok(output)
}
