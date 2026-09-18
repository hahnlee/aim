use super::common::{build_runtime_native_owner, require_file};
use super::graphics_core_probes::{
    RuntimeCoreObjects, compile_runtime_core_objects, core_probe_includes,
};
use super::graphics_link_checks::validate_graphics_runtime_link;
use super::graphics_link_inputs::GraphicsRuntimeInputs;
use super::graphics_phases::run_graphics_upstream_gates;
use super::graphics_surface::compile_surface_objects;
use super::*;
pub(crate) fn audit_runtime_graphics_link(root: &Path) -> Result<()> {
    audit_runtime_graphics_link_mode(root, true, false)
}

/// Validate/link against already-built graphics inputs without rerunning the
/// long upstream closure scripts. This is the inner-loop target after a
/// narrow TU change; the full command remains the release/CI gate.
pub(crate) fn audit_runtime_graphics_link_fast(root: &Path) -> Result<()> {
    audit_runtime_graphics_link_mode(root, false, false)
}

/// Build the developer graph through its final dylib/symbol-check edge.
///
/// Ninja owns invalidation here: an unchanged tree is a no-op, a narrow TU
/// edit recompiles only that object, and an input affecting the final closure
/// relinks and runs the fast symbol checks. Expensive source-pinned upstream
/// audits remain exclusive to `audit-runtime-graphics-link` (the release/CI
/// gate) instead of running after every local edit.
pub(crate) fn audit_runtime_graphics_link_incremental(root: &Path) -> Result<()> {
    build_native_graph(root, "graphics-audit")
}

