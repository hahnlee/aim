use super::*;

pub(crate) fn compile_runtime_graphics_session(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    compile_runtime_graphics_session_flavor(
        root,
        build_dir,
        includes,
        ndk_include,
        ndk_arch_include,
        true,
    )
}

pub(crate) fn compile_runtime_graphics_session_cpu(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    compile_runtime_graphics_session_flavor(
        root,
        build_dir,
        includes,
        ndk_include,
        ndk_arch_include,
        false,
    )
}

fn compile_runtime_graphics_session_flavor(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
    real_graphics: bool,
) -> Result<PathBuf> {
    let object = if real_graphics {
        env::var_os("DARWIN_ART_NATIVE_GRAPHICS_SESSION_OBJECT")
            .map(PathBuf::from)
            .unwrap_or_else(|| build_dir.join("darwin_art_graphics_session.cc.o"))
    } else {
        build_dir.join("darwin_art_graphics_session_cpu.cc.o")
    };
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = if real_graphics {
        build_dir.join("runtime-graphics-session-hashes.cache")
    } else {
        build_dir.join("runtime-graphics-session-cpu-hashes.cache")
    };
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(ndk_arch_include)
        .arg("-idirafter")
        .arg(ndk_include)
        .args(["-Wno-macro-redefined", "-DDARWIN_ART_AOSP_COMPAT_LSEEK64"]);
    if real_graphics {
        command
            .args([
                "-DDARWIN_ART_REAL_GRAPHICS",
                "-DDARWIN_ART_HWUI_GPU",
                "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
            ])
            .arg("-I")
            .arg(root.join("_aosp/frameworks/base/libs/hwui"))
            .arg("-I")
            .arg(root.join("_aosp/frameworks/base/libs/hwui/hwui"))
            .arg("-I")
            .arg(root.join("_aosp/frameworks/base/libs/hwui/pipeline/skia"));
    }
    command
        .arg("-I")
        .arg(root.join("_aosp/art/libdexfile"))
        .arg("-I")
        .arg(root.join("_aosp/art/libelffile"))
        .arg("-I")
        .arg(root.join("_aosp/art/libprofile"))
        .arg("-I")
        .arg(root.join("_aosp/art/libnativebridge/include"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/external/tinyxml2"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libcutils/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libutils/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libsystem/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/logging/liblog/include"))
        .arg("-c")
        .arg(root.join("runtime/embedding/graphics_session.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the small JavaVMExt network-loader boundary separately from the
/// large managed Activity entry probe.  Network fixture changes now
/// invalidate this TU only instead of recompiling the managed entry point.
pub(crate) fn build_runtime_graphics_session(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_SESSION_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("_build/runtime-link-probe/darwin_art_graphics_session.cc.o"));
    let build_dir = output.parent().ok_or_else(|| {
        format!(
            "graphics session output has no parent: {}",
            output.display()
        )
    })?;
    let includes = [
        root.join("include"),
        root.join("compat"),
        root.join("_build/runtime-arm64/generated"),
        root.join("_build/runtime-common/patched-source/runtime"),
        root.join("_build/foundation/patched-source/libartbase"),
        root.join("_aosp/art/runtime"),
        root.join("_aosp/art/runtime/base"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/libnativehelper/include_jni"),
        root.join("_aosp/libnativehelper/header_only_include"),
        root.join("_aosp/libnativehelper/platform_header_only_include"),
        root.join("_aosp/external/skia"),
        PathBuf::from("/opt/homebrew/include"),
    ];
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let object = compile_runtime_graphics_session(
        root,
        build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-graphics-session: {}", output.display());
    Ok(())
}
