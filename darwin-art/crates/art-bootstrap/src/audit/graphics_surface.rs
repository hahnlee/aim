use super::*;

pub(super) fn compile_surface_objects(
    root: &Path,
    build_dir: &Path,
    probe_cache: &Path,
    compiler_identity: &str,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let surface_object = build_dir.join("darwin_surface_bridge.mm.o");
    let mut surface_command = Command::new("clang++");
    surface_command
        .args(["-std=c++20", "-fobjc-arc", "-Wall", "-Wextra", "-c"])
        .arg(root.join("compat/darwin_surface_bridge.mm"))
        .arg("-I")
        .arg(root.join("compat"))
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/system/logging/liblog/include"))
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-DSK_USER_CONFIG_HEADER=\"include/config/SkUserConfigManual.h\"")
        .arg("-o")
        .arg(&surface_object);
    let _ = compile_cached_probe_tu(
        &mut surface_command,
        &surface_object,
        probe_cache,
        compiler_identity,
    )?;

    let surface_gpu_object = build_dir.join("darwin_surface_gpu_bridge.mm.o");
    let mut surface_gpu_command = Command::new("clang++");
    surface_gpu_command
        .args([
            "-std=c++20",
            "-fobjc-arc",
            "-Wall",
            "-Wextra",
            "-Wno-macro-redefined",
            "-c",
        ])
        .arg(root.join("compat/darwin_surface_gpu_bridge.mm"))
        .arg("-I")
        .arg(root.join("compat"))
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/gpu"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/effects"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/utils"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/private"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/android"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/codec"))
        .arg("-I")
        .arg(root.join("_aosp/system/logging/liblog/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libcutils/include"))
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-DSK_USER_CONFIG_HEADER=\"include/config/SkUserConfigManual.h\"")
        .arg("-o")
        .arg(&surface_gpu_object);
    let _ = compile_cached_probe_tu(
        &mut surface_gpu_command,
        &surface_gpu_object,
        probe_cache,
        compiler_identity,
    )?;
    let document_panel_object = build_dir.join("document_panel.mm.o");
    let mut document_panel_command = Command::new("clang++");
    document_panel_command
        .args(["-std=c++20", "-fobjc-arc", "-Wall", "-Wextra", "-c"])
        .arg(root.join("compat/filesystem/document_panel.mm"))
        .arg("-I")
        .arg(root.join("compat"))
        .arg("-I")
        .arg(root.join("include"))
        .arg("-o")
        .arg(&document_panel_object);
    let _ = compile_cached_probe_tu(
        &mut document_panel_command,
        &document_panel_object,
        probe_cache,
        compiler_identity,
    )?;
    Ok((surface_object, surface_gpu_object, document_panel_object))
}
