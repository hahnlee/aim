use super::*;

pub(crate) fn verify_bootclasspath(root: &Path) -> Result<()> {
    let prebuilt = root.join("_prebuilt/android-16/bootclasspath");
    let probe = root.join("_build/dex-probe/dex-probe");
    let manifest = read_key_value_file(&root.join("bootclasspath.lock"))?;
    if !probe.exists() {
        return Err("DEX probe is missing; run `build-dex` first".into());
    }

    let entries = [
        (
            "core-oj",
            "CORE_OJ_SHA256",
            "CORE_OJ_SIZE",
            "AOSP DEX: verified=yes version=39 classes=4188 methods=41526 \
             class[0]=Ljava/lang/Object;",
        ),
        (
            "core-libart",
            "CORE_LIBART_SHA256",
            "CORE_LIBART_SIZE",
            "AOSP DEX: verified=yes version=39 classes=543 methods=5453 \
             class[0]=Landroid/compat/Compatibility$BehaviorChangeDelegate;",
        ),
    ];
    for (name, sha_key, size_key, expected) in entries {
        let jar = prebuilt.join(format!("{name}.jar"));
        if !jar.exists() {
            return Err(format!(
                "{} is missing; extract matching Android 16 boot JARs first",
                jar.display()
            )
            .into());
        }
        verify_sha256(&jar, lock_value(&manifest, sha_key)?)?;
        let expected_size: u64 = lock_value(&manifest, size_key)?.parse()?;
        let actual_size = fs::metadata(&jar)?.len();
        if actual_size != expected_size {
            return Err(format!(
                "size mismatch for {}: expected {expected_size}, found {actual_size}",
                jar.display()
            )
            .into());
        }
        let extract_dir = root.join(format!("_build/bootclasspath/{name}"));
        fs::create_dir_all(&extract_dir)?;
        run_command(
            Command::new("unzip")
                .args(["-o", "-q"])
                .arg(&jar)
                .arg("classes.dex")
                .arg("-d")
                .arg(&extract_dir),
        )?;
        let output = command_output(
            Command::new(&probe)
                .arg("--summary")
                .arg(extract_dir.join("classes.dex")),
        )?;
        if output.trim() != expected {
            return Err(format!("unexpected {name} DEX summary: {output:?}").into());
        }
        println!("verify-bootclasspath: {name} {}", output.trim());
    }
    let framework = prebuilt.join("framework.jar");
    if !framework.exists() {
        return Err(format!(
            "{} is missing; pull /system/framework/framework.jar from the matching Android 16 image",
            framework.display()
        )
        .into());
    }
    verify_sha256(&framework, lock_value(&manifest, "FRAMEWORK_SHA256")?)?;
    let expected_framework_size: u64 = lock_value(&manifest, "FRAMEWORK_SIZE")?.parse()?;
    if fs::metadata(&framework)?.len() != expected_framework_size {
        return Err(format!("size mismatch for {}", framework.display()).into());
    }
    let framework_summaries = [
        (
            "classes.dex",
            "AOSP DEX: verified=yes version=39 classes=6609 methods=65389 \
             class[0]=Landroid/Manifest$permission;",
        ),
        (
            "classes2.dex",
            "AOSP DEX: verified=yes version=39 classes=7041 methods=65454 \
             class[0]=Landroid/hardware/camera2/impl/CallbackProxies$SessionStateCallbackProxy$$ExternalSyntheticLambda0;",
        ),
        (
            "classes3.dex",
            "AOSP DEX: verified=yes version=39 classes=8736 methods=65454 \
             class[0]=Landroid/os/IInterface;",
        ),
        (
            "classes4.dex",
            "AOSP DEX: verified=yes version=39 classes=6855 methods=65167 \
             class[0]=Lcom/android/ims/internal/IImsServiceController;",
        ),
        (
            "classes5.dex",
            "AOSP DEX: verified=yes version=39 classes=6478 methods=56051 \
             class[0]=Lcom/android/internal/hidden_from_bootclasspath/android/app/admin/flags/CustomFeatureFlags$$ExternalSyntheticLambda0;",
        ),
    ];
    let extract_dir = root.join("_build/bootclasspath/framework");
    fs::create_dir_all(&extract_dir)?;
    for (dex_name, expected) in framework_summaries {
        run_command(
            Command::new("unzip")
                .args(["-o", "-q"])
                .arg(&framework)
                .arg(dex_name)
                .arg("-d")
                .arg(&extract_dir),
        )?;
        let output = command_output(
            Command::new(&probe)
                .arg("--summary")
                .arg(extract_dir.join(dex_name)),
        )?;
        if output.trim() != expected {
            return Err(format!("unexpected framework {dex_name} DEX summary: {output:?}").into());
        }
        println!(
            "verify-bootclasspath: framework/{dex_name} {}",
            output.trim()
        );
    }
    let framework_location = prebuilt.join("framework-location.jar");
    if !framework_location.exists() {
        return Err(format!(
            "{} is missing; pull /system/framework/framework-location.jar from the matching Android 16 image",
            framework_location.display()
        )
        .into());
    }
    verify_sha256(
        &framework_location,
        lock_value(&manifest, "FRAMEWORK_LOCATION_SHA256")?,
    )?;
    let expected_location_size: u64 = lock_value(&manifest, "FRAMEWORK_LOCATION_SIZE")?.parse()?;
    if fs::metadata(&framework_location)?.len() != expected_location_size {
        return Err(format!("size mismatch for {}", framework_location.display()).into());
    }
    let location_extract_dir = root.join("_build/bootclasspath/framework-location");
    fs::create_dir_all(&location_extract_dir)?;
    run_command(
        Command::new("unzip")
            .args(["-o", "-q"])
            .arg(&framework_location)
            .arg("classes.dex")
            .arg("-d")
            .arg(&location_extract_dir),
    )?;
    let location_output = command_output(
        Command::new(&probe)
            .arg("--summary")
            .arg(location_extract_dir.join("classes.dex")),
    )?;
    let expected_location = "AOSP DEX: verified=yes version=39 classes=493 methods=4411 \
             class[0]=Landroid/location/Address$1;";
    if location_output.trim() != expected_location {
        return Err(
            format!("unexpected framework-location DEX summary: {location_output:?}").into(),
        );
    }
    println!(
        "verify-bootclasspath: framework-location {}",
        location_output.trim()
    );
    let icu_source = prebuilt.join("core-icu4j.jar");
    let expected_icu_size: u64 = lock_value(&manifest, "CORE_ICU4J_SOURCE_SIZE")?.parse()?;
    if fs::metadata(&icu_source)?.len() != expected_icu_size {
        return Err(format!("size mismatch for {}", icu_source.display()).into());
    }
    prepare_icu_bootclasspath(root)?;
    println!(
        "verify-bootclasspath: core-icu4j AOSP DEX: verified=yes version=39 \
         classes=1596 methods=14990 class[0]=Landroid/icu/impl/Assert;"
    );
    Ok(())
}
