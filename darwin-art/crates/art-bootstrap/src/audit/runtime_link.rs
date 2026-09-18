use super::common::{build_runtime_native_owner, require_file};
use super::graphics_core_probes::{
    RuntimeCoreObjects, compile_runtime_core_objects, core_probe_includes,
};
use super::runtime_link_checks::validate_runtime_link;
use super::*;

pub(crate) fn audit_runtime_link(root: &Path) -> Result<()> {
    let framework_vm_provider = super::java_vm_provider::build_framework_java_vm_provider(root)?;
    // Refresh the genuine pinned registrar; never substitute a local table or
    // link the entire policy archive over the existing native-loader owner.
    build_shell_gate(root, "build-android16-native-loader-policy.sh")?;
    let classloader_factory_jni =
        root.join("_build/native-loader-policy/classloader_factory_jni.o");
    require_file(
        &classloader_factory_jni,
        "ClassLoaderFactory registrar is missing",
    )?;
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
    build_shell_gate(root, "build-android16-input-keymaps.sh")?;
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
    let surface_object = build_dir.join("darwin_surface_bridge.mm.o");
    let document_panel_object = build_dir.join("document_panel.mm.o");
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
    build_shell_gate(root, "build-bionic-runtime-provider-closure.sh")?;
    let runtime_native_owner_archive = build_runtime_native_owner(root)?;
    let includes = core_probe_includes(root, &build_paths, &runtime);
    let include_refs = includes.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let probe_cache = build_dir.join("runtime-link-probe-hashes.cache");
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
    let registration_object = compile_native_registration(
        root,
        &build_dir,
        &include_refs,
        &probe_cache,
        &compiler_identity,
        &[],
    )?;
    let graphics_state_object = if let Some(path) =
        env::var_os("DARWIN_ART_NATIVE_GRAPHICS_STATE_OBJECT")
        && Path::new(&path).is_file()
    {
        PathBuf::from(path)
    } else {
        compile_runtime_graphics_state_cpu(
            root,
            &build_dir,
            &include_refs,
            &ndk_include,
            &ndk_arch_include,
        )?
    };
    let graphics_session_object = compile_runtime_graphics_session_cpu(
        root,
        &build_dir,
        &include_refs,
        &ndk_include,
        &ndk_arch_include,
    )?;
    let event_ingress_object = compile_runtime_event_ingress(root, &build_dir, &include_refs)?;
    let vsync_source_object = compile_runtime_vsync_source(root, &build_dir, &include_refs)?;
    let context_loader_object = compile_system_class_loader(root, &build_dir, &include_refs)?;
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
        &probe_cache,
        &compiler_identity,
    )?;

    // Keep the headless libart boundary on the same owner-filtered policy as
    // the graphics image.  A linker's old hand-written C++ list hid genuine
    // AOSP provider ABI and made the fixture depend on whichever archive had
    // happened to be linked first.  Export only definitions from the actual
    // owning archives; Darwin cross-image ABI remains in the narrow shared
    // inventories below.
    let art_export_list = build_dir.join("aosp-libart.exports");
    use super::provider_export_policy::{ProviderClass, allows, validate_bionic_imports};
    validate_bionic_imports(&command_output(
        Command::new("nm").arg("-u").arg(&openjdk_named_jni_owner),
    )?)?;
    let art_export_providers = [
        (
            root.join("_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a"),
            ProviderClass::MixedArt,
        ),
        (
            root.join("_build/runtime-core/libart-core-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        (
            root.join("_build/foundation/libartbase-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        (
            root.join("_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"),
            ProviderClass::PinnedUpstream,
        ),
        (
            root.join("_build/nativehelper-foundation/libnativehelper_jvm.a"),
            ProviderClass::PinnedUpstream,
        ),
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
            .filter(|symbol| !symbol.contains('$'))
            .filter(|symbol| {
                !symbol.contains("GLOBAL__sub_I_") && !symbol.contains("cxx_global_var_init")
            })
            .map(str::to_owned);
        art_exports.extend(provider_exports);
    }
    // JNI methods are discovered by ART rather than ordinary relocations. The
    // complete pinned NIO owner is force-loaded below, and its exact JNI
    // names form part of this shared provider inventory.
    let nio_symbols =
        command_output(Command::new("nm").args(["-gU"]).arg(root.join(
            "_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a",
        )))?;
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
    art_exports.sort_unstable();
    art_exports.dedup();
    if art_exports.is_empty() {
        return Err("pinned headless libart bootstrap has no external exports".into());
    }
    fs::write(&art_export_list, format!("{}\n", art_exports.join("\n")))?;

    let mut linker = Command::new("clang++");
    linker
        .arg("-dynamiclib")
        .arg(format!(
            "-Wl,-map,{}",
            build_dir.join("runtime-link.map").display()
        ))
        .arg("-Wl,-install_name,@rpath/libdarwin_art_runtime.dylib")
        .args(["-Xlinker", "-exported_symbols_list", "-Xlinker"])
        .arg(&art_export_list)
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
        .arg("-Wl,-exported_symbol,_darwin_art_surface_set_input_sink")
        .arg("-Wl,-exported_symbol,_darwin_art_android_input_sink_install")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_destroy")
        .arg("-Wl,-exported_symbol,_darwin_art_surface_active_gpu")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_install_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_fd_inheritance_boundary")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_install_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_uninstall_scm_endpoint_provider")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_socket_broker_is_active")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_clear_hooks")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_install_configured")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_process_state_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_install")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_export_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_export_retained_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_release_export_lease")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_import_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_binder_close_file_descriptor")
        .arg("-Wl,-exported_symbol,_darwin_art_bionic_fs_process_uninstall")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_acquire")
        .arg("-Wl,-exported_symbol,_darwin_art_provider_native_release")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_create")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_attach")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_lookup")
        .arg("-Wl,-exported_symbol,_darwin_art_runtime_native_owner_destroy")
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
        .arg(&process_entry_object)
        .arg(&vm_bootstrap_object)
        .arg(&process_state_object)
        .arg(&process_config_object)
        .arg(&process_shutdown_object)
        .arg(&vm_shutdown_object)
        .arg(&app_process_shutdown_object)
        .arg(&registration_object)
        .arg(&boot_native_registration_object)
        .arg(&boot_native_libraries_object)
        .arg(&graphics_state_object)
        .arg(&context_loader_object)
        .arg(&graphics_session_object)
        .arg(&event_ingress_object)
        .arg(&vsync_source_object)
        .arg(&surface_object)
        .arg(&document_panel_object)
        .arg(&framework_vm_provider)
        .arg(&classloader_factory_jni)
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
        .arg(root.join("_build/android16-input-keymaps/libandroid-input-keymaps.a"))
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
        .arg(root.join(
            "_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a",
        ))
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
    // This OpenJDK System.log entrypoint is not part of the Bionic provider
    // inventory. Keep its reviewed JNI ABI explicit; all Darwin Bionic
    // exports come solely from the owner-filtered symbols list above.
    linker.arg("-Wl,-exported_symbol,_Java_java_lang_System_log");
    super::art_test_exports::apply_runtime_abi(&mut linker);
    super::embedding_exports::apply(&mut linker);
    super::native_client_exports::apply(&mut linker, false);
    let description = describe_command(&linker);
    let link_stamp = build_dir.join("runtime-link.fingerprint");
    let output = link_with_cache(&mut linker, &runtime_library, &link_stamp)?;
    validate_runtime_link(&build_dir, &runtime_library, output, &description)?;
    // Publish the exact successful owner closure for separately linked native
    // acceptance images. No fixture source or export enters this product.
    crate::native_link_recipe::write_successful_recipe(
        &build_dir.join("runtime-link.argv"),
        &linker,
    )?;
    build_runtime_host(root)
}
