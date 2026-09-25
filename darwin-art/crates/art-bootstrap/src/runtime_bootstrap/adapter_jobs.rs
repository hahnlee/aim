use super::*;
use crate::native_build::PendingNativeCompile;
use darwin_art_build_contract::{
    COMMON_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES, HEADLESS_ADAPTER_SOURCES,
};

pub(super) fn adapter_jobs(
    staged: &RuntimeBootstrapStaging,
    real_graphics: bool,
    includes: &[&Path],
    runtime_includes: &[&Path],
) -> Vec<PendingNativeCompile> {
    let mut jobs = Vec::new();
    let adapter_sources = if real_graphics {
        GRAPHICS_ADAPTER_SOURCES
    } else {
        HEADLESS_ADAPTER_SOURCES
    };
    for &adapter_source in adapter_sources {
        let common = is_common_adapter_source(adapter_source);
        let adapter_object_dir = if common {
            &staged.runtime_core_object_dir
        } else {
            &staged.object_dir
        };
        // Adapter sources may live in responsibility-specific subdirectories.
        // Keep the archive member namespace flat so the shared object cache
        // never depends on pre-creating a parallel directory tree.
        let adapter_object =
            adapter_object_dir.join(format!("{}.o", adapter_source.replace(['/', '\\'], "_")));
        let compile_includes = if common { runtime_includes } else { includes };
        let mut adapter_command = if real_graphics
            && matches!(
                adapter_source,
                "darwin_libcore_natives.cc"
                    | "darwin_libcore_unicode_natives.cc"
                    | "darwin_framework_graphics_runtime.cc"
            ) {
            let mut libcore_includes = includes.to_vec();
            libcore_includes
                .retain(|path| *path != Path::new("/opt/homebrew/opt/icu4c@78/include"));
            libcore_includes.insert(0, staged.android_icu_i18n.as_path());
            libcore_includes.insert(0, staged.android_icu_common.as_path());
            runtime_bootstrap_cpp_command(&libcore_includes)
        } else {
            runtime_bootstrap_cpp_command(compile_includes)
        };
        if real_graphics
            && matches!(
                adapter_source,
                "darwin_framework_natives.cc"
                    | "darwin_framework_resource_registration.cc"
                    | "darwin_framework_graphics_runtime.cc"
                    | "darwin_runtime_elf_resolver.cc"
            )
        {
            adapter_command
                .arg("-DDARWIN_ART_REAL_GRAPHICS")
                .arg("-I")
                .arg(&staged.libcutils_include);
        }
        if adapter_source == "loader/graphics_ndk_symbols.cc" {
            adapter_command
                .arg("-I")
                .arg(staged.root.join("_aosp/frameworks/native/include"));
        }
        if adapter_source == "window/composition_fence_monitor.cc" {
            adapter_command.arg("-I").arg(
                staged
                    .root
                    .join("tools/bionic-socket-broker-adapter/include"),
            );
        }
        if matches!(
            adapter_source,
            "darwin_framework_natives.cc" | "window/surface_jni.cc" | "window/texture_view_jni.cc"
        ) {
            // Surface.java's nativeLockCanvas contract includes the pinned
            // NDK Canvas ABI from HWUI's apex export set in every flavor.
            adapter_command.arg("-I").arg(
                staged
                    .root
                    .join("_aosp/frameworks/base/libs/hwui/apex/include"),
            );
            adapter_command
                .arg("-I")
                .arg(staged.root.join("_aosp/system/core/libcutils/include"));
        }
        if matches!(
            adapter_source,
            "darwin_framework_binder_natives.cc"
                | "window/surface_jni.cc"
                | "window/remote_surface_producer.cc"
                | "binder/context_manager.cc"
                | "binder/ndk_parcel_host.cc"
                | "binder/platform_syscalls.cc"
                | "binder/service_endpoint.cc"
                | "binder/native_endpoint_lifetime.cc"
                | "../runtime/framework/wm/root_key_server_jni.cc"
                | "../runtime/framework/app/kernel_binder_client.cc"
                | "../runtime/framework/system/kernel_binder_service.cc"
        ) {
            // Binder/Parcel object layouts and the service/context endpoints
            // switch as one AOSP-owned unit. The Darwin boundary archive owns
            // only UNIX connection, peer identity and worker-thread entry.
            adapter_command
                .arg("-DDARWIN_ART_ORIGINAL_BINDER_JNI")
                .arg("-I")
                .arg(
                    staged
                        .root
                        .join("_build/binder-jni/patched-source/core/jni"),
                )
                .arg("-I")
                .arg(
                    staged.root.join(
                        "_build/surfaceflinger-core/work/frameworks-native/libs/binder/include",
                    ),
                )
                .arg("-I")
                .arg(staged.root.join("_aosp/system/core/libutils/include"))
                .arg("-I")
                .arg(staged.root.join("_aosp/system/core/libsystem/include"))
                .arg("-I")
                .arg(&staged.libcutils_include)
                .arg("-I")
                .arg(staged.root.join("_aosp/system/logging/liblog/include"));
        }
        if adapter_source == "binder/ndk_parcel_host.cc" {
            // HWUI's Bitmap parcel JNI uses the NDK AParcel API; its host
            // implementation shares the pinned libbinder_ndk headers.
            let ndk = staged.root.join("_aosp/frameworks/native/libs/binder/ndk");
            adapter_command
                .arg("-D__INTRODUCED_IN(n)=")
                .arg("-I")
                .arg(ndk.join("include_ndk"))
                .arg("-I")
                .arg(ndk.join("include_platform"))
                .arg("-I")
                .arg(staged.root.join("_aosp/system/libbase/include"));
        }
        if adapter_source == "binder/platform_syscalls.cc" {
            for include in [
                "tools/bionic-fs-facade/include",
                "tools/bionic-ioctl-facade/include",
                "tools/bionic-vm-facade/include",
            ] {
                adapter_command.arg("-I").arg(staged.root.join(include));
            }
        }
        if adapter_source == "darwin_os_constants.cc" {
            adapter_command
                .arg("-I")
                .arg(staged.root.join("_build/os-constants/generated"));
        }
        if matches!(
            adapter_source,
            "../runtime/framework/connectivity/network_path_platform.mm"
                | "../runtime/framework/wm/desktop_root_client_jni.mm"
                | "filesystem/document_panel.mm"
                | "window/desktop_root_events.mm"
                | "window/desktop_root_target.mm"
                | "window/desktop_root_surface.mm"
                | "window/appkit_window_delegate.mm"
                | "window/root_geometry_host.mm"
                | "window/appkit_content_view.mm"
                | "graphics/metal_display_backing.mm"
                | "graphics/surface_backing_owner.mm"
                | "graphics/scanout_diagnostic_capture.mm"
        ) {
            // These providers own strong/weak AppKit or Metal references. ARC is a
            // resource-lifetime requirement, not merely a syntax preference.
            adapter_command.args(["-fobjc-arc", "-fblocks"]);
        }
        if adapter_source == "loader/android_dlwarning.cc" {
            adapter_command
                .arg("-I")
                .arg(staged.root.join("_aosp/bionic-linker-config/linker"));
        }
        if real_graphics && adapter_source == "darwin_icu_jni_bridge.cc" {
            adapter_command.arg("-I").arg(staged.root.join("include"));
        }
        if real_graphics
            && matches!(
                adapter_source,
                "darwin_libcore_natives.cc" | "darwin_libcore_unicode_natives.cc"
            )
        {
            adapter_command.arg("-DDARWIN_ART_FULL_LIBCORE_LINUX");
        }
        if matches!(
            adapter_source,
            "darwin_runtime_adapters.cc"
                | "darwin_framework_animation_natives.cc"
                | "darwin_android_asset_manager.cc"
                | "darwin_android_platform.mm"
                | "graphics/hardware_buffer_owner.mm"
                | "looper/android_looper_owner.cc"
                | "looper/android_choreographer_owner.cc"
                | "darwin_android_native_window.cc"
                | "darwin_android_sync.cc"
                | "darwin_android_surface_texture.cc"
                | "darwin_android_media_ndk.cc"
                | "darwin_runtime_elf_lifecycle.cc"
                | "darwin_runtime_elf_resolver.cc"
                | "darwin_runtime_native_loader.cc"
                | "loader/bionic_provider_set.cc"
                | "loader/bionic_symbol_lookup.cc"
                | "darwin_runtime_jni_registration.cc"
                | "darwin_provider_owners.cc"
                | "darwin_jni_proxy_lookup.cc"
                | "darwin_jni_proxy_registration.cc"
        ) {
            for include in [
                "tools/bionic-provider-namespace/include",
                "tools/bionic-dso-lifecycle-facade/include",
                "tools/bionic-fs-facade/include",
                "tools/bionic-dns-facade/include",
                "tools/bionic-socket-broker-adapter/include",
                "tools/bionic-sendfile-facade/include",
                "tools/bionic-stdio-facade/include",
                "tools/bionic-ioctl-facade/include",
                "tools/bionic-strftime-facade/include",
                "tools/bionic-vm-facade/include",
            ] {
                adapter_command.arg("-I").arg(staged.root.join(include));
            }
        }
        if matches!(
            adapter_source,
            "filesystem/archive_filesystem.cc" | "filesystem/guest_config.cc"
                | "filesystem/guest_file.cc" | "process/procfs_jni.cc"
        ) {
            for include in [
                "tools/bionic-fs-facade/include",
                "tools/bionic-ioctl-facade/include",
                "tools/bionic-errno-tls/include",
            ] {
                adapter_command.arg("-I").arg(staged.root.join(include));
            }
        }
        if adapter_source == "looper/android_looper_owner.cc" {
            adapter_command
                .arg("-I")
                .arg(staged.root.join("tools/bionic-errno-tls/include"));
        }
        if adapter_source == "darwin_android_elf_image_registry.cc" {
            adapter_command.arg("-I").arg(
                staged
                    .root
                    .join("tools/android-dl-iterate-phdr-provider/include"),
            );
        }
        if adapter_source == "darwin_android_surface_texture.cc" {
            adapter_command
                .arg("-I")
                .arg(
                    staged.root.join(
                        "_aosp/hwui-static-deps/frameworks-native/libs/nativedisplay/include",
                    ),
                )
                .arg("-I")
                .arg(staged.root.join("_aosp/system/core/libsystem/include"));
        }
        if adapter_source == "darwin_android_asset_manager.cc" {
            adapter_command
                .arg("-DDARWIN_ART_AOSP_COMPAT_LSEEK64")
                .arg("-I")
                .arg(&staged.liblog_include)
                .args(["-include", "log/log.h"]);
            for include in [
                "frameworks/base/libs/androidfw/include",
                "frameworks/base/core/jni/include",
                "frameworks/native/include",
                "system/incremental_delivery/incfs/util/include",
                "system/core/libutils/include",
                "system/core/libsystem/include",
                "system/core/libcutils/include",
            ] {
                adapter_command
                    .arg("-I")
                    .arg(staged.root.join("_aosp").join(include));
            }
        }
        adapter_command
            .arg("-idirafter")
            .arg(&staged.ndk_arch_include)
            .arg("-idirafter")
            .arg(&staged.ndk_include)
            .arg("-Wno-macro-redefined")
            .arg("-c")
            .arg(staged.root.join("compat").join(adapter_source))
            .arg("-o")
            .arg(&adapter_object);
        jobs.push(PendingNativeCompile {
            command: adapter_command,
            object: adapter_object,
        });
    }
    jobs
}

fn is_common_adapter_source(source: &str) -> bool {
    COMMON_ADAPTER_SOURCES.contains(&source)
}
