use super::*;

pub(crate) fn build_skia(root: &Path) -> Result<()> {
    const EXPECTED_OUTPUT: &str = "Skia Darwin raster: 64x64 rowBytes=256 hash=a4bb4cdb0b4779ea";
    const EXPECTED_FRAMEWORK_OUTPUT: &str =
        "Skia Android framework utils: base-canvas=same surface=same reset-clip=64x64";
    const EXPECTED_IOSURFACE_OUTPUT: &str = "darwin-skia-surface: direct-iosurface frames=120 staging-copies=0 \
         final-hash=b32235958413af5e sequence-hash=5fd4ea042fd71d4e";

    let lock = read_lock(root)?;
    let skia = root.join("_aosp/external/skia");
    if !skia.join("BUILD.gn").is_file() {
        return Err("Skia source is missing; run `art-bootstrap sync` first".into());
    }
    let source_revision = fs::read_to_string(skia.join(".source-revision"))?;
    if source_revision.trim() != lock_value(&lock, "SKIA_REVISION")? {
        return Err("materialized Skia revision does not match sources.lock".into());
    }
    let libcutils = root.join("_aosp/system/core/libcutils");
    let libcutils_include = libcutils.join("include");
    let libcutils_revision = fs::read_to_string(libcutils.join(".source-revision"))?;
    if libcutils_revision.trim() != lock_value(&lock, "SYSTEM_CORE_REVISION")? {
        return Err("materialized libcutils headers do not match sources.lock".into());
    }
    verify_sha256(
        &libcutils_include.join("cutils/trace.h"),
        lock_value(&lock, "LIBCUTILS_TRACE_HEADER_SHA256")?,
    )?;

    let gn = skia.join("bin/gn");
    if !gn.is_file() {
        run_command(
            Command::new("python3")
                .arg("bin/fetch-gn")
                .current_dir(&skia),
        )?;
    }
    verify_sha256(&gn, lock_value(&lock, "SKIA_GN_DARWIN_ARM64_SHA256")?)?;

    let ninja = skia.join("third_party/ninja/ninja");
    if !ninja.is_file() {
        run_command(
            Command::new("python3")
                .arg("bin/fetch-ninja")
                .current_dir(&skia),
        )?;
    }
    verify_sha256(&ninja, lock_value(&lock, "SKIA_NINJA_DARWIN_ARM64_SHA256")?)?;

    let build_dir = root.join("_build/skia");
    fs::create_dir_all(&build_dir)?;
    let compat = root.join("compat");
    let gn_args = format!(
        "is_official_build=true is_debug=false target_cpu=\"arm64\" \
         skia_enable_gpu=false skia_enable_graphite=false skia_enable_pdf=false \
         skia_enable_skottie=false skia_enable_svg=false skia_enable_precompile=false \
         skia_enable_tools=false skia_enable_fontmgr_empty=true skia_use_expat=false \
         skia_use_fonthost_mac=false skia_use_freetype=false skia_use_fontconfig=false \
         skia_use_harfbuzz=false skia_use_icu=false skia_use_perfetto=false \
         skia_disable_tracing=false \
         skia_use_gl=false skia_use_metal=false skia_use_vulkan=false \
         skia_use_libjpeg_turbo_decode=false skia_use_libjpeg_turbo_encode=false \
         skia_use_no_jpeg_encode=true skia_use_libpng_decode=false \
         skia_use_libpng_encode=false skia_use_no_png_encode=true \
         skia_use_libwebp_decode=false skia_use_libwebp_encode=false \
         skia_use_no_webp_encode=true skia_use_wuffs=false skia_use_piex=false \
         skia_use_xps=false skia_use_zlib=false skia_use_dng_sdk=false \
         skia_use_libheif=false skia_use_crabbyavif=false skia_use_libjxl_decode=false \
         skia_use_libavif=false skia_use_bidi=false skia_use_libgrapheme=false \
        skia_build_rust_targets=false \
         extra_cflags=[\"-I{}\",\"-I{}\",\"-DSK_BUILD_FOR_ANDROID_FRAMEWORK\"]",
        compat.display(),
        libcutils_include.display()
    );
    run_command(
        Command::new(&gn)
            .arg("gen")
            .arg(&build_dir)
            .arg(format!("--args={gn_args}"))
            .current_dir(&skia),
    )?;
    run_command(
        Command::new(&ninja)
            .arg("-C")
            .arg(&build_dir)
            .arg("skia")
            .current_dir(&skia),
    )?;

    let skia_archive = build_dir.join("libskia.a");
    let skcms_archive = build_dir.join("libskcms.a");
    for archive in [&skia_archive, &skcms_archive] {
        if !archive.is_file() {
            return Err(format!("Skia archive is missing: {}", archive.display()).into());
        }
    }

    let skia_symbols = command_output(Command::new("nm").args(["-gU"]).arg(&skia_archive))?;
    for symbol in [
        "__ZN23SkAndroidFrameworkUtils20getBaseWrappedCanvasEP8SkCanvas",
        "__ZN23SkAndroidFrameworkUtils20getSurfaceFromCanvasEP8SkCanvas",
        "__ZN23SkAndroidFrameworkUtils9ResetClipEP8SkCanvas",
    ] {
        if !skia_symbols.contains(symbol) {
            return Err(
                format!("Skia archive is missing Android framework symbol {symbol}").into(),
            );
        }
    }

    let framework_smoke = build_dir.join("skia-android-framework-smoke");
    run_command(
        Command::new("clang++")
            .args([
                "-std=c++20",
                "-O2",
                "-DNDEBUG",
                "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
            ])
            .arg(format!("-I{}", compat.display()))
            .arg(format!("-I{}", skia.display()))
            .arg(root.join("probes/skia_android_framework_smoke.cc"))
            .arg(&skia_archive)
            .arg(&skcms_archive)
            .args(["-framework", "CoreGraphics", "-framework", "CoreFoundation"])
            .arg("-o")
            .arg(&framework_smoke),
    )?;
    let framework_output = command_output(&mut Command::new(&framework_smoke))?;
    if framework_output.trim() != EXPECTED_FRAMEWORK_OUTPUT {
        return Err(
            format!("unexpected Skia Android framework output: {framework_output:?}").into(),
        );
    }

    let smoke = build_dir.join("skia-raster-smoke");
    run_command(
        Command::new("clang++")
            .args([
                "-std=c++20",
                "-O2",
                "-DNDEBUG",
                "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
            ])
            .arg(format!("-I{}", compat.display()))
            .arg(format!("-I{}", skia.display()))
            .arg(root.join("probes/skia_raster_smoke.cc"))
            .arg(&skia_archive)
            .arg(&skcms_archive)
            .args(["-framework", "CoreGraphics", "-framework", "CoreFoundation"])
            .arg("-o")
            .arg(&smoke),
    )?;
    let kind = command_output(Command::new("file").arg(&smoke))?;
    if !kind.contains("Mach-O 64-bit executable arm64") {
        return Err(format!("unexpected Skia smoke format: {kind}").into());
    }
    let output = command_output(&mut Command::new(&smoke))?;
    if output.trim() != EXPECTED_OUTPUT {
        return Err(format!("unexpected Skia raster output: {output:?}").into());
    }

    let surface_smoke = build_dir.join("darwin-skia-surface-smoke");
    run_command(
        Command::new("clang++")
            .args([
                "-std=c++20",
                "-O2",
                "-DNDEBUG",
                "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
                "-fobjc-arc",
                "-Wall",
                "-Wextra",
                "-Werror",
                // Xcode 26 promotes long-standing AppKit/IOSurface aliases
                // used by the Android compatibility bridge to errors. The
                // bridge owns these compatibility shims; keep this probe
                // buildable across SDKs while preserving all other warnings.
                "-Wno-deprecated-declarations",
            ])
            .arg(format!("-I{}", compat.display()))
            .arg(format!("-I{}", skia.display()))
            .arg(root.join("compat/darwin_surface_bridge.mm"))
            .arg(root.join("compat/filesystem/document_panel.mm"))
            .arg(root.join("compat/window/appkit_content_view.mm"))
            .arg(root.join("compat/input/darwin_hardware_key_translation.mm"))
            .arg(root.join("compat/window/desktop_root_events.mm"))
            .arg(root.join("compat/window/desktop_foreground_provider.cc"))
            .arg(root.join("compat/window/desktop_root_target.mm"))
            .arg(root.join("compat/window/desktop_root_surface.mm"))
            .arg(root.join("compat/window/appkit_window_delegate.mm"))
            .arg(root.join("compat/window/root_geometry.cc"))
            .arg(root.join("compat/window/root_geometry_host.mm"))
            .arg(root.join("compat/graphics/metal_display_backing.mm"))
            .arg(root.join("compat/graphics/surface_backing_owner.mm"))
            .arg(root.join("compat/graphics/scanout_diagnostic_capture.mm"))
            .arg("-I")
            .arg(root.join("tools/bionic-socket-broker-adapter/include"))
            .arg(root.join("compat/window/composition_fence_monitor.cc"))
            .arg(root.join("probes/darwin_skia_surface_smoke.mm"))
            .arg(&skia_archive)
            .arg(&skcms_archive)
            .args([
                "-framework",
                "AppKit",
                "-framework",
                "ApplicationServices",
                "-framework",
                "CoreGraphics",
                "-framework",
                "CoreFoundation",
                "-framework",
                "IOSurface",
                "-framework",
                "Metal",
                "-framework",
                "QuartzCore",
                "-o",
            ])
            .arg(&surface_smoke),
    )?;
    let iosurface_output =
        command_output(Command::new(&surface_smoke).env("DARWIN_ART_SURFACE_HEADLESS", "1"))?;
    if iosurface_output.trim() != EXPECTED_IOSURFACE_OUTPUT {
        return Err(
            format!("unexpected direct Skia IOSurface output: {iosurface_output:?}").into(),
        );
    }
    println!(
        "build-skia: {} {} archive_bytes={} {}",
        output.trim(),
        framework_output.trim(),
        fs::metadata(&skia_archive)?.len(),
        iosurface_output.trim()
    );
    Ok(())
}