pub(crate) fn audit_runtime_graphics_link_mode(
    root: &Path,
    run_upstream_gates: bool,
    incremental: bool,
) -> Result<()> {
    // Fast links must refresh the same canonical JavaVM provider too; archive
    // presence alone does not establish source/compiler identity.
    build_framework_java_vm_provider(root)?;
    // The final dylib embeds libartbase and exports its production C++ ABI.
    // Rebuild that provider through its dependency cache before linking so a
    // compile-contract change (for example ART_STATIC_LIBARTBASE) cannot be
    // hidden behind an otherwise fresh-looking archive.
    build_foundation(root)?;
    let unwindstack_core = build_runtime_unwindstack_core(root)?;
    let unwindstack_dex = build_runtime_unwindstack_dex(root)?;
    let unwindstack_providers =
        root.join("_build/runtime-unwindstack/libunwindstack-mach-providers.a");
    let rust_demangle = root.join(
        "_build/runtime-unwindstack/rust-demangle-target/release/libdarwin_art_rust_demangle.a",
    );
    // dex2oat and the final runtime dylib force-load the compiler archive.
    // Build that producer edge here so compiler patches cannot be hidden by a
    // stale archive from an earlier explicit `build-jit-compiler` invocation.
    build_jit_compiler(root)?;
    let dex2oat_archive = build_dex2oat(root)?;
    let elf_loader = build_elf_loader(root)?;
    run_command(
        Command::new("bash")
            .arg(root.join("tools/android-managed-native-load/audit.sh"))
            .arg("--build-only"),
    )?;
    if run_upstream_gates {
        run_graphics_upstream_gates(root, incremental)?;
    }
    let runtime_native_owner_archive = build_runtime_native_owner(root)?;
    build_shell_gate(root, "build-android16-system-properties.sh")?;
    build_shell_gate(root, "build-android16-binder-jni.sh")?;
    build_shell_gate(root, "build-android16-input-keymaps.sh")?;
    run_command(
        Command::new("bash").arg(root.join("tools/build-android16-openjdkjvmti-darwin.sh")),
    )?;

    let runtime = root.join("_aosp/art/runtime");
    let build_paths = BuildPaths::from_root(root);
    let build_dir = build_paths.native_output("runtime-graphics-link-probe");
    let runtime_library = build_dir.join("libdarwin_art_runtime_graphics.dylib");
    let GraphicsRuntimeInputs {
        graphics_closure,
        bootstrap,
        icu_jni_archive,
        libcore_linux_archive,
        os_constants_archive,
        unix_filesystem_archive,
        openjdkjvm_archive,
        openjdkjvmti_archive,
        managed_load_archive,
        file_input_stream_archive,
        file_descriptor_archive,
        system_natives_archive,
        boringssl_crypto_archive,
        unix_native_dispatcher_archive,
        fdlibm_archive,
        openjdk_nio_mapping_archive,
        openjdk_nio_support_archive,
        libcore_memory_archive,
        libcore_jni_constants_archive,
        asynchronous_close_registrar,
        asynchronous_close_backend,
        resource_jni_archive,
        android_util_log_archive,
        application_shared_memory_archive,
        debugstore_archive,
        activity_thread_archive,
        bionic_dlwarning_object,
        classloader_factory_jni,
        trace_archive,
        perfetto_library,
        virtual_ref_base_ptr_archive,
        android_runtime_host,
    } = GraphicsRuntimeInputs::load(root, &build_paths)?;

    fs::create_dir_all(&build_dir)?;
    // Boot JavaVMExt must own a separate RTLD_LOCAL image for AOSP OpenJDK's
    // named-JNI methods. The aggregate runtime image also contains framework
    // and test Java_* entrypoints and is intentionally not registered as a
    // boot library.
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
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    // Keep the process probe flavor-neutral. The linked compatibility object is
    // the sole owner of DARWIN_ART_REAL_GRAPHICS and chooses the real backend.
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let probe_cache = build_dir.join("runtime-graphics-probe-hashes.cache");
    let core_build_dir = build_paths.native_output("native-runtime/core");
    fs::create_dir_all(&core_build_dir)?;
    let core_probe_cache = core_build_dir.join("core-probe-hashes.cache");
    let RuntimeCoreObjects {
        boot_native_registration: boot_native_registration_object,
        boot_native_libraries: boot_native_libraries_object,
        vm_bootstrap: vm_bootstrap_object,
        process_entry: process_entry_object,
        process_state: process_state_object,
        process_config: process_config_object,
        process_shutdown: process_shutdown_object,
        vm_shutdown: vm_shutdown_object,
        app_process_shutdown: app_process_shutdown_object,
    } = compile_runtime_core_objects(
        root,
        &core_build_dir,
        &include_refs,
        &core_probe_cache,
        &compiler_identity,
    )?;
    let event_ingress_object = compile_runtime_event_ingress(root, &build_dir, &include_refs)?;
    let vsync_source_object = compile_runtime_vsync_source(root, &build_dir, &include_refs)?;
    let graphics_state_object = if let Some(path) =
        env::var_os("DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT")
        && Path::new(&path).is_file()
    {
        PathBuf::from(path)
    } else {
        compile_runtime_graphics_state(
            root,
            &build_dir,
            &include_refs,
            &ndk_include,
            &ndk_arch_include,
        )?
    };
    let graphics_session_object_real = compile_runtime_graphics_session(
        root,
        &build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    let registration_object = compile_native_registration(
        root,
        &build_dir,
        &include_refs,
        &probe_cache,
        &compiler_identity,
        &["-DDARWIN_ART_REAL_GRAPHICS", "-DDARWIN_ART_HWUI_GPU"],
    )?;
    let context_loader_object = compile_system_class_loader(root, &build_dir, &include_refs)?;
    let (surface_object, surface_gpu_object, document_panel_object) =
        compile_surface_objects(root, &build_dir, &probe_cache, &compiler_identity)?;

    // Android's libart is a shared-library boundary for the in-APEX
    // libarttest/JVMTI clients.  An ld64 `-exported_symbol` option turns the
    // complete dylib export table into an allowlist, so the small host ABI
    // list below used to hide ordinary AOSP C++ definitions. Export genuine
    // provider-owned definitions, unioned with exact Darwin cross-image ABI.
    // Mixed archives must not implicitly publish host/fixture observations.
    // The graphics bootstrap archive
    // is not the whole provider boundary: runtime-core owns synchronization
    // primitives and libartbase owns allocators used directly by unchanged
    // AOSP native run-tests.  Keeping the provider set here (at the
    // production dylib boundary) avoids test-specific link shims.
    let art_export_list = build_dir.join("aosp-libart.exports");
    use super::provider_export_policy::{ProviderClass, allows, validate_bionic_imports};
    validate_bionic_imports(&command_output(
        Command::new("nm").arg("-u").arg(&openjdk_named_jni_owner),
    )?)?;
    let art_export_providers = [
        (bootstrap.clone(), ProviderClass::MixedArt),
        (
            root.join("_build/runtime-core/libart-core-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        (
            root.join("_build/foundation/libartbase-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        // AOSP libopenjdk declares libopenjdkjvm as a shared dependency.  The
        // Darwin aggregate is the process libart/libopenjdkjvm provider, so
        // publish that pinned module's complete public JVM_/jio_ ABI for the
        // sibling RTLD_LOCAL named-JNI image instead of cloning its TU there.
        (openjdkjvm_archive.clone(), ProviderClass::PinnedUpstream),
        // libopenjdk_native_defaults also declares libnativehelper#impl as a
        // shared dependency.  Keep that genuine provider visible across the
        // same process boundary; jni_util must not rely on a private symbol
        // accidentally present in the aggregate image.
        (
            root.join("_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        // OpenJDK's RTLD_LOCAL named-JNI owner is loaded after libart.  Its
        // NIO implementation is the AOSP module closure and must resolve the
        // process-wide Bionic facade ABI through the runtime boundary, not
        // through ad-hoc per-owner copies or test-specific stubs.
        (
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-native-providers.a",
            ),
            ProviderClass::DarwinBionic,
        ),
        (
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a",
            ),
            ProviderClass::DarwinBionic,
        ),
        (
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-float-conversion.a",
            ),
            ProviderClass::DarwinBionic,
        ),
    ];
    let mut art_exports = Vec::new();
    for (provider, class) in &art_export_providers {
        let provider_symbols = command_output(Command::new("nm").args(["-gU"]).arg(provider))?;
        let provider_exports = provider_symbols
            .lines()
            .filter_map(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                let kind = fields.get(fields.len().saturating_sub(2)).copied();
                let symbol = fields.last().copied();
                match (kind, symbol) {
                    (Some(kind), Some(symbol))
                        if kind.len() == 1 && "TtDdSsBbCcWwVv".contains(kind) =>
                    {
                        Some(symbol)
                    }
                    _ => None,
                }
            })
            .filter(|symbol| symbol.starts_with('_'))
            .filter(|symbol| allows(*class, symbol))
            // Rust provider archives also contain compiler-builtins/std
            // implementation members. They are not standalone shared-ABI
            // definitions and must not become forced linker roots.
            .filter(|symbol| !symbol.contains('$'))
            // Do not turn archive-local C++ static initializers into part of
            // the shared ABI.  In particular libartbase's globals_unix.cc
            // constructor asserts that a separately loaded Android
            // libartbase.dylib exists; Darwin embeds this provider in libart
            // instead, so retaining that initializer would make every host
            // runtime fail before it can execute the test client.
            .filter(|symbol| {
                !symbol.contains("GLOBAL__sub_I_") && !symbol.contains("cxx_global_var_init")
            })
            .map(str::to_owned);
        art_exports.extend(provider_exports);
    }
    // JNI entrypoints are intentionally discovered by ART at runtime, so they
    // have no ordinary relocation from the image. Keep the archive force-
    // loaded below and derive its complete JNI export surface from the
    // provider itself. This is one generic contract for every OpenJDK native
    // method; it must not grow one `-u` linker root per application or test.
    let nio_symbols = command_output(
        Command::new("nm")
            .args(["-gU"])
            .arg(&unix_native_dispatcher_archive),
    )?;
    let nio_jni_exports = nio_symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|symbol| symbol.starts_with("_Java_"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if nio_jni_exports.is_empty() {
        return Err("OpenJDK NIO archive has no JNI entrypoints".into());
    }
    art_exports.extend(nio_jni_exports);
    art_exports.extend(super::graphics_bitmap::bitmap_exports(root)?);
    art_exports.sort_unstable();
    art_exports.dedup();
    if art_exports.is_empty() {
        return Err("pinned libart bootstrap has no external exports".into());
    }
    fs::write(&art_export_list, format!("{}\n", art_exports.join("\n")))?;

    let link_map = build_dir.join("runtime-graphics-link.map");
    let mut linker = Command::new("clang++");
    linker
        .arg("-dynamiclib")
        .arg("-Wl,-install_name,@rpath/libdarwin_art_runtime_graphics.dylib")
        // Pass the list as a distinct linker argument so link_with_cache can
        // fingerprint the file itself (the comma-joined -Wl spelling hides
        // the path from its input metadata walk).
        .args(["-Xlinker", "-exported_symbols_list", "-Xlinker"])
        .arg(&art_export_list)
        .arg("-Wl,-exported_symbol,_darwin_art_run_process")
        .arg("-Wl,-exported_symbol,_darwin_art_run_dex2oat")
        .arg("-Wl,-exported_symbol,_darwin_art_run_profman")
        .arg("-Wl,-exported_symbol,_darwin_art_shutdown_process")
        .arg("-Wl,-exported_symbol,_darwin_art_dispatch_pointer")
        .arg("-Wl,-exported_symbol,_darwin_art_unwindstack_quick_frames")
        .arg("-Wl,-exported_symbol,_darwin_art_walk_managed_frames")
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
        .arg("-Wl,-exported_symbol,_darwin_art_android_input_sink_install")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_destroy")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_active_gpu")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_gpu_active_canvas")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_install_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_fd_inheritance_boundary")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_uninstall_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_socket_broker_is_active")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_clear_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_install_configured")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_install")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_export_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_export_retained_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_release_export_lease")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_import_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_close_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_acquire")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_release")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_create")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_attach")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_lookup")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_destroy")
        // The host invokes this narrow lifecycle hook immediately before an
        // Android-style process exit.  Keep it rooted despite dead stripping
        // and expose the C ABI from the production graphics runtime image.
        .arg("-Wl,-u,_darwin_art_prepare_process_exit")
        .arg("-Wl,-exported_symbol,_darwin_art_prepare_process_exit")
        .arg("-Wl,-exported_symbol,___jit_debug_descriptor")
        .arg("-Wl,-exported_symbol,___dex_debug_descriptor")
        .arg("-Wl,-exported_symbol,_ArtPlugin_Initialize")
        .arg("-Wl,-exported_symbol,_ArtPlugin_Deinitialize")
        // Runtime::AttachAgent is an AOSP EXPORT surface consumed by
        // libarttest/libtiagent. The explicit Darwin export list must retain
        // that public ART test-agent ABI instead of hiding it accidentally.
        .arg("-Wl,-exported_symbol,__ZN3art7Runtime11AttachAgentEP7_JNIEnvRKNSt3__112basic_stringIcNS3_11char_traitsIcEENS3_9allocatorIcEEEEP8_jobject")
        // libarttest is an AOSP runtime-test client of these ART APIs. Keep
        // their original C++ ABI visible so tests such as 566 can inspect the
        // real JIT code cache without a Darwin-only replacement JNI method.
        .arg("-Wl,-exported_symbol,__ZN3art3jit3Jit13JitAtFirstUseEv")
        .arg("-Wl,-exported_symbol,__ZN3art6mirror5Class30FindDeclaredDirectMethodByNameENSt3__117basic_string_viewIcNS2_11char_traitsIcEEEENS_11PointerSizeE")
        .arg("-Wl,-exported_symbol,__ZN3art8CodeInfoC1EPKNS_20OatQuickMethodHeaderE")
        .arg("-Wl,-exported_symbol,__ZNK3art3jit12JitCodeCache10ContainsPcEPKv")
        .arg("-Wl,-exported_symbol,_EnsureFrontOfChain")
        .arg("-Wl,-exported_symbol,_darwin_art_sigchain_owns_signal")
        .arg("-Wl,-exported_symbol,_darwin_art_sigchain_sigaction")
        // Converted Mach-O JNI modules observe Android's logical pthread name
        // even when Darwin cannot rename a different host pthread directly.
        .arg("-Wl,-u,_darwin_art_pthread_getname_np")
        .arg("-Wl,-exported_symbol,_darwin_art_pthread_getname_np")
        .arg("-Wl,-u,_darwin_art_pthread_setname_np")
        .arg("-Wl,-exported_symbol,_darwin_art_pthread_setname_np")
        // The named-JNI owner reaches these TLS errno adapters through
        // indirect NIO calls, so retain them as real runtime ABI roots.
        .arg("-Wl,-u,_darwin_art_bionic_errno_load")
        .arg("-Wl,-u,_darwin_art_bionic_errno_store")
        .arg("-Wl,-u,_darwin_art_bionic_errno_set_from_darwin")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_errno_load")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_errno_store")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_errno_set_from_darwin")
        .arg("-Wl,-dead_strip")
        .arg(format!("-Wl,-map,{}", link_map.display()))
        .arg(&process_entry_object)
        .arg(&vm_bootstrap_object)
        .arg(&registration_object)
        .arg(&boot_native_registration_object)
        .arg(&boot_native_libraries_object)
        .arg(&process_state_object)
        .arg(&process_config_object)
        .arg(&process_shutdown_object)
        .arg(&vm_shutdown_object)
        .arg(&app_process_shutdown_object)
        .arg(&graphics_state_object)
        .arg(&context_loader_object)
        .arg(&graphics_session_object_real)
        .arg(&event_ingress_object)
        .arg(&vsync_source_object)
        .arg(&surface_object)
        .arg(&surface_gpu_object)
        .arg(&document_panel_object)
        // The relocatable graphics closure is the sole owner of HWUI's
        // process-singleton pools; resolve it before optional providers.
        .arg(&graphics_closure)
        // SurfaceControl transactions cross the exact Android 16
        // TransactionHandler/ResolvedComposerState boundary before the
        // Darwin Composer consumes them. Keep the frontend and its AOSP
        // Binder/libgui/Fence closure in the production graphics dylib.
        .arg(root.join("_build/surfaceflinger-core/libsurfaceflinger-frontend-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libgui-transaction-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libbinder-darwin.a"))
        .arg(root.join("_build/surfaceflinger-core/libui-fence-darwin.a"))
        .arg(root.join("_build/skia-metal-gpu/libskia.a"))
        .arg(root.join("_build/skia-metal-gpu/libskcms.a"))
        // The relocatable graphics closure already force-loads the complete
        // HWUI archive. Do not add the archive again here: CommonPool and
        // RenderThread contain process-singleton state, and a second archive
        // edge would create a distinct pool that shutdown cannot join.
        .arg(root.join("_build/android-graphics-jni/libandroid-graphics-jni-darwin.a"))
        // HWUI is an Android EGL/GLES client.  Keep its standard C ABI bound
        // to the project-built ANGLE dylibs instead of manufacturing another
        // static GL implementation in the compatibility archive.  Android-
        // specific ANativeWindow/AHardwareBuffer behavior remains in the
        // platform dispatch bridge linked below.
        .arg(root.join("_build/angle-source/out/DarwinArtRelease/libEGL.dylib"))
        .arg(root.join("_build/angle-source/out/DarwinArtRelease/libGLESv2.dylib"))
        // Keep OpenJDK JVMTI in the same ART image on Darwin. Its implementation
        // consumes ART-private symbols that are hidden across Android DSOs as
        // part of one APEX closure; embedding the archive preserves that
        // ownership without exporting the entire Runtime C++ ABI.
        .arg(format!(
            "-Wl,-force_load,{}",
            openjdkjvmti_archive.display()
        ))
        .arg(&bootstrap)
        // AOSP Java Binder/Parcel and the Darwin RPC transport boundary are
        // separate owners. They follow the runtime registrar archive, then
        // libbinder is rescanned to satisfy JNI members selected here.
        .arg(root.join("_build/binder-jni/libbinder-jni-darwin.a"))
        .arg(root.join("_build/android16-input-keymaps/libandroid-input-keymaps.a"))
        .arg(root.join("_build/binder-jni/libbinder-rpc-boundary-darwin.a"))
        // Rust's process Binder endpoint resolves these three narrow Bionic
        // descriptor hooks with dlsym.  They have no static caller, so link
        // their single boundary object directly instead of force-loading the
        // unrelated RPC archive members.
        .arg(root.join("_build/binder-jni/fd-transport.o"))
        .arg(root.join("_build/surfaceflinger-core/libbinder-darwin.a"))
        // DexFiles discovers this AOSP descriptor through dlsym; retain only
        // the debugger-interface member instead of force-loading the runtime
        // archive (which duplicates ICU/ART providers).
        .arg(root.join("_build/runtime-common/objects/jit_debugger_interface.cc.o"))
        .arg(root.join("_build/runtime-common/objects/darwin_art_stack_resolver.cc.o"))
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
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-float-conversion.a",
            )
            .display()
        ))
        .arg(
            root.join(
                "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a",
            ),
        )
        .arg(root.join("_build/icu-foundation/libandroidicuinit-darwin.a"))
        .arg(&elf_loader)
        .arg(format!(
            "-Wl,-force_load,{}",
            runtime_native_owner_archive.display()
        ))
        .arg(&system_natives_archive)
        .arg(&boringssl_crypto_archive)
        .arg(&file_descriptor_archive)
        // Keep the complete OpenJDK NIO owner resident. UnixCopyFile.transfer
        // is discovered by ART's JNI lookup rather than referenced by the
        // link graph, so a normal archive edge would dead-strip its object.
        // Keep the archive path as a distinct linker argument so the native
        // link fingerprint observes archive replacement as an input change.
        .args(["-Xlinker", "-force_load", "-Xlinker"])
        .arg(&unix_native_dispatcher_archive)
        .arg(&fdlibm_archive)
        .arg(&file_input_stream_archive)
        .arg(&openjdk_nio_mapping_archive)
        .arg(&openjdk_nio_support_archive)
        .arg(&libcore_memory_archive)
        .arg(&libcore_jni_constants_archive)
        .arg(&unix_filesystem_archive)
        // Complete java.lang.Runtime owner must precede openjdkjvm, which
        // supplies its JVM_* support symbols.
        .arg(&managed_load_archive)
        .arg(&openjdkjvm_archive)
        .arg(&os_constants_archive)
        .arg(&android_util_log_archive)
        .arg(&application_shared_memory_archive)
        .arg(&debugstore_archive)
        .arg(&activity_thread_archive)
        .arg(&bionic_dlwarning_object)
        .arg(&classloader_factory_jni)
        .arg(&trace_archive)
        .arg(&perfetto_library)
        .arg(&virtual_ref_base_ptr_archive)
        .arg(format!(
            "-Wl,-force_load,{}",
            resource_jni_archive.display()
        ))
        .arg(&android_runtime_host)
        .arg(&libcore_linux_archive)
        .arg(&asynchronous_close_registrar)
        .arg(&asynchronous_close_backend)
        .arg(root.join("_build/interpreter-core/libart-interpreter-darwin.a"))
        .arg(format!("-Wl,-force_load,{}", dex2oat_archive.display()))
        .arg(root.join("_build/jit-compiler/libart-compiler-darwin.a"))
        .arg(root.join("_build/jit-compiler/libart-libelffile-darwin.a"))
        .arg(root.join("_build/runtime-arm64/libart-arm64-darwin.a"))
        .arg(root.join("_build/runtime-core/libart-core-darwin.a"))
        .arg(root.join("_build/runtime-platform/libart-platform-darwin.a"))
        // libunwindstack's dex adapter references the external ADexFile ABI
        // through indirection, so ld64 cannot discover these roots while
        // scanning normally. Force-load the pinned AOSP libdexfile provider
        // just as the Android APEX dependency does.
        .arg(format!(
            "-Wl,-force_load,{}",
            root.join("_build/dex-probe/libdexfile-darwin.a").display()
        ))
        .arg(
            build_paths
                .native_output("runtime-graphics-bootstrap/objects/artbase_os_linux_aosp_fmt.cc.o"),
        )
        .arg(root.join("_build/foundation/libartbase-darwin.a"))
        .arg(root.join("_build/ziparchive-incfs/libziparchive-for-incfs-darwin.a"))
        .arg(root.join("_build/graphics-foundations/liblog-darwin.a"))
        .arg(format!("-Wl,-force_load,{}", icu_jni_archive.display()))
        // ld64 does not rescan archives that appeared before the force-loaded
        // resource/ICU roots. Re-supply their complete Android.bp providers in
        // dependency order; normal archive extraction prevents duplicates with
        // the already-composed graphics closure.
        .arg(root.join("_build/androidfw-foundation/libandroidfw-darwin.a"))
        .arg(root.join("_build/ui-types-foundation/libui-types.a"))
        .arg(root.join("_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"))
        .arg(root.join("_build/system-properties/libsystem-properties-jni-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libutils-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libutils-binder-darwin.a"))
        .arg(root.join("_build/graphics-foundations/libcutils-darwin.a"))
        .arg(root.join("_build/graphics-foundations/liblog-darwin.a"))
        .arg(root.join("_build/libbase-foundation/libandroid-base-darwin.a"))
        .arg(root.join("_build/ziparchive-incfs/libziparchive-for-incfs-darwin.a"))
        .arg(root.join("_build/foundation/libziparchive-darwin.a"))
        .arg(root.join("_build/icu-foundation/libicui18n-darwin.a"))
        .arg(root.join("_build/icu-foundation/libicuuc-common-darwin.a"))
        .arg(root.join("_build/icu-foundation/libicuuc-stubdata-darwin.a"))
        .arg(root.join("_build/graphics-codecs/libpng-darwin.a"))
        .arg(root.join("_build/graphics-codecs/libz-darwin.a"))
        .args([
            "-L/opt/homebrew/lib",
            "-llz4",
            "-lzstd",
            "-lsqlite3",
            "-lz",
            "-lresolv",
            "-Wl,-rpath,@loader_path/../angle-source/out/DarwinArtRelease",
            "-Wl,-rpath,@loader_path/../tracing-perfetto/perfetto-out",
            "-framework",
            "CoreFoundation",
            "-framework",
            "CoreGraphics",
            "-framework",
            "ImageIO",
            "-framework",
            "Foundation",
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
    super::art_test_exports::apply_runtime_abi(&mut linker);
    super::embedding_exports::apply(&mut linker);
    super::native_client_exports::apply(&mut linker, true);
    let description = describe_command(&linker);
    let link_stamp = build_dir.join("runtime-graphics-link.fingerprint");
    let output = link_with_cache(&mut linker, &runtime_library, &link_stamp)?;
    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr)?;
        fs::write(build_dir.join("link.err"), &stderr)?;
        return Err(format!("real-graphics Runtime link failed: {description}\n{stderr}").into());
    }

    validate_graphics_runtime_link(root, &runtime_library, &openjdk_named_jni_owner, &link_map)?;
    build_runtime_host(root)?;
    println!(
        "audit-runtime-graphics-link: closure complete registrar=51 fake-symbols=0 host-icu=0 host-fmt=0 CoreText=0"
    );
    Ok(())
}
