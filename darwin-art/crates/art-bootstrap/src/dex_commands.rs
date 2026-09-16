use super::*;

#[path = "dex_commands/baseline.rs"]
mod baseline;
#[path = "dex_commands/button.rs"]
mod button;
#[path = "dex_commands/elf_jni.rs"]
mod elf_jni;
#[path = "dex_commands/network.rs"]
mod network;

pub(crate) use baseline::build_dex_probe;
pub(crate) use button::build_button_dex_probe;
pub(crate) use elf_jni::build_elf_jni_dex_probe;
pub(crate) use network::build_network_dex_probe;

pub(crate) fn find_d8() -> Result<PathBuf> {
    let sdk_root = android_sdk_root()?;
    let build_tools = sdk_root.join("build-tools");
    let mut candidates = fs::read_dir(&build_tools)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("d8"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
        .pop()
        .ok_or_else(|| format!("d8 was not found under {}", build_tools.display()).into())
}

pub(crate) fn android_sdk_root() -> Result<PathBuf> {
    env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| env::var_os("ANDROID_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Android/sdk"))
        })
        .ok_or_else(|| "could not determine the Android SDK directory".into())
}

pub(crate) fn find_android_platform_jar() -> Result<PathBuf> {
    let jar = android_sdk_root()?.join("platforms/android-36/android.jar");
    if !jar.is_file() {
        return Err(format!("Android API 36 platform JAR is missing: {}", jar.display()).into());
    }
    Ok(jar)
}

pub(crate) fn find_android_core_system_modules() -> Result<PathBuf> {
    let jar = android_sdk_root()?.join("platforms/android-36/core-for-system-modules.jar");
    if !jar.is_file() {
        return Err(format!("Android core system modules are missing: {}", jar.display()).into());
    }
    Ok(jar)
}

// D8 is free to renumber synthetic lambda classes when an unrelated method is
// added. Treating its full diagnostic string as a golden file made every Java
// compatibility edit rebuild foundation twice merely to discover the next
// index. The stable contract is the verified DEX version/count plus named
// runtime classes that must remain packaged.
pub(crate) fn verify_dex_contract(
    output: &str,
    classes: usize,
    methods: usize,
    required_descriptors: &[&str],
) -> Result<()> {
    let header = format!("AOSP DEX: verified=yes version=38 classes={classes} methods={methods} ");
    if !output.trim().starts_with(&header) {
        return Err(format!("unexpected DEX contract header: {output:?}").into());
    }
    for descriptor in required_descriptors {
        if !output.contains(descriptor) {
            return Err(
                format!("DEX contract is missing required class {descriptor}: {output:?}").into(),
            );
        }
    }
    Ok(())
}

// The probe's diagnostic enumerates class_defs, not referenced type strings.
// These classes must have a single owner in original services.jar.
pub(crate) fn verify_service_definitions_external(output: &str) -> Result<()> {
    for name in [
        "Lcom/android/server/pm/AbstractStatsBase;",
        "Lcom/android/server/pm/AbstractStatsBase$1;",
        "Lcom/android/server/pm/dex/PackageDexUsage;",
        "Lcom/android/server/pm/dex/PackageDexUsage$DexUseInfo;",
        "Lcom/android/server/pm/dex/PackageDexUsage$PackageUseInfo;",
    ] {
        if output.contains(&format!("={name}")) {
            return Err(format!("support DEX shadows original services.jar class {name}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod service_ownership_tests {
    #[test]
    fn rejects_copied_original_but_allows_adapter() {
        assert!(
            super::verify_service_definitions_external(
                "class[2]=Lcom/android/server/pm/dex/PackageDexUsage;"
            )
            .is_err()
        );
        assert!(
            super::verify_service_definitions_external(
                "class[2]=Lcom/android/server/pm/dex/DexUsageStore;"
            )
            .is_ok()
        );
    }
}
