use std::fs;
use std::path::Path;
use std::process::Command;

use crate::Result;
use crate::support::command_output;

pub(super) fn validate_graphics_runtime_link(
    root: &Path,
    runtime_library: &Path,
    openjdk_named_jni_owner: &Path,
    link_map: &Path,
) -> Result<()> {
    let global_symbols = command_output(Command::new("nm").args(["-gU"]).arg(runtime_library))?;
    for required in [
        "_darwin_art_run_process",
        "_darwin_art_shutdown_process",
        "_darwin_art_dispatch_pointer",
        "_darwin_art_pump_framework_frame",
        "_darwin_art_graphics_session_create",
        "_darwin_art_graphics_session_close",
        "_darwin_art_graphics_session_destroy",
        "_darwin_art_graphics_session_dispatch_pointer",
        "_darwin_art_graphics_session_dispatch_pointer_v2",
        "_darwin_art_graphics_session_dispatch_key_v1",
        "_darwin_art_graphics_session_pump_frame",
        "_darwin_art_graphics_session_pump_main_looper",
        "_darwin_art_surface_create",
        "_darwin_art_surface_resize",
        "_darwin_art_surface_get_size",
        "_darwin_art_surface_update",
        "_darwin_art_surface_map_producer",
        "_darwin_art_surface_unmap_producer",
        "_darwin_art_surface_present",
        "_darwin_art_surface_present_async",
        "_darwin_art_surface_pump_events",
        "_darwin_art_surface_close_requested",
        "_darwin_art_appkit_pump_events",
        "_darwin_art_surface_set_input_sink",
        "_darwin_art_android_input_sink_install",
        "_darwin_art_surface_destroy",
        "_darwin_art_provider_install_hooks",
        "_darwin_art_bionic_install_fd_inheritance_boundary",
        "_darwin_art_bionic_install_scm_endpoint_provider",
        "_darwin_art_bionic_uninstall_scm_endpoint_provider",
        "_darwin_art_bionic_socket_broker_is_active",
        "_darwin_art_provider_clear_hooks",
        "_darwin_art_provider_native_acquire",
        "_darwin_art_provider_native_release",
        "_darwin_art_runtime_native_owner_create",
        "_darwin_art_runtime_native_owner_attach",
        "_darwin_art_runtime_native_owner_lookup",
        "_darwin_art_runtime_native_owner_destroy",
        "_jniRegisterNativeMethods",
    ] {
        if !global_symbols.contains(required) {
            return Err(format!("real-graphics Runtime lacks required symbol {required}").into());
        }
    }
    let all_symbols = command_output(Command::new("nm").args(["-aC"]).arg(runtime_library))?;
    // These are internal Rust/native transport calls, not public dylib ABI.
    for required in [
        "_darwin_art_service_endpoint_open",
        "_darwin_art_service_endpoint_close",
        "_darwin_art_service_endpoint_ready",
        "_darwin_art_service_endpoint_lost",
    ] {
        if !all_symbols.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            matches!(fields.as_slice(), [_, "t" | "T", name] if *name == required)
        }) {
            return Err(format!("real-graphics Runtime lacks endpoint owner {required}").into());
        }
    }
    // Dynamic JNI lookup is the Android contract for OpenJDK native methods:
    // the linker must retain and export the complete provider surface even
    // though no image relocation references those entrypoints. The aggregate
    // runtime is deliberately not the owner: it also carries test/framework
    // Java_* symbols. Check the dedicated owner against the source/archive
    // manifest so missing and surplus candidates both fail closed.
    let named_manifest = fs::read_to_string(
        root.join("_build/system-natives-darwin/openjdk-named-jni-exported-symbols.txt"),
    )?;
    let expected_exports = named_manifest
        .lines()
        .collect::<std::collections::HashSet<_>>();
    if expected_exports.len() != 64
        || expected_exports
            .iter()
            .any(|line| !line.starts_with("_Java_"))
        || named_manifest.lines().any(|line| line == "_JNI_OnLoad")
    {
        return Err("OpenJDK named-JNI export manifest has unexpected JNI_OnLoad/count".into());
    }
    let owner_symbols = command_output(
        Command::new("nm")
            .args(["-gU"])
            .arg(openjdk_named_jni_owner),
    )?;
    let actual_exports = owner_symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .collect::<std::collections::HashSet<_>>();
    if actual_exports != expected_exports {
        return Err("dedicated OpenJDK named-JNI owner has missing or surplus exports".into());
    }
    let openjdkjvm_manifest =
        fs::read_to_string(root.join("_build/openjdkjvm-darwin/openjdkjvm-exports.txt"))?;
    let missing_jvm_exports = openjdkjvm_manifest
        .lines()
        .map(|symbol| format!("_{symbol}"))
        .filter(|symbol| !global_symbols.lines().any(|line| line.ends_with(symbol)))
        .collect::<Vec<_>>();
    if !missing_jvm_exports.is_empty() {
        return Err(format!(
            "real-graphics Runtime dropped libopenjdkjvm exports: {}",
            missing_jvm_exports.join(", ")
        )
        .into());
    }
    let owner_undefined =
        command_output(Command::new("nm").arg("-u").arg(openjdk_named_jni_owner))?;
    if !owner_undefined
        .lines()
        .any(|line| line.trim() == "_JVM_GetLastErrorString")
    {
        return Err("dedicated OpenJDK named-JNI owner lost its libopenjdkjvm import".into());
    }
    let owner_all_symbols =
        command_output(Command::new("nm").arg("-a").arg(openjdk_named_jni_owner))?;
    if owner_all_symbols.lines().any(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        fields.last().is_some_and(|symbol| {
            (symbol.starts_with("_JVM_") || symbol.starts_with("_jio_"))
                && fields.get(fields.len().saturating_sub(2)) != Some(&"U")
        })
    }) {
        return Err(
            "dedicated OpenJDK named-JNI owner duplicates libopenjdkjvm definitions".into(),
        );
    }
    let load_smoke = runtime_library
        .parent()
        .ok_or("real-graphics Runtime has no output directory")?
        .join("openjdk-named-jni-load-smoke");
    command_output(
        Command::new("clang")
            .args(["-std=c11", "-arch", "arm64", "-Wall", "-Wextra", "-Werror"])
            .arg(root.join("probes/openjdk_named_jni_load_smoke.c"))
            .arg("-o")
            .arg(&load_smoke),
    )?;
    let load_output = command_output(
        Command::new(&load_smoke)
            .arg(runtime_library)
            .arg(openjdk_named_jni_owner),
    )?;
    if load_output.trim() != "openjdk-owner-load: RTLD_GLOBAL(runtime)->RTLD_LOCAL(owner)=PASS" {
        return Err(format!("unexpected OpenJDK owner load smoke output: {load_output}").into());
    }
    let nio_archive = root
        .join("_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a");
    let nio_symbols = command_output(Command::new("nm").args(["-gU"]).arg(&nio_archive))?;
    let missing_nio_jni = nio_symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|symbol| symbol.starts_with("_Java_"))
        .filter(|symbol| !actual_exports.contains(symbol))
        .collect::<Vec<_>>();
    if !missing_nio_jni.is_empty() {
        return Err(format!(
            "real-graphics Runtime dropped dynamically-discovered JNI exports: {}",
            missing_nio_jni.join(", ")
        )
        .into());
    }
    for registrar in [
        "_init_android_graphics",
        "_register_android_graphics_classes",
        "register_android_content_AssetManager",
        "register_android_content_StringBlock",
        "register_android_content_XmlBlock",
        "register_android_content_res_ApkAssets",
        "register_com_android_internal_util_VirtualRefBasePtr",
        "register_android_util_Log",
        "register_android_util_EventLog",
        "register_com_android_internal_content_NativeLibraryHelper",
        "darwin_art_android_runtime_install",
        "darwin_art_android_runtime_uninstall",
        "darwin_art::libcore_darwin::RegisterLinuxNatives",
        "register_android_system_OsConstants",
        "register_java_io_UnixFileSystem",
        "register_java_io_FileInputStream",
        "register_java_io_FileDescriptor",
        "register_java_lang_System",
        "register_java_sun_nio_fs_UnixNativeDispatcher",
        "register_sun_nio_ch_IOUtil",
        "register_sun_nio_ch_FileChannelImpl",
        "register_sun_nio_ch_FileDispatcherImpl",
        "register_sun_nio_ch_NativeThread",
        "darwin_art_restore_sun_nio_ch_NativeThread_signal",
        "register_libcore_io_Memory",
        "DarwinArtLibcoreJniConstants::GetPrimitiveByteArrayClass",
        "JVM_GetLastErrorString",
        "register_libcore_io_AsynchronousCloseMonitor",
        "async_close_monitor_signal_blocked_threads",
        "JniConstants_FileDescriptor_descriptor",
        "darwin_art_elf_graph_load",
        "darwin_art_elf_graph_lookup_root",
        "darwin_art_elf_graph_unload",
        "darwin_art_jni_proxy_init",
        "darwin_art_jni_proxy_java_vm",
        "darwin_art_bionic_namespace_bind_builtins",
        "darwin_art_bionic_binary128_conversion_resolve",
        "darwin_art_bionic_strtold",
        "darwin_art_bionic_strtold_l",
        "darwin_art_bionic_wcstold",
        "darwin_art_bionic_syslog_resolve",
        "darwin_art_bionic_syscall_resolve",
        "darwin_art_bionic_stdio_process_install",
        "darwin_art_bionic_stdio_process_uninstall",
        "darwin_art_bionic_formatted_stdio_resolve",
        "darwin_art_bionic_scanf_resolve",
        "darwin_art_bionic_sscanf",
        "darwin_art_bionic_vsscanf",
        "darwin_art_bionic_swprintf_resolve",
        "darwin_art_bionic_swprintf",
        "darwin_art_bionic_ioctl_resolve",
        "darwin_art_bionic_ioctl_activate",
        "darwin_art_bionic_ioctl_deactivate",
        "darwin_art_bionic_sendfile_resolve",
        "darwin_art_bionic_sendfile_activate",
        "darwin_art_bionic_sendfile_deactivate",
        "darwin_art_bionic_sendfile",
        "darwin_art_bionic_strftime_resolve",
        "darwin_art_bionic_strftime_activate",
        "darwin_art_bionic_strftime_deactivate",
        "darwin_art_bionic_wide_stdio_resolve",
        "darwin_art_bionic_fputwc",
        "darwin_art_bionic_getwc",
        "darwin_art_bionic_ungetwc",
        "darwin_art_bionic_wide_float_resolve",
        "ElfJniOnLoadTrampoline",
        "CreateRegularTrampolines",
        "TrampolineEntryMask",
        "android::uirenderer::renderthread::RenderThread::threadLoop()",
        "android::uirenderer::renderthread::RenderThread::requireGlContext()",
        "android::uirenderer::renderthread::RenderThread::requireGrContext()",
        "android::uirenderer::renderthread::RenderThread::postFrameCallback",
    ] {
        if !all_symbols.contains(registrar) {
            return Err(format!("real-graphics Runtime lacks registrar symbol {registrar}").into());
        }
    }
    for forbidden in [
        "DarwinPaint",
        "DarwinRenderNode",
        "DarwinAssetManager",
        "LogIsLoggable",
        "LogPrintln",
        "ProbeCanvas",
        "JniConstants_FileDescriptor_fd",
        "UnixFileSystemInitIds",
        "UnixFileSystemGetBooleanAttributes",
        "FileDescriptorGetAppend",
        "FileDescriptorIsSocket",
    ] {
        if all_symbols.contains(forbidden) {
            return Err(format!("real-graphics Runtime contains fake symbol {forbidden}").into());
        }
    }
    let has_icu78 = all_symbols.lines().any(|line| {
        line.split_whitespace().last().is_some_and(|symbol| {
            !symbol.starts_with("_OUTLINED_FUNCTION_") && symbol.ends_with("_78")
        })
    });
    let has_icu76 = all_symbols.lines().any(|line| {
        line.split_whitespace().last().is_some_and(|symbol| {
            !symbol.starts_with("_OUTLINED_FUNCTION_") && symbol.ends_with("_76")
        })
    });
    if has_icu78 || !has_icu76 {
        return Err("real-graphics Runtime did not retain a pure AOSP ICU76 ABI".into());
    }
    let registration_header = fs::read_to_string(
        root.join("_build/android-graphics-jni/generated/darwin_android_graphics_registration.h"),
    )?;
    if !registration_header.contains("kNativeClassCount = 51") {
        return Err("real-graphics registrar is not the verified 51-class set".into());
    }
    let dependencies = command_output(Command::new("otool").arg("-L").arg(runtime_library))?;
    for required in ["@rpath/libEGL.dylib", "@rpath/libGLESv2.dylib"] {
        if !dependencies.contains(required) {
            return Err(format!("real-graphics Runtime lacks ANGLE dependency: {required}").into());
        }
    }
    for forbidden in ["CoreText", "libicu", "libfmt", "libfreetype"] {
        if dependencies.contains(forbidden) {
            return Err(format!("forbidden real-graphics host dependency: {forbidden}").into());
        }
    }
    let link_map_contents = fs::read_to_string(link_map)?;
    // The ART runtime graphics closure is an ld -r flattened owner, so its
    // final map records android16-graphics-runtime-closure.o instead of each
    // archive member. The closure audit already pins the Android HWUI member
    // manifest; here retain the owner check while accepting that provenance.
    let has_aosp_renderthread_owner = link_map_contents
        .contains("(renderthread_RenderThread.cpp.o)")
        || (link_map_contents.contains("android16-graphics-runtime-closure.o")
            && all_symbols.contains("RenderThread"));
    if !has_aosp_renderthread_owner
        || link_map_contents.contains("(platform_host_renderthread_RenderThread.cpp.o)")
    {
        return Err("real-graphics Runtime did not link the AOSP RenderThread owner".into());
    }
    let device_info_command = fs::read_to_string(
        root.join("_build/hwui-static-foundation/objects/DeviceInfo.cpp.o.command"),
    )?;
    if device_info_command.contains("HWUI_NULL_GPU")
        || device_info_command.contains("NULL_GPU_MAX_TEXTURE_SIZE")
    {
        return Err("real-graphics HWUI was compiled with the null-GPU contract".into());
    }
    if link_map_contents.contains("/opt/homebrew/opt/icu")
        || link_map_contents.contains("/opt/homebrew/Cellar/icu")
        || link_map_contents.contains("/opt/homebrew/opt/fmt")
        || link_map_contents.contains("/opt/homebrew/Cellar/fmt")
    {
        return Err("real-graphics link map consumed a Homebrew ICU/fmt provider".into());
    }
    Ok(())
}
