use super::*;

#[path = "graphics_hwui.rs"]
mod graphics_hwui;
#[path = "graphics_session.rs"]
mod graphics_session;
#[path = "graphics_state.rs"]
mod graphics_state;

pub(crate) use graphics_hwui::*;
pub(crate) use graphics_session::*;
pub(crate) use graphics_state::*;

/// Compile the backend-neutral Android graphics probe as an independent
/// translation unit.  This edge deliberately only produces an object: the
/// product graphics audit owns linking and must not be reached from the
/// developer probe rule.
pub(crate) fn compile_runtime_graphics_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    if let Some(path) = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_OBJECT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let object = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_graphics_probe.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-graphics-probe-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(ndk_arch_include)
        .arg("-idirafter")
        .arg(ndk_include)
        .arg("-DLOG_TAG=\"DarwinArtHWUI\"")
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-include")
        .arg("log/log_main.h")
        .arg("-DDARWIN_ART_REAL_GRAPHICS")
        .arg("-DDARWIN_ART_HWUI_GPU")
        .arg("-DDARWIN_ART_AOSP_COMPAT_LSEEK64")
        .arg("-c")
        .arg(root.join("probes/runtime_graphics_probe.cc"))
        .arg("-o")
        .arg(&object);
    for include in [
        "frameworks/base/libs/hwui",
        "frameworks/base/libs/hwui/hwui",
        "frameworks/base/libs/hwui/pipeline/skia",
        "frameworks/base/libs/androidfw/include",
        "frameworks/base/include",
        "frameworks/native/include",
        "system/incremental_delivery/incfs/util/include",
        "system/core/libutils/include",
        "system/core/libsystem/include",
        "system/core/libcutils/include",
        "frameworks/native/libs/ui/include",
        "frameworks/native/libs/ui/include_types",
        "frameworks/native/libs/nativewindow/include",
        "frameworks/native/libs/arect/include",
        "system/logging/liblog/include",
        "external/skia",
        "external/skia/include/core",
        "external/skia/include/effects",
        "external/skia/include/private",
        "external/skia/include/android",
        "external/skia/include/utils",
        "external/skia/include/codec",
        "frameworks/minikin/include",
        "external/harfbuzz_ng/src",
        "external/googletest/googletest/include",
    ] {
        command.arg("-I").arg(root.join("_aosp").join(include));
    }
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_graphics_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_graphics_probe.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("graphics probe output has no parent: {}", output.display()))?;
    let build_paths = BuildPaths::from_root(root);
    let runtime = root.join("_aosp/art/runtime");
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let object = compile_runtime_graphics_probe(
        root,
        build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-graphics-probe: {}", output.display());
    Ok(())
}

