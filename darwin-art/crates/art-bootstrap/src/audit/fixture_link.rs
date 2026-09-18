//! Test-client -> product linkage. No ART or platform implementation archive
//! is allowed in this recipe: the product image owns every runtime singleton.
use super::common::require_file;
use super::*;
use crate::build_context::BuildPaths;

pub(crate) fn build_runtime_fixture_client(root: &Path, graphics: bool) -> Result<()> {
    let paths = BuildPaths::from_root(root);
    let product_dir = paths.native_output(if graphics {
        "runtime-graphics-link-probe"
    } else {
        "runtime-link-probe"
    });
    let product = product_dir.join(if graphics {
        "libdarwin_art_runtime_graphics.dylib"
    } else {
        "libdarwin_art_runtime.dylib"
    });
    require_file(&product, "build the matching product runtime first")?;
    let output_dir = paths.native_output(if graphics {
        "native-fixtures/graphics"
    } else {
        "native-fixtures/headless"
    });
    fs::create_dir_all(&output_dir)?;
    let runtime = root.join("_aosp/art/runtime");
    let mut includes = super::graphics_core_probes::core_probe_includes(root, &paths, &runtime);
    for path in [
        "_aosp/art/test",
        "_aosp/art/compiler",
        "_aosp/art/libprofile",
        "_aosp/art/openjdkjvmti",
        "_aosp/art/libartpalette/include",
        "_aosp/frameworks/base/libs/hwui",
        "_aosp/frameworks/base/libs/hwui/hwui",
        "_aosp/frameworks/base/libs/hwui/pipeline/skia",
        "_aosp/frameworks/base/libs/androidfw/include",
        "_aosp/frameworks/base/include",
        "_aosp/frameworks/native/include",
        "_aosp/frameworks/native/libs/ui/include",
        "_aosp/frameworks/native/libs/ui/include_types",
        "_aosp/frameworks/native/libs/nativewindow/include",
        "_aosp/frameworks/native/libs/arect/include",
        "_aosp/system/core/libutils/include",
        "_aosp/system/core/libsystem/include",
        "_aosp/system/core/libcutils/include",
        "_aosp/system/incremental_delivery/incfs/util/include",
        "_aosp/system/logging/liblog/include",
        "_aosp/external/skia",
        "_aosp/external/skia/include/core",
        "_aosp/external/skia/include/effects",
        "_aosp/external/skia/include/private",
        "_aosp/external/skia/include/utils",
        "_aosp/external/skia/include/android",
        "_aosp/external/skia/include/codec",
        "_aosp/frameworks/minikin/include",
        "_aosp/external/harfbuzz_ng/src",
        "_aosp/external/freetype/include",
        "_aosp/libnativehelper-full/include",
    ] {
        includes.push(root.join(path));
    }
    let refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk, ndk_arch) = find_ndk_headers()?;
    let identity = command_output(Command::new("clang++").arg("--version"))?;
    let cache = output_dir.join("fixture-objects.cache");
    let mut objects = Vec::new();
    // All sidecar users must share this flavor's layout and compiler flags.
    // The headless-only framework tables are fixture ownership: keeping them
    // out of the product and graphics clients prevents fake native handles
    // from becoming part of either production archive.
    let mut fixture_sources = vec![
        "probes/runtime_entry_probe.cc",
        "probes/runtime_registration_fixture.cc",
        "probes/media_codec_surface_fixture.cc",
        "probes/runtime_acceptance_state.cc",
        "probes/runtime_fixture_options.cc",
        "probes/graphics_fixture_state.cc",
        "probes/fixture_input_channel.cc",
        "probes/fixture_input_dispatch.cc",
        "probes/fixture_input_guest_io.cc",
        "probes/fixture_motion_event_recycler.cc",
        "probes/runtime_shutdown_probe.cc",
        "probes/runtime_elf_probe.cc",
        "probes/runtime_abi_probe.cc",
        "probes/runtime_frame_probe.cc",
        "probes/runtime_network_probe.cc",
        "probes/runtime_network_loader.cc",
        "probes/runtime_acceptance_phases.cc",
        "probes/runtime_jni_acceptance_probe.cc",
        "probes/runtime_graphics_probe.cc",
        "probes/runtime_graphics_phase.cc",
        "probes/runtime_graphics_input.cc",
        "probes/runtime_graphics_gpu.cc",
        "probes/runtime_hwui_probe.cc",
        "probes/runtime_app_bootstrap.cc",
        "probes/runtime_app_presentation.cc",
        "probes/runtime_app_resources.cc",
        "probes/runtime_app_activity.cc",
        "probes/runtime_upstream_arttest.cc",
        "probes/unwindstack_acceptance.cc",
        "_aosp/art/test/common/runtime_state.cc",
        "_aosp/art/test/common/stack_inspect.cc",
    ];
    if !graphics {
        fixture_sources.extend([
            "probes/headless_graphics_fixture_natives.cc",
            "probes/headless_resources_fixture_natives.cc",
        ]);
    }
    for source in fixture_sources {
        let object = output_dir.join(format!("{}.o", source.replace('/', "_")));
        let mut command = runtime_cpp_command(&refs);
        command
            .args([
                "-include",
                "mirror/object_reference.h",
                "-Wno-macro-redefined",
                "-DSK_BUILD_FOR_ANDROID_FRAMEWORK",
                "-DLOG_TAG=\"DarwinArtFixture\"",
                "-include",
                "log/log_main.h",
                "-DDARWIN_ART_AOSP_COMPAT_LSEEK64",
            ])
            .arg("-idirafter")
            .arg(&ndk_arch)
            .arg("-idirafter")
            .arg(&ndk);
        if graphics {
            command.args(["-DDARWIN_ART_REAL_GRAPHICS", "-DDARWIN_ART_HWUI_GPU"]);
        }
        // Legacy standalone APK fixture builders keep their old entry until
        // they switch runners; this client's entry names are always unique.
        command
            .arg("-Ddarwin_art_run_process=darwin_art_run_fixture")
            .arg("-c")
            .arg(root.join(source))
            .arg("-o")
            .arg(&object);
        compile_cached_probe_tu(&mut command, &object, &cache, &identity)?;
        objects.push(object);
    }
    let library = output_dir.join("libdarwin_art_fixture.dylib");
    let mut linker = Command::new("clang++");
    linker
        .arg("-dynamiclib")
        .arg("-Wl,-undefined,error")
        .arg("-Wl,-install_name,@rpath/libdarwin_art_fixture.dylib")
        .arg(format!("-Wl,-rpath,{}", product_dir.display()))
        .arg(format!(
            "-Wl,-map,{}",
            output_dir.join("fixture-link.map").display()
        ))
        .args(&objects)
        .arg(&product)
        .arg("-o")
        .arg(&library);
    let stamp = output_dir.join("fixture-link.cache");
    let linked = link_with_cache(&mut linker, &library, &stamp)?;
    if !linked.status.success() {
        return Err(format!(
            "fixture client link failed: {}\n{}",
            linked.status,
            String::from_utf8_lossy(&linked.stderr)
        )
        .into());
    }
    println!(
        "runtime-fixture-client: PASS product={} client={}",
        product.display(),
        library.display()
    );
    Ok(())
}
