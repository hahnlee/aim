use super::*;

pub(crate) fn compile_runtime_graphics_input_probe(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_INPUT_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_runtime_graphics_input.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-probe-graphics-input-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("probes/runtime_graphics_input.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the Android-owned host input boundary.  Unlike the legacy fixture
/// input TU, this source only creates framework MotionEvent/KeyEvent packets
/// and submits them to ViewRootImpl's InputChannel; it has no hit-testing or
/// direct performClick path and is therefore eligible for the product graph.
pub(crate) fn compile_runtime_event_ingress(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_EVENT_INGRESS_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_event_ingress.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-event-ingress-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("runtime/framework/input/event_ingress.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

/// Compile the Android display-edge producer separately from the fixture
/// RenderNode/presentation code.  The production object only dispatches the
/// framework vsync callback and asks the Darwin HWC provider to scan out the
/// latched IOSurface.
pub(crate) fn compile_runtime_vsync_source(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
) -> Result<PathBuf> {
    let object = env::var_os("DARWIN_ART_NATIVE_VSYNC_SOURCE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| build_dir.join("darwin_art_vsync_source.cc.o"));
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = build_dir.join("runtime-vsync-source-hashes.cache");
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut command = runtime_cpp_command(includes);
    command
        .arg("-c")
        .arg(root.join("runtime/framework/display/vsync_source.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn compile_runtime_graphics_state(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    compile_runtime_graphics_state_flavor(
        root,
        build_dir,
        includes,
        ndk_include,
        ndk_arch_include,
        true,
    )
}

/// Compile the state bridge for the direct-APK/headless flavor.  That flavor
/// deliberately does not pull HWUI/RenderThread types into its ABI; it still
/// needs the same opaque GraphicsState layout for the common lifecycle code.
pub(crate) fn compile_runtime_graphics_state_cpu(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
) -> Result<PathBuf> {
    compile_runtime_graphics_state_flavor(
        root,
        build_dir,
        includes,
        ndk_include,
        ndk_arch_include,
        false,
    )
}

fn compile_runtime_graphics_state_flavor(
    root: &Path,
    build_dir: &Path,
    includes: &[&Path],
    ndk_include: &Path,
    ndk_arch_include: &Path,
    real_graphics: bool,
) -> Result<PathBuf> {
    let object = if real_graphics {
        env::var_os("DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT")
            .map(PathBuf::from)
            .unwrap_or_else(|| build_dir.join("darwin_art_graphics_state.cc.o"))
    } else {
        build_dir.join("darwin_art_graphics_state_cpu.cc.o")
    };
    if let Some(parent) = object.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache_path = if real_graphics {
        build_dir.join("runtime-probe-graphics-state-hashes.cache")
    } else {
        build_dir.join("runtime-probe-graphics-state-cpu-hashes.cache")
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
            .args([
                "-I",
                root.join("_aosp/frameworks/base/libs/hwui")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/frameworks/base/libs/hwui/hwui")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/frameworks/base/libs/hwui/pipeline/skia")
                    .to_str()
                    .unwrap(),
            ])
            .args(["-I", root.join("_aosp/external/skia").to_str().unwrap()])
            .args([
                "-I",
                root.join("_aosp/external/skia/include/core")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/system/core/libcutils/include")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/system/core/libutils/include")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/system/core/libsystem/include")
                    .to_str()
                    .unwrap(),
            ])
            .args([
                "-I",
                root.join("_aosp/system/logging/liblog/include")
                    .to_str()
                    .unwrap(),
            ]);
    }
    command
        .arg("-c")
        .arg(root.join("runtime/embedding/graphics_state.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(&mut command, &object, &cache_path, &compiler_identity)?;
    Ok(object)
}

pub(crate) fn build_runtime_graphics_state(root: &Path) -> Result<()> {
    let output = env::var_os("DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("_build/runtime-link-probe/darwin_art_graphics_state.cc.o"));
    let build_dir = output
        .parent()
        .ok_or_else(|| format!("graphics state output has no parent: {}", output.display()))?;
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
    let object = compile_runtime_graphics_state(
        root,
        build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    if object != output {
        fs::copy(&object, &output)?;
    }
    println!("build-runtime-graphics-state: {}", output.display());
    Ok(())
}
