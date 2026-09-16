use super::common::{build_runtime_native_owner, require_file};
use super::graphics_core_probes::{
    CoreProbeObjects, compile_core_probe_objects, core_probe_includes,
};
use super::runtime_link_checks::validate_runtime_link;
use super::*;

pub(crate) fn audit_runtime_link(root: &Path) -> Result<()> {
    // This audit is a supported direct entry point.  Refresh the headless
    // bootstrap producer before force-loading its archive so the link checks
    // can never validate a stale object set (notably the framework shutdown
    // owner) left by an earlier graph invocation.
    build_runtime_bootstrap(root)?;
    build_shell_gate(root, "build-android16-application-shared-memory.sh")?;
    build_shell_gate(root, "build-android16-debugstore.sh")?;
    build_shell_gate(root, "build-android16-activity-thread.sh")?;
    build_shell_gate(root, "build-android16-bionic-linker-config.sh")?;
    build_shell_gate(root, "build-android16-tracing-perfetto.sh")?;
    build_shell_gate(root, "build-android16-system-properties.sh")?;
    build_shell_gate(root, "build-android16-binder-jni.sh")?;
    // Runtime::Create can compile JNI stubs through the ART compiler archive.
    // Build that producer here, too: the headless audit is a supported direct
    // entry point (and is used by `all`), so relying on a prior explicit JIT
    // command can link a runtime with stale JNI codegen.
    build_jit_compiler(root)?;
    let unwindstack_core = build_runtime_unwindstack_core(root)?;
    let unwindstack_dex = build_runtime_unwindstack_dex(root)?;
    let unwindstack_providers =
        root.join("_build/runtime-unwindstack/libunwindstack-mach-providers.a");
    let rust_demangle = root.join(
        "_build/runtime-unwindstack/rust-demangle-target/release/libdarwin_art_rust_demangle.a",
    );
    let elf_loader = build_elf_loader(root)?;
    let runtime = root.join("_aosp/art/runtime");
    let build_paths = BuildPaths::from_root(root);
    let build_dir = build_paths.native_output("runtime-link-probe");
    let object = build_dir.join("darwin_art_runtime.cc.o");
    let surface_object = build_dir.join("darwin_surface_bridge.mm.o");
    // The CPU/runtime link owns only the IOSurface/AppKit core. The Ganesh
    // Metal implementation is compiled and linked exclusively by the
    // graphics runtime below, where the GPU Skia archive is explicit.
    let runtime_library = build_dir.join("libdarwin_art_runtime.dylib");
    fs::create_dir_all(&build_dir)?;
    let openjdk_named_jni_owner = build_dir.join("libopenjdk-named-jni-owner.dylib");
    run_command(
        Command::new("bash")
            .arg(root.join("tools/build-android16-openjdk-named-jni-owner.sh"))
            .arg(&openjdk_named_jni_owner),
    )?;
    require_file(
        &openjdk_named_jni_owner,
        "OpenJDK named-JNI owner dylib is missing",
    )?;
    let network_object = if let Some(path) = env::var_os("DARWIN_ART_NATIVE_NETWORK_OBJECT") {
        PathBuf::from(path)
    } else {
        compile_runtime_network_probe(root, &build_dir)?
    };
    require_file(&network_object, "runtime network object is missing")?;
    build_shell_gate(root, "build-bionic-runtime-provider-closure.sh")?;
    let runtime_native_owner_archive = build_runtime_native_owner(root)?;
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let probe_cache = build_dir.join("runtime-link-probe-hashes.cache");
    let core_build_dir = build_paths.native_output("native-probes/core");
    fs::create_dir_all(&core_build_dir)?;
    let core_probe_cache = core_build_dir.join("core-probe-hashes.cache");
    let CoreProbeObjects {
        elf: elf_probe_object,
        abi: abi_probe_object,
        process_state: process_state_object,
        process_options: process_options_object,
        shutdown: shutdown_probe_object,
        frame: frame_probe_object,
    } = compile_core_probe_objects(
        root,
        &core_build_dir,
        &include_refs,
        &core_probe_cache,
        &compiler_identity,
    )?;
    let registration_object = build_dir.join("darwin_art_runtime_registration_phase.cc.o");
    let mut registration_command = runtime_cpp_command(&include_refs);
    registration_command
        .arg("-c")
        .arg(root.join("probes/runtime_registration_phase.cc"))
        .arg("-o")
        .arg(&registration_object);
    let _ = compile_cached_probe_tu(
        &mut registration_command,
        &registration_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let graphics_probe_object = build_dir.join("darwin_art_runtime_graphics_probe.cc.o");
    let mut graphics_probe_command = runtime_cpp_command(&include_refs);
    graphics_probe_command
        .arg("-c")
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
        .arg("-I")
        .arg(root.join("_aosp/external/googletest/googletest/include"))
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
    // The runtime entry probe owns the APK GPU lifecycle even in the CPU
    // audit flavor. Use the production GPU implementation here; unresolved
    // GPU symbols must not be hidden behind a weak stand-in.
    let graphics_gpu_object = compile_runtime_graphics_gpu_probe(
        root,
        &build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    let graphics_cpu_stubs_object = build_dir.join("darwin_art_runtime_graphics_cpu_stubs.cc.o");
    let mut graphics_cpu_stubs_command = runtime_cpp_command(&include_refs);
    graphics_cpu_stubs_command
        .arg("-c")
        .arg(root.join("probes/runtime_graphics_cpu_stubs.cc"))
        .arg("-o")
        .arg(&graphics_cpu_stubs_object);
    let _ = compile_cached_probe_tu(
        &mut graphics_cpu_stubs_command,
        &graphics_cpu_stubs_object,
        &probe_cache,
        &compiler_identity,
    )?;
    let graphics_state_object = if let Some(path) =
        env::var_os("DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT")
        && Path::new(&path).is_file()
    {
        PathBuf::from(path)
    } else {
        compile_runtime_graphics_state_probe(
            root,
            &build_dir,
            &include_refs,
            &ndk_include,
            &ndk_arch_include,
        )?
    };
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
    let context_loader_object =
        compile_runtime_context_loader_probe(root, &build_dir, &include_refs)?;
    // The runtime entry probe publishes the unchanged AOSP test harness
    // registrar. Link its real implementation so the public test ABI exports
    // are backed by the pinned source rather than unresolved linker roots.
    let mut arttest_objects = Vec::new();
    for (label, source) in [
        ("runtime_state", "_aosp/art/test/common/runtime_state.cc"),
        ("stack_inspect", "_aosp/art/test/common/stack_inspect.cc"),
        ("registration", "probes/runtime_upstream_arttest.cc"),
    ] {
        let arttest_object = build_dir.join(format!("darwin_art_arttest_{label}.cc.o"));
        let mut arttest_command = runtime_cpp_command(&include_refs);
        arttest_command
            .arg("-I")
            .arg(root.join("_aosp/art/test/common"))
            .arg("-I")
            .arg(root.join("_aosp/libnativehelper-full/include"))
            .arg("-I")
            .arg(root.join("_aosp/system/logging/liblog/include"))
            .arg("-include")
            .arg(
                root.join("_build/runtime-common/patched-source/runtime/mirror/object_reference.h"),
            )
            .arg("-c")
            .arg(root.join(source))
            .arg("-o")
            .arg(&arttest_object);
        let _ = compile_cached_probe_tu(
            &mut arttest_command,
            &arttest_object,
            &probe_cache,
            &compiler_identity,
        )?;
        arttest_objects.push(arttest_object);
    }
    let app_bootstrap_object = if let Some(path) =
        env::var_os("DARWIN_ART_NATIVE_APP_BOOTSTRAP_OBJECT")
        && Path::new(&path).is_file()
    {
        PathBuf::from(path)
    } else {
        compile_runtime_app_bootstrap_probe(root, &build_dir, &include_refs)?
    };
    let _compiled_app_resources =
        compile_runtime_app_resources_probe(root, &build_dir, &include_refs)?;
    let _compiled_app_activity =
        compile_runtime_app_activity_probe(root, &build_dir, &include_refs)?;
    let app_presentation_object = if let Some(path) =
        env::var_os("DARWIN_ART_NATIVE_APP_PRESENTATION_OBJECT")
        && Path::new(&path).is_file()
    {
        PathBuf::from(path)
    } else {
        compile_runtime_app_presentation_probe(root, &build_dir, &include_refs)?
    };
    let app_resources_object = app_resources_object_path(&build_dir);
    require_file(
        &app_resources_object,
        "runtime app resources object is missing",
    )?;
    let app_activity_object = app_activity_object_path(&build_dir);
    require_file(
        &app_activity_object,
        "runtime app activity object is missing",
    )?;
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
        .arg(root.join("_aosp/external/skia/include/core"))
        .arg("-I")
        .arg(root.join("_aosp/external/skia/include/effects"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/hwui"))
        .arg("-I")
        .arg(root.join("_aosp/frameworks/base/libs/hwui/pipeline/skia"))
        .arg("-I")
        .arg(root.join("_aosp/system/logging/liblog/include"))
        .arg("-I")
        .arg(root.join("_aosp/system/core/libcutils/include"))
        .arg("-DSK_BUILD_FOR_ANDROID_FRAMEWORK")
        .arg("-DSK_USER_CONFIG_HEADER=\"include/config/SkUserConfigManual.h\"")
        .arg("-o")
        .arg(&surface_object);
    let _ = compile_cached_probe_tu(
        &mut surface_command,
        &surface_object,
        &probe_cache,
        &compiler_identity,
    )?;

    let mut linker = Command::new("clang++");
    linker
        .arg("-dynamiclib")
        .arg("-Wl,-install_name,@rpath/libdarwin_art_runtime.dylib")
        .arg(root.join("_build/application-shared-memory/libapplication-shared-memory-darwin.a"))
        .arg(root.join("_build/debugstore/libdebugstore-darwin.a"))
        .arg(root.join("_build/activity-thread/libactivity-thread-darwin.a"))
        .arg(root.join("_build/bionic-linker-config/linker_dlwarning.o"))
        .arg(root.join("_build/tracing-perfetto/libandroid-tracing-perfetto-darwin.a"))
        .arg(root.join("_build/tracing-perfetto/perfetto-out/libperfetto_c.dylib"))
        .arg("-Wl,-rpath,@loader_path/../tracing-perfetto/perfetto-out")
        // RuntimeSession resolves this lifecycle hook through the dylib ABI;
        // force it through dead-strip and publish it for the host engine.
        .arg("-Wl,-u,_darwin_art_prepare_process_exit")
        .arg("-Wl,-exported_symbol,_darwin_art_prepare_process_exit")
        .arg("-Wl,-exported_symbol,_darwin_art_run_process")
        .arg("-Wl,-exported_symbol,_Java_Main_makeVisiblyInitialized")
        .arg("-Wl,-exported_symbol,_darwin_art_shutdown_process")
        .arg("-Wl,-exported_symbol,_darwin_art_dispatch_pointer")
        .arg("-Wl,-exported_symbol,_darwin_art_dispatch_pointer_v2")
        .arg("-Wl,-exported_symbol,_darwin_art_pump_framework_frame")
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
        .arg("-Wl,-exported_symbol,_darwin_art_surface_next_pointer_event")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_next_pointer_event_v2")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_next_key_event_v1")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_destroy")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_active_gpu")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_install_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_clear_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_install_configured")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_install")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_export_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_import_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_close_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_acquire")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_release")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_create")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_attach")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_lookup")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_destroy")
        .arg("-Wl,-exported_symbol,__ZN3art6GetTidEv")
        .arg("-Wl,-exported_symbol,__ZN3art5Locks26thread_suspend_count_lock_E")
        .arg("-Wl,-exported_symbol,__ZN3art6Thread11is_started_E")
        .arg("-Wl,-exported_symbol,__ZN3art3jit3Jit13JitAtFirstUseEv")
        .arg("-Wl,-exported_symbol,__ZN3art6mirror5Class30FindDeclaredDirectMethodByNameENSt3__117basic_string_viewIcNS2_11char_traitsIcEEEENS_11PointerSizeE")
        .arg("-Wl,-exported_symbol,__ZN3art8CodeInfoC1EPKNS_20OatQuickMethodHeaderE")
        .arg("-Wl,-exported_symbol,__ZNK3art3jit12JitCodeCache10ContainsPcEPKv")
        .arg("-Wl,-exported_symbol,___jit_debug_descriptor")
        .arg("-Wl,-exported_symbol,___dex_debug_descriptor")
        .arg("-Wl,-exported_symbol,_JVM_GetLastErrorString")
        .arg("-Wl,-exported_symbol,_JVM_Sync")
        .arg("-Wl,-exported_symbol,_Java_java_lang_Float_floatToRawIntBits")
        .arg("-Wl,-exported_symbol,_Java_java_lang_Float_intBitsToFloat")
        .arg("-Wl,-exported_symbol,_Java_java_lang_Double_doubleToRawLongBits")
        .arg("-Wl,-exported_symbol,_Java_java_lang_Double_longBitsToDouble")
        .arg("-Wl,-exported_symbol,_Java_android_system_OsConstants_initConstants")
        .arg("-Wl,-dead_strip")
        .arg(&object)
        .arg(&elf_probe_object)
        .arg(&abi_probe_object)
        .arg(&process_state_object)
        .arg(&process_options_object)
        .arg(&registration_object)
        .arg(&shutdown_probe_object)
        .arg(&frame_probe_object)
        .arg(&graphics_probe_object)
        .arg(&graphics_state_object)
        .arg(&graphics_cpu_stubs_object)
        .arg(&graphics_gpu_object)
        .arg(&network_loader_object)
        .arg(&context_loader_object)
        .args(&arttest_objects)
        .arg(&app_bootstrap_object)
        .arg(&app_resources_object)
        .arg(&app_activity_object)
        .arg(&app_presentation_object)
        .arg(&jni_acceptance_object)
        .arg(&graphics_phase_object)
        .arg(&graphics_input_object)
        .arg(&network_object)
        .arg(&surface_object)
        // Native registration is rooted from ART startup rather than a direct
        // C reference. Force-load the archive so Binder/graphics/system
        // registrars remain present in the headless runtime dylib as they are
        // in the graphics runtime.
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join("_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a")
                .display()
        ))
        .arg(root.join("_build/binder-jni/libbinder-jni-darwin.a"))
        .arg(root.join("_build/binder-jni/libbinder-rpc-boundary-darwin.a"))
        // Dynamically resolved by the Rust Binder process endpoint. Link the
        // exact FD boundary object because ordinary archive extraction cannot
        // see dlsym-only roots.
        .arg(root.join("_build/binder-jni/fd-transport.o"))
        // RegisterLibcoreNatives owns these AOSP OpenJDK tables; keep the
        // module archives on the CPU closure rather than manufacturing local
        // substitutes for their entrypoints.
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join("_build/system-natives-darwin/libopenjdk-system-natives-darwin.a")
                .display()
        ))
        // libopenjdk's JNI owner imports the AOSP JVM service ABI. Keep that
        // provider in the process runtime so RTLD_LOCAL named-JNI modules can
        // resolve it without relying on flat-namespace host symbols.
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join("_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a")
                .display()
        ))
        .arg(root.join("_build/system-natives-darwin/libcrypto-boringssl-darwin.a"))
        .arg(root.join("_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a"))
        .arg(root.join("_build/unix-native-dispatcher-darwin/libfdlibm-darwin.a"))
        .arg(root.join("_build/unix-filesystem-darwin/libopenjdk-unix-filesystem-darwin.a"))
        .arg(root.join("_build/icu-foundation/libicui18n-darwin.a"))
        .arg(root.join("_build/androidfw-foundation/libandroidfw-darwin.a"))
        .arg(root.join("_build/system-properties/libsystem-properties-jni-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libutils-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libcutils-darwin.a"))
        .arg(root.join("_build/graphics-foundations/liblog-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libsurfaceflinger-frontend-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libgui-transaction-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libbinder-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libui-fence-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libutils-binder-darwin.a"))
        .arg(root.join("_build/ui-types-foundation/libui-types.a"))
        .arg(root.join("_build/android-graphics-jni/libandroid-graphics-jni-darwin.a"))
        .arg(root.join("_build/resource-jni-foundation/libandroid-resource-jni-darwin.a"))
        .arg(root.join("_build/skia-metal-gpu/libskia.a"))
        .arg(&unwindstack_providers)
        .arg(&unwindstack_core)
        .arg(&unwindstack_dex)
        .arg(&rust_demangle)
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
        .arg(root.join("_build/jit-compiler/libart-compiler-darwin.a"))
        .arg(root.join("_build/jit-compiler/libart-libelffile-darwin.a"))
        .arg(root.join("_build/runtime-arm64/libart-arm64-darwin.a"))
        .arg(root.join("_build/runtime-core/libart-core-darwin.a"))
        .arg(root.join("_build/runtime-platform/libart-platform-darwin.a"))
        .arg(root.join("_build/dex-probe/libdexfile-darwin.a"))
        .arg(root.join("_build/foundation/libartbase-darwin.a"))
        // The runtime probe uses the source-pinned Android-base/fmt v11
        // objects.  The smaller foundation archive intentionally omits those
        // objects; linking Homebrew fmt here would mix ABI generations.
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
            "-lzstd",
            "-lsqlite3",
            "-licui18n",
            "-licuuc",
            "-licudata",
            "-lz",
            "-lresolv",
            "-framework",
            "AppKit",
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
    // RTLD_LOCAL OpenJDK native modules import these process-wide Bionic
    // shims. Exporting the exact ABI set keeps their state in one Rust owner
    // instead of embedding a second facade instance in each module.
    for symbol in [
        "_Java_java_lang_System_log",
        "_darwin_art_bionic_access",
        "_darwin_art_bionic_chmod",
        "_darwin_art_bionic_close",
        "_darwin_art_bionic_closedir",
        "_darwin_art_bionic_fchmod",
        "_darwin_art_bionic_fchown",
        "_darwin_art_bionic_fd_export_for_scm",
        "_darwin_art_bionic_fs_fcntl_core",
        "_darwin_art_bionic_fs_owns_fd_core",
        "_darwin_art_bionic_fs_resolve_private_host_path",
        "_darwin_art_bionic_fs_statvfs_core",
        "_darwin_art_bionic_fstat",
        "_darwin_art_bionic_fsync",
        "_darwin_art_bionic_getcwd",
        "_darwin_art_bionic_link",
        "_darwin_art_bionic_lseek",
        "_darwin_art_bionic_lstat",
        "_darwin_art_bionic_mkdir",
        "_darwin_art_bionic_open",
        "_darwin_art_bionic_opendir",
        "_darwin_art_bionic_pathconf",
        "_darwin_art_bionic_read",
        "_darwin_art_bionic_readdir",
        "_darwin_art_bionic_readlink",
        "_darwin_art_bionic_realpath",
        "_darwin_art_bionic_rename",
        "_darwin_art_bionic_socket_broker_close",
        "_darwin_art_bionic_socket_broker_read",
        "_darwin_art_bionic_socket_broker_write",
        "_darwin_art_bionic_stat",
        "_darwin_art_bionic_statvfs",
        "_darwin_art_bionic_symlink",
        "_darwin_art_bionic_unlinkat",
        "_darwin_art_bionic_utimensat",
        "_darwin_art_bionic_write",
    ] {
        linker.arg(format!("-Wl,-exported_symbol,{symbol}"));
    }
    super::art_test_exports::apply(&mut linker);
    let description = describe_command(&linker);
    let link_stamp = build_dir.join("runtime-link.fingerprint");
    let output = link_with_cache(&mut linker, &runtime_library, &link_stamp)?;
    validate_runtime_link(&build_dir, &runtime_library, output, &description)?;
    build_runtime_host(root)
}
