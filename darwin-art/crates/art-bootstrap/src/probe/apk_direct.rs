use super::super::native_probe_commands::core_probe_includes;
use super::super::native_probe_commands::{
    FixtureCoreObjects, RuntimeCoreObjects, compile_fixture_core_objects,
    compile_native_registration, compile_runtime_core_objects,
};
use super::*;
use crate::build_context::BuildPaths;

pub(crate) fn pinned_direct_apk_ndk_bin() -> Result<PathBuf> {
    let sdk_root = env::var_os("ANDROID_SDK_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Android/sdk"))
        })
        .ok_or("ANDROID_SDK_ROOT or HOME is required to locate pinned NDK r28c")?;
    let bin = sdk_root.join("ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin");
    for tool in ["aarch64-linux-android35-clang", "llvm-objcopy"] {
        if !bin.join(tool).is_file() {
            return Err(format!(
                "pinned NDK r28c tool is missing: {}",
                bin.join(tool).display()
            )
            .into());
        }
    }
    Ok(bin)
}

pub(crate) fn build_direct_apk_runtime_fixture(root: &Path) -> Result<PathBuf> {
    let tool_root = root.join("tools/android-apk-native-direct-load");
    run_command(Command::new("bash").arg(tool_root.join("audit.sh")))?;

    let build_dir = root.join("_build/android-apk-native-direct-runtime");
    let fixture_dir = build_dir.join("fixtures");
    fs::create_dir_all(&fixture_dir)?;
    let clang = pinned_direct_apk_ndk_bin()?.join("aarch64-linux-android35-clang");
    let grandchild = fixture_dir.join("libapk-direct-grandchild.so");
    let child = fixture_dir.join("libapk-direct-child.so");
    let root_library = fixture_dir.join("libapk-direct-root.so");
    let common = [
        "-shared",
        "-fPIC",
        "-O2",
        "-nostdlib",
        "-fuse-ld=lld",
        "-Wl,--hash-style=sysv",
        "-Wl,--build-id=none",
        "-Wl,-z,now",
        "-Wl,-z,norelro",
        "-Wl,-z,max-page-size=16384",
    ];
    run_command(
        Command::new(&clang)
            .args(common)
            .arg("-Wl,-soname,libapk-direct-grandchild.so")
            .arg(format!(
                "-Wl,--version-script,{}",
                tool_root.join("fixtures/grandchild.map").display()
            ))
            .arg(tool_root.join("fixtures/grandchild.c"))
            .arg("-o")
            .arg(&grandchild),
    )?;
    run_command(
        Command::new(&clang)
            .args(common)
            .arg("-Wl,-soname,libapk-direct-child.so")
            .arg(format!(
                "-Wl,--version-script,{}",
                tool_root.join("fixtures/child.map").display()
            ))
            .arg(tool_root.join("fixtures/child.c"))
            .arg(&grandchild)
            .arg("-o")
            .arg(&child),
    )?;
    run_command(
        Command::new(&clang)
            .args(common)
            .arg("-Wl,-soname,libapk-direct-root.so")
            .arg(format!(
                "-Wl,--version-script,{}",
                tool_root.join("fixtures/root.map").display()
            ))
            .arg(tool_root.join("fixtures/root.c"))
            .arg(&child)
            .arg("-o")
            .arg(&root_library),
    )?;
    for library in [&root_library, &child, &grandchild] {
        let kind = command_output(Command::new("file").arg(library))?;
        if !kind.contains("ELF 64-bit LSB shared object, ARM aarch64") {
            return Err(format!("unexpected direct APK fixture format: {kind}").into());
        }
    }

    let apk = build_dir.join("direct-runtime.apk");
    if apk.exists() {
        fs::set_permissions(&apk, fs::Permissions::from_mode(0o600))?;
        fs::remove_file(&apk)?;
    }
    run_command(
        Command::new("python3")
            .arg(tool_root.join("make_fixture.py"))
            .arg(&apk)
            .arg("valid")
            .arg(&root_library)
            .arg(&child)
            .arg(&grandchild),
    )?;
    if fs::metadata(&apk)?.mode() & 0o777 != 0o400 {
        return Err("direct APK runtime fixture is not immutable mode 0400".into());
    }
    Ok(apk)
}