pub(crate) fn compile_runtime_graphics_phase(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    if let Some(path) = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_PHASE_OBJECT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let object = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_PHASE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_graphics_phase.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-graphics-phase-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("probes/runtime_graphics_phase.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_graphics_phase_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_PHASE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_graphics_phase.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("graphics phase output has no parent: {}", output.display()))?;
    let includes = [
        root.join("include"),
        root.join("compat"),
        root.join("_build/runtime-arm64/generated"),
        root.join("_build/runtime-common/patched-source/runtime"),
        root.join("_build/foundation/patched-source/libartbase"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/libdexfile"),
        root.join("_aosp/art/libelffile"),
        root.join("_aosp/art/cmdline"),
        root.join("_aosp/art/libnativebridge/include"),
        root.join("_aosp/art/runtime"),
        root.join("_aosp/art/runtime/base"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/external/tinyxml2"),
        root.join("_aosp/libnativehelper/include_jni"),
        root.join("_aosp/libnativehelper/header_only_include"),
        root.join("_aosp/libnativehelper/platform_header_only_include"),
        PathBuf::from("/opt/homebrew/include"),
    ];
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = compile_runtime_graphics_phase(root, build_dir, &include_refs)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-graphics-phase-probe: {}", output.display());
    Ok(())
}

/// Compile the Metal/HWUI RenderNode presenter separately from the
/// backend-neutral graphics probe.  This is the hot edit boundary for
/// Canvas/RenderNode work; input/state and headless presentation changes no
/// longer rebuild this private HWUI include closure.
pub(crate) fn compile_runtime_graphics_gpu_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_GPU_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_graphics_gpu.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-graphics-gpu-probe-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(ndk_arch_include)
        .arg("-idirafter")
        .arg(ndk_include)
        .arg("-DLOG_TAG=\"DarwinArtHWUI\"")
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-include")
        .arg("log/log_main.h")
        .arg("-DDARWIN_ART_REAL_GRAPHICS")
        .arg("-DDARWIN_ART_HWUI_GPU")
        .arg("-DDARWIN_ART_AOSP_COMPAT_LSEEK64")
        .arg("-c")
        .arg(root.join("probes/runtime_graphics_gpu.cc"))
        .arg("-o")
        .arg(&object);
    // Keep this compile action independent of the caller's working directory:
    // xtask invokes it from both the repository root and a Cargo target dir.
    for include in [
        "frameworks/base/libs/hwui",
        "frameworks/base/libs/hwui/hwui",
        "frameworks/base/libs/hwui/pipeline/skia",
        "frameworks/base/libs/androidfw/include",
        "frameworks/base/include",
        "frameworks/native/include",
        "system/incremental_delivery/incfs/util/include",
        "system/core/libutils/include",
        "system/core/libsystem/include",
        "system/core/libcutils/include",
        "frameworks/native/libs/ui/include",
        "frameworks/native/libs/ui/include_types",
        "frameworks/native/libs/nativewindow/include",
        "frameworks/native/libs/arect/include",
        "system/logging/liblog/include",
        "external/skia",
        "external/skia/include/core",
        "external/skia/include/effects",
        "external/skia/include/private",
        "external/skia/include/android",
        "external/skia/include/utils",
        "external/skia/include/codec",
        "frameworks/minikin/include",
        "external/harfbuzz_ng/src",
        "external/googletest/googletest/include",
    ] {
        command.arg("-I").arg(root.join("_aosp").join(include));
    }
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_graphics_gpu_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_GPU_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_graphics_gpu.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("graphics GPU output has no parent: {}", output.display()))?;
    let build_paths = BuildPaths::from_root(root);
    let runtime = root.join("_aosp/art/runtime");
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let object = compile_runtime_graphics_gpu_probe(
        root,
        build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-graphics-gpu-probe: {}", output.display());
    Ok(())
}

/// Compile the DEX/JNI ABI acceptance matrix independently from the launcher
/// and Android window/resource setup. The phase has no process-global state;
/// it receives the already-created ART/JNI handles at the call boundary.
pub(crate) fn compile_runtime_jni_acceptance_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    if let Some(path) = env::var_os("DARWIN_ART_NATIVE_JNI_ACCEPTANCE_OBJECT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    let object = env::var_os("DARWIN_ART_NATIVE_JNI_ACCEPTANCE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_jni_acceptance_probe.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-jni-acceptance-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-include")
        .arg("mirror/object_reference.h")
        .arg("-idirafter")
        .arg(ndk_arch_include)
        .arg("-idirafter")
        .arg(ndk_include)
        .arg("-c")
        .arg(root.join("probes/runtime_jni_acceptance_probe.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_jni_acceptance_probe(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_JNI_ACCEPTANCE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join("_build/runtime-link-probe/darwin_art_runtime_jni_acceptance_probe.cc.o")
        });
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("JNI acceptance output has no parent: {}", output.display()))?;
    // This command is itself the Ninja edge that rebuilds the object. The
    // environment path also serves as a reuse hint to higher-level link
    // commands, so remove the dirty edge's old output before entering the
    // shared helper; otherwise its reuse fast path would silently retain stale
    // code after an included acceptance header changed.
    if output.is_file() {
        fs::remove_file(&output)?;
    }
    let build_paths = BuildPaths::from_root(root);
    let runtime = root.join("_aosp/art/runtime");
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let object = compile_runtime_jni_acceptance_probe(root, build_dir, &include_refs)?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-jni-acceptance-probe: {}", output.display());
    Ok(())
}
