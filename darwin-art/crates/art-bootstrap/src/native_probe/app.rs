use super::*;

pub(crate) fn compile_runtime_network_loader_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = build_dir.join("darwin_art_runtime_network_loader.cc.o");
    let cache_path = build_dir.join("runtime-probe-network-loader-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("probes/runtime_network_loader.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the JNI-only context-loader bridge separately from the managed
/// Activity entry probe.  It has no ART/HWUI dependency beyond JNI headers.
pub(crate) fn compile_system_class_loader(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = build_dir.join("darwin_art_system_class_loader.cc.o");
    let cache_path = build_dir.join("runtime-probe-context-loader-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("runtime/art/system_class_loader.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the managed DEX/class-loader bootstrap separately from the large
/// runtime orchestration entry point. Changes to app class discovery or the
/// direct-APK load transaction now invalidate this TU only.
pub(crate) fn compile_runtime_app_bootstrap_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_APP_BOOTSTRAP_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_app_bootstrap.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-app-bootstrap-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut command = runtime_cpp_command(includes);
    command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-c")
        .arg(root.join("probes/runtime_app_bootstrap.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the framework-resource construction phase separately from the
/// Activity/PhoneWindow presentation phase. Resource/APK asset edits now
/// invalidate this narrow object instead of the full presentation TU.
pub(crate) fn app_resources_object_path(build_dir: &Path) -> PathBuf {
    env::var_os("DARWIN_ART_NATIVE_APP_RESOURCES_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_app_resources.cc.o"))
}

pub(crate) fn compile_runtime_app_resources_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = app_resources_object_path(build_dir);
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-app-resources-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut command = runtime_cpp_command(includes);
    command
        .args([
            "-include",
            "mirror/object_reference.h",
            "-DDARWIN_ART_REAL_GRAPHICS",
            "-DDARWIN_ART_HWUI_GPU",
        ])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-c")
        .arg(root.join("probes/runtime_app_resources.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the Activity.attach/PhoneWindow/theme phase independently from
/// resource construction and display-list presentation.  This keeps changes
/// to lifecycle wiring from invalidating either the asset or GPU recording
/// translation units.
pub(crate) fn app_activity_object_path(build_dir: &Path) -> PathBuf {
    env::var_os("DARWIN_ART_NATIVE_APP_ACTIVITY_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_app_activity.cc.o"))
}

pub(crate) fn compile_runtime_app_activity_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = app_activity_object_path(build_dir);
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-app-activity-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut command = runtime_cpp_command(includes);
    command
        .args([
            "-include",
            "mirror/object_reference.h",
            "-DDARWIN_ART_REAL_GRAPHICS",
            "-DDARWIN_ART_HWUI_GPU",
        ])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-c")
        .arg(root.join("probes/runtime_app_activity.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the detached Activity/PhoneWindow/DecorView presentation phase as
/// its own cacheable translation unit. This keeps framework/JNI presentation
/// edits from rebuilding the runtime entry orchestration and bootstrap.
pub(crate) fn compile_runtime_app_presentation_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_APP_PRESENTATION_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_app_presentation.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-app-presentation-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut command = runtime_cpp_command(includes);
    command
        .args([
            "-include",
            "mirror/object_reference.h",
            "-DDARWIN_ART_REAL_GRAPHICS",
            "-DDARWIN_ART_HWUI_GPU",
        ])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-c")
        .arg(root.join("probes/runtime_app_presentation.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Include closure shared by the two detached application phases. Keeping
/// this in the probe builder means the Ninja edges can invoke a narrow Cargo
/// command without reproducing ART's include/define construction in xtask.
fn app_probe_include_paths(root: &Path) -> Vec<PathBuf> {
    let runtime = root.join("_aosp/art/runtime");
    vec![
        root.join("include"),
        root.join("compat"),
        root.join("_build/runtime-arm64/generated"),
        root.join("_build/runtime-common/patched-source/runtime"),
        root.join("_build/foundation/patched-source/libartbase"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/cmdline"),
        root.join("_aosp/art/libdexfile"),
        root.join("_aosp/art/libelffile"),
        root.join("_aosp/art/libprofile"),
        root.join("_aosp/art/libnativebridge/include"),
        runtime.clone(),
        runtime.join("base"),
        runtime.join("arch/arm64"),
        root.join("_aosp/art/libartpalette/include"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/external/tinyxml2"),
        root.join("_aosp/libnativehelper/include_jni"),
        root.join("_aosp/libnativehelper/header_only_include"),
        root.join("_aosp/libnativehelper/platform_header_only_include"),
        root.join("_aosp/external/dlmalloc"),
        root.join("tools/bionic-dns-facade/include"),
        root.join("tools/bionic-fs-facade/include"),
        root.join("tools/bionic-ioctl-facade/include"),
        root.join("tools/bionic-socket-broker-adapter/include"),
        PathBuf::from("/opt/homebrew/include"),
    ]
}

fn build_detached_app_probe(root: &Path, presentation: bool) -> Result<()> {
    let output_name = if presentation {
        "darwin_art_runtime_app_presentation.cc.o"
    } else {
        "darwin_art_runtime_app_bootstrap.cc.o"
    };
    let output = if presentation {
        env::var_os("DARWIN_ART_NATIVE_APP_PRESENTATION_OBJECT")
    } else {
        env::var_os("DARWIN_ART_NATIVE_APP_BOOTSTRAP_OBJECT")
    }
    .map(PathBuf::from)
    .unwrap_or_else(|| root.join("_build/runtime-link-probe").join(output_name));
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("app probe output has no parent: {}", output.display()))?;
    let includes = app_probe_include_paths(root);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = if presentation {
        compile_runtime_app_resources_probe(root, build_dir, &include_refs)?;
        compile_runtime_app_activity_probe(root, build_dir, &include_refs)?;
        compile_runtime_app_presentation_probe(root, build_dir, &include_refs)?
    } else {
        compile_runtime_app_bootstrap_probe(root, build_dir, &include_refs)?
    };
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!(
        "build-runtime-app-{}-probe: {}",
        if presentation {
            "presentation"
        } else {
            "bootstrap"
        },
        output.display()
    );
    Ok(())
}

pub(crate) fn build_runtime_app_bootstrap_probe(root: &Path) -> Result<()> {
    build_detached_app_probe(root, false)
}

pub(crate) fn build_runtime_app_presentation_probe(root: &Path) -> Result<()> {
    build_detached_app_probe(root, true)
}

pub(crate) fn build_runtime_app_resources_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_APP_RESOURCES_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_app_resources.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("app resources output has no parent: {}", output.display()))?;
    let includes = app_probe_include_paths(root);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = compile_runtime_app_resources_probe(root, build_dir, &include_refs)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-app-resources-probe: {}", output.display());
    Ok(())
}

pub(crate) fn build_runtime_app_activity_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_APP_ACTIVITY_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_app_activity.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("app activity output has no parent: {}", output.display()))?;
    let includes = app_probe_include_paths(root);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = compile_runtime_app_activity_probe(root, build_dir, &include_refs)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-app-activity-probe: {}", output.display());
    Ok(())
}