pub(crate) fn build_runtime_direct_apk_link(root: &Path) -> Result<PathBuf> {
    let elf_loader = build_elf_loader(root)?;
    audit_runtime_link(root)?;
    let runtime = root.join("_aosp/art/runtime");
    let build_dir = root.join("_build/runtime-direct-apk-link-probe");
    let original_member_dir = build_dir.join("original-member");
    let patched_member_dir = build_dir.join("patched-member");
    fs::create_dir_all(&original_member_dir)?;
    fs::create_dir_all(&patched_member_dir)?;
    let source_archive = root.join("_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a");
    let runtime_native_owner_archive = root.join("target/release/libdarwin_art_runtime.a");
    if !runtime_native_owner_archive.is_file() {
        return Err(format!(
            "Rust runtime owner archive is missing: {}",
            runtime_native_owner_archive.display()
        )
        .into());
    }
    let bootstrap = build_dir.join("libart-runtime-direct-apk-bootstrap-darwin.a");
    fs::copy(&source_archive, &bootstrap)?;
    // Direct-APK discovery is owned by the native-loader TU. Keep its
    // discovery calls isolated from the general adapter object so objcopy
    // cannot silently produce a direct flavor with unresolved shims.
    let member_name = "darwin_runtime_native_loader.cc.o";
    let original_member = original_member_dir.join(member_name);
    let patched_member = patched_member_dir.join(member_name);
    if original_member.exists() {
        fs::remove_file(&original_member)?;
    }
    if patched_member.exists() {
        fs::remove_file(&patched_member)?;
    }
    run_command(
        Command::new("ar")
            .args(["-x"])
            .arg(&source_archive)
            .arg(member_name)
            .current_dir(&original_member_dir),
    )?;
    let objcopy = pinned_direct_apk_ndk_bin()?.join("llvm-objcopy");
    let renames = [
        (
            "_darwin_art_elf_discover_sibling_graph",
            "_darwin_art_direct_discover_sibling_graph",
        ),
        (
            "_darwin_art_elf_discovered_graph_root_soname",
            "_darwin_art_direct_discovered_graph_root_soname",
        ),
        (
            "_darwin_art_elf_discovered_graph_sources",
            "_darwin_art_direct_discovered_graph_sources",
        ),
        (
            "_darwin_art_elf_discovered_graph_destroy",
            "_darwin_art_direct_discovered_graph_destroy",
        ),
    ];
    let mut objcopy_command = Command::new(objcopy);
    for (from, to) in renames {
        objcopy_command.args(["--redefine-sym", &format!("{from}={to}")]);
    }
    run_command(objcopy_command.arg(&original_member).arg(&patched_member))?;
    let patched_undefined = command_output(Command::new("nm").arg("-u").arg(&patched_member))?;
    for (_, direct) in renames {
        if !patched_undefined.contains(direct) {
            return Err(format!("patched runtime adapter does not reference {direct}").into());
        }
    }
    run_command(
        Command::new("ar")
            .arg("-d")
            .arg(&bootstrap)
            .arg(member_name),
    )?;
    run_command(
        Command::new("ar")
            .arg("-r")
            .arg(&bootstrap)
            .arg(&patched_member),
    )?;
    run_command(Command::new("ar").arg("-s").arg(&bootstrap))?;

    let object = build_dir.join("darwin_art_runtime_direct_apk.cc.o");
    let graph_object = build_dir.join("darwin_art_runtime_apk_graph.cc.o");
    let network_object = if let Some(path) = env::var_os("DARWIN_ART_NATIVE_NETWORK_OBJECT") {
        PathBuf::from(path)
    } else {
        compile_runtime_network_probe(root, &build_dir)?
    };
    if !network_object.is_file() {
        return Err(format!(
            "runtime network object is missing: {}",
            network_object.display()
        )
        .into());
    }
    let surface_object = root.join("_build/runtime-link-probe/darwin_surface_bridge.mm.o");
    let document_panel_object = root.join("_build/runtime-link-probe/document_panel.mm.o");
    let runtime_library = build_dir.join("libdarwin_art_runtime_direct_apk.dylib");
    let includes = [
        root.join("include"),
        root.join("compat"),
        root.join("crates/darwin-art-elf-loader/include"),
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
    ];
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    // The direct APK flavor has an independent cache from the GPU graphics
    // probe.  A command-line flavor change must never reuse a real-HWUI TU.
    // Tests reuse the production objects and their cache; fixture objects
    // have a separate output root and cannot enter the production closure.
    let build_paths = BuildPaths::from_root(root);
    let core_build_dir = build_paths.native_output("native-runtime/core");
    fs::create_dir_all(&core_build_dir)?;
    let core_probe_cache = core_build_dir.join("core-probe-hashes.cache");
    let core_includes = core_probe_includes(root, &build_paths, &runtime);
    let core_include_refs = core_includes
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let probe_cache = build_dir.join("runtime-direct-apk-probe-hashes.cache");
    let RuntimeCoreObjects {
        boot_native_registration: boot_native_registration_object,
        boot_native_libraries: boot_native_libraries_object,
        process_state: process_state_object,
        process_config: process_config_object,
        process_shutdown: process_shutdown_object,
        vm_shutdown: vm_shutdown_object,
        app_process_shutdown: app_process_shutdown_object,
        ..
    } = compile_runtime_core_objects(
        root,
        &core_build_dir,
        &core_include_refs,
        &core_probe_cache,
        &compiler_identity,
    )?;
    let fixture_build_dir = build_paths.native_output("native-probes/core");
    fs::create_dir_all(&fixture_build_dir)?;
    let fixture_cache = fixture_build_dir.join("fixture-hashes.cache");
    let FixtureCoreObjects {
        elf: elf_probe_object,
        abi: abi_probe_object,
        acceptance: acceptance_state_object,
        fixture_options: fixture_options_object,
        graphics_fixture_state: graphics_fixture_state_object,
        shutdown: shutdown_probe_object,
        frame: frame_probe_object,
    } = compile_fixture_core_objects(
        root,
        &fixture_build_dir,
        &core_include_refs,
        &fixture_cache,
        &compiler_identity,
    )?;
    // The runtime entry point is intentionally kept as a thin orchestrator;
    // its registration phase is a separate TU and every flavor must link it
    // explicitly.  The direct-APK flavor used to omit this edge, which only
    // surfaced when the APK-specific dylib was linked after the TU split.
    let registration_object = compile_native_registration(
        root,
        &build_dir,
        &include_refs,
        &probe_cache,
        &compiler_identity,
        &[],
    )?;
    let registration_fixture_object =
        build_dir.join("darwin_art_runtime_registration_fixture.cc.o");
    let mut registration_fixture_command = runtime_cpp_command(&include_refs);
    registration_fixture_command
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-c")
        .arg(root.join("probes/runtime_registration_fixture.cc"))
        .arg("-o")
        .arg(&registration_fixture_object);
    let _ = compile_cached_probe_tu(
        &mut registration_fixture_command,
        &registration_fixture_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let headless_graphics_fixture_object =
        build_dir.join("darwin_art_headless_graphics_fixture_natives.cc.o");
    let mut headless_graphics_fixture_command = runtime_cpp_command(&include_refs);
    headless_graphics_fixture_command
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-c")
        .arg(root.join("probes/headless_graphics_fixture_natives.cc"))
        .arg("-o")
        .arg(&headless_graphics_fixture_object);
    let _ = compile_cached_probe_tu(
        &mut headless_graphics_fixture_command,
        &headless_graphics_fixture_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let headless_resources_fixture_object =
        build_dir.join("darwin_art_headless_resources_fixture_natives.cc.o");
    let mut headless_resources_fixture_command = runtime_cpp_command(&include_refs);
    headless_resources_fixture_command
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-c")
        .arg(root.join("probes/headless_resources_fixture_natives.cc"))
        .arg("-o")
        .arg(&headless_resources_fixture_object);
    let _ = compile_cached_probe_tu(
        &mut headless_resources_fixture_command,
        &headless_resources_fixture_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let graphics_probe_object = build_dir.join("darwin_art_runtime_graphics_probe.cc.o");
    let mut graphics_probe_command = runtime_cpp_command(&include_refs);
    graphics_probe_command
        .arg("-I")
        .arg(root.join("_aosp/frameworks/minikin/include"))
        .arg("-I")
        .arg(root.join("_aosp/external/harfbuzz_ng/src"))
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-DDARWIN_ART_AOSP_COMPAT_LSEEK64")
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-DLOG_TAG=\"DarwinArtHWUI\"")
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-include")
        .arg("log/log_main.h")
        .arg("-c")
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/pipeline/skia"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/androidfw/include"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/include"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/native/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/incremental_delivery/incfs/util/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libutils/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libsystem/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libcutils/include"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/native/libs/ui/include"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/native/libs/ui/include_types"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/native/libs/nativewindow/include"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/native/libs/arect/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/logging/liblog/include"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/pipeline/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/effects"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/private"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/android"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/utils"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/codec"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/minikin/include"))
        .arg("-I")
        .arg(root.join("_aosp/external/harfbuzz_ng/src"))
        .arg(root.join("probes/runtime_graphics_probe.cc"))
        .arg("-o")
        .arg(&graphics_probe_object);
    let _ = compile_cached_probe_tu(
        &mut graphics_probe_command,
        &graphics_probe_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let graphics_phase_object = compile_runtime_graphics_phase(root, &build_dir, &include_refs)?;
    let graphics_input_object =
        compile_runtime_graphics_input_probe(root, &build_dir, &include_refs)?;
    let graphics_state_object = compile_runtime_graphics_state_cpu(
        root,
        &build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    let graphics_session_object = compile_runtime_graphics_session_cpu(
        root,
        &build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    let jni_acceptance_object =
        compile_runtime_jni_acceptance_probe(root, &build_dir, &include_refs)?;
    let mut probe_command = runtime_cpp_command(&include_refs);
    probe_command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-I")
        .arg(root.join("_aosp/external/skia"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-c")
        .arg(root.join("probes/runtime_entry_probe.cc"))
        .arg("-o")
        .arg(&object);
    let _ = compile_cached_probe_tu(
        &mut probe_command,
        &object,
        &probe_cache,
        &compiler_identity,
    )?;
    let network_loader_object =
        compile_runtime_network_loader_probe(root, &build_dir, &include_refs)?;
    let context_loader_object = compile_system_class_loader(root, &build_dir, &include_refs)?;
    let app_bootstrap_object =
        compile_runtime_app_bootstrap_probe(root, &build_dir, &include_refs)?;
    let _app_resources_object =
        compile_runtime_app_resources_probe(root, &build_dir, &include_refs)?;
    let _app_activity_object = compile_runtime_app_activity_probe(root, &build_dir, &include_refs)?;
    let app_presentation_object =
        compile_runtime_app_presentation_probe(root, &build_dir, &include_refs)?;
    let app_resources_object = app_resources_object_path(&build_dir);
    let app_activity_object = app_activity_object_path(&build_dir);
    let mut graph_command = runtime_cpp_command(&include_refs);
    graph_command
        .args(["-include", "mirror/object_reference.h"])
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-Wno-macro-redefined")
        .arg("-DDARWIN_ART_DIRECT_APK_RUNTIME")
        .arg("-c")
        .arg(root.join("probes/runtime_apk_graph.cc"))
        .arg("-o")
        .arg(&graph_object);
    let _ = compile_cached_probe_tu(
        &mut graph_command,
        &graph_object,
        &probe_cache,
        &compiler_identity,
    )?;

    let mut linker = Command::new("clang++");
    linker
        .arg("-dynamiclib")
        .arg("-Wl,-install_name,@rpath/libdarwin_art_runtime_direct_apk.dylib")
        .arg("-Wl,-exported_symbol,_darwin_art_run_process")
        .arg("-Wl,-exported_symbol,_darwin_art_shutdown_process")
        .arg("-Wl,-exported_symbol,_darwin_art_dispatch_pointer")
        .arg("-Wl,-exported_symbol,_darwin_art_unwindstack_quick_frames")
        .arg("-Wl,-exported_symbol,___dex_debug_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_pump_framework_frame")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_create")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_close")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_destroy")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_dispatch_pointer")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_dispatch_pointer_v2")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_dispatch_key_v1")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_pump_frame")
        .arg("-Wl,-exported_symbol,_darwin_art_graphics_session_pump_main_looper")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_create")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_resize")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_get_size")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_update")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_map_producer")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_unmap_producer")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_present")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_present_async")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_pump_events")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_close_requested")
        .arg("-Wl,-exported_symbol,_darwin_art_appkit_pump_events")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_set_input_sink")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_destroy")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_active_gpu")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_gpu_active_canvas")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_install_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_fd_inheritance_boundary")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_uninstall_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_socket_broker_is_active")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_clear_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_acquire")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_release")
        .arg("-Wl,-exported_symbol,__ZN3art6GetTidEv")
        .arg("-Wl,-exported_symbol,__ZN3art5Locks26thread_suspend_count_lock_E")
        .arg("-Wl,-exported_symbol,__ZN3art6Thread11is_started_E")
        .arg("-Wl,-dead_strip")
        .arg(&object)
        .arg(&elf_probe_object)
        .arg(&abi_probe_object)
        .arg(&acceptance_state_object)
        .arg(&process_state_object)
        .arg(&process_config_object)
        .arg(&process_shutdown_object)
        .arg(&vm_shutdown_object)
        .arg(&app_process_shutdown_object)
        .arg(&fixture_options_object)
        .arg(&graphics_fixture_state_object)
        .arg(&shutdown_probe_object)
        .arg(&frame_probe_object)
        .arg(&registration_object)
        .arg(&boot_native_registration_object)
        .arg(&boot_native_libraries_object)
        .arg(&registration_fixture_object)
        .arg(&headless_graphics_fixture_object)
        .arg(&headless_resources_fixture_object)
        .arg(&graphics_probe_object)
        .arg(&graphics_state_object)
        .arg(&network_loader_object)
        .arg(&context_loader_object)
        .arg(&app_bootstrap_object)
        .arg(&app_resources_object)
        .arg(&app_activity_object)
        .arg(&app_presentation_object)
        .arg(&jni_acceptance_object)
        .arg(&graphics_session_object)
        .arg(&graphics_phase_object)
        .arg(&graphics_input_object)
        .arg(root.join("_build/runtime-graphics-bootstrap/objects/darwin_provider_owners.cc.o"))
        .arg(&graph_object)
        .arg(&network_object)
        .arg(&surface_object)
        .arg(&document_panel_object)
        .arg(root.join("compat/input/darwin_hardware_key_translation.mm"))
        .arg(root.join("compat/window/desktop_root_events.mm"))
        .arg(root.join("compat/window/desktop_foreground_provider.cc"))
        .arg(root.join("compat/window/desktop_root_target.mm"))
        .arg("-fobjc-arc")
        .arg(root.join("compat/window/desktop_root_surface.mm"))
        .arg(root.join("compat/window/appkit_window_delegate.mm"))
        .arg(root.join("compat/window/appkit_content_view.mm"))
        .arg(root.join("_build/skia-metal-gpu/libskia.a"))
        .arg(root.join("_build/skia-metal-gpu/libskcms.a"))
        .arg(&bootstrap)
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-binary128-conversion.a"
            )
            .display()
        ))
        .arg(
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-native-providers.a",
            ),
        )
        .arg(
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-float-conversion.a",
            ),
        )
        .arg(
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a",
            ),
        )
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join("_build/icu-foundation/libandroidicuinit-darwin.a")
                .display()
        ))
        .arg(root.join("_build/icu-foundation/libicuuc-common-darwin.a"))
        .arg(root.join("_build/icu-foundation/libicuuc-stubdata-darwin.a"))
        .arg(&elf_loader)
        .arg(format!(
            "-Wl,-force_load,{}",
            runtime_native_owner_archive.display()
        ))
        .arg(root.join("_build/interpreter-core/libart-interpreter-darwin.a"))
        .arg(root.join("_build/runtime-arm64/libart-arm64-darwin.a"))
        .arg(root.join("_build/runtime-core/libart-core-darwin.a"))
        .arg(root.join("_build/runtime-platform/libart-platform-darwin.a"))
        .arg(root.join("_build/dex-probe/libdexfile-darwin.a"))
        .arg(root.join("_build/foundation/libartbase-darwin.a"))
        // The direct APK flavor does not link the graphics closure, so it
        // must carry the same source-pinned fmt v11 owner as the full runtime
        // link rather than relying on the host fmt dylib (currently v12).
        .arg(root.join("_build/libbase-foundation/libandroid-base-darwin.a"))
        .arg(root.join("_build/foundation/libandroid-base-darwin.a"))
        .arg(root.join("_build/foundation/libziparchive-darwin.a"))
        .arg(root.join("_build/nativehelper-foundation/libnativehelper_jvm.a"))
        .arg(root.join("_build/graphics-foundations/liblog-darwin.a"))
        .args([
            "-L/opt/homebrew/lib",
            "-L/opt/homebrew/opt/icu4c@78/lib",
            "-lfmt",
            "-llz4",
            "-lsqlite3",
            "-licui18n",
            "-licuuc",
            "-licudata",
            "-lz",
            "-lresolv",
            "-framework",
            "AppKit",
            "-framework",
            "ApplicationServices",
            "-framework",
            "IOSurface",
            "-framework",
            "Metal",
            "-framework",
            "QuartzCore",
            "-framework",
            "Security",
            "-framework",
            "AudioToolbox",
            "-framework",
            "CoreAudio",
            "-framework",
            "CoreMedia",
            "-framework",
            "CoreVideo",
            "-framework",
            "VideoToolbox",
            "-framework",
            "Network",
            "-framework",
            "SystemConfiguration",
            "-o",
        ])
        .arg(&runtime_library);
    run_command(&mut linker)?;
    let undefined = command_output(Command::new("nm").arg("-u").arg(&runtime_library))?;
    if undefined.contains("darwin_art_direct_") {
        return Err("direct APK runtime dylib has unresolved discovery shims".into());
    }
    let all_symbols = command_output(Command::new("nm").args(["-aC"]).arg(&runtime_library))?;
    for required in [
        "darwin_art_direct_discover_sibling_graph",
        "darwin_art_direct_discovered_graph_sources",
        "darwin_art_elf_graph_load",
        "NativeBridgeGetTrampoline2",
    ] {
        if !all_symbols.contains(required) {
            return Err(format!("direct APK runtime lacks required symbol {required}").into());
        }
    }
    Ok(runtime_library)
}

pub(crate) fn probe_runtime_apk_direct(root: &Path) -> Result<()> {
    let apk = build_direct_apk_runtime_fixture(root)?;
    let runtime_library = build_runtime_direct_apk_link(root)?;
    prepare_icu_bootclasspath(root)?;
    build_runtime_host(root)?;
    let executable = root.join("target/debug/darwin-art-host");
    let core_oj = root.join("_prebuilt/android-16/bootclasspath/core-oj.jar");
    let core_libart = root.join("_prebuilt/android-16/bootclasspath/core-libart.jar");
    let framework = root.join("_prebuilt/android-16/bootclasspath/framework.jar");
    let core_icu4j = root.join("_build/bootclasspath/core-icu4j.jar");
    let classes_dex = root.join("_build/dex-probe/dex/classes.dex");
    for input in [
        &executable,
        &runtime_library,
        &core_oj,
        &core_libart,
        &framework,
        &core_icu4j,
        &classes_dex,
        &apk,
    ] {
        if !input.is_file() {
            return Err(format!("direct APK runtime input is missing: {}", input.display()).into());
        }
    }
    let output = command_output(
        Command::new(&executable)
            .arg(&runtime_library)
            .arg(&core_oj)
            .arg(&core_libart)
            .arg(&framework)
            .arg(&core_icu4j)
            .arg(&classes_dex)
            .env("DARWIN_ART_DIRECT_APK_FIXTURE", &apk)
            .env("DARWIN_ART_DIRECT_APK_ROOT", "libapk-direct-root.so"),
    )?;
    let expected = "Hello from Darwin ART main: 안녕\n\
                    ART Android direct APK ELF: source=readonly-fd-slices copy=0 extract=0 alignment=16384 graph=root+child+grandchild load=JavaVMExt+NativeBridge JNI_OnLoad=0x00010006 unload=shutdown-trampolines-zero authority=isolated-process\n\
                    ART Darwin Runtime::Create: ok\n\
                    ART Darwin app ClassLoader: PathClassLoader\n\
                    ART Darwin DEX interpreter: Hello.answer()=42\n\
                    ART Darwin JNI: hostPageSize()=16384 nativeRoundTrip()=42\n\
                    ART runtime native: System.arraycopy()=42\n\
                    ART Android framework: ProbeActivity().probeValue()=42\n\
                    ART Android window: Activity.attach()=PhoneWindow+DecorView\n\
                    ART Android view: Activity.setContentView()->DecorView.draw(Canvas)=360x640\n\
                    ART Android lifecycle: Activity.onCreate()=43\n\
                    ART Darwin launcher: main(String[])=ok";
    if output.trim() != expected {
        return Err(format!("unexpected direct APK runtime output: {output:?}").into());
    }
    println!(
        "probe-runtime-apk-direct: ART JavaVMExt + NativeBridge + readonly APK fd slices PASS"
    );
    Ok(())
}
