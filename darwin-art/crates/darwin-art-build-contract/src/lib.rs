//! Small, dependency-free contracts shared by native build orchestration.
//!
//! This crate deliberately contains no filesystem or process code.  It is the
//! stable Rust source of truth for identities that must agree between the
//! canonical ART builder and the incremental Ninja graph generator.

pub mod native_install;

/// Bump when the common runtime/adapters include or command contract changes.
/// A mismatch disables cache promotion until the canonical builder repopulates
/// `_build/runtime-common`.
pub const RUNTIME_CACHE_IDENTITY: &str = "darwin-art-runtime-core-cache-v29-camera-metadata-jni";

/// Identity of the generated native Ninja graph. Keeping this beside the
/// runtime cache contract prevents the canonical builder and graph emitter
/// from silently disagreeing about graph format or edge ownership.
pub const NATIVE_GRAPH_VERSION: &str = "darwin-art-native-graph-v33-camera-metadata-jni";

/// Canonical adapter translation units for the two runtime flavors.  Keeping
/// this list in the dependency-free contract crate prevents the Cargo
/// bootstrap and Ninja graph emitter from drifting when a native boundary is
/// split or added.
pub const HEADLESS_ADAPTER_SOURCES: &[&str] = &[
    "darwin_canvas_headless_stubs.cc",
    "darwin_art_abi_layout.cc",
    "darwin_android_jni_trampoline.cc",
    "darwin_android_elf_image_registry.cc",
    "darwin_android_system_fonts.cc",
    "darwin_android_asset_manager.cc",
    "darwin_android_platform.mm",
    "network/multinetwork.cc",
    "memory/shared_memory.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_media_codec.cc",
    "darwin_provider_owners.cc",
    "darwin_angle_egl.cc",
    "darwin_android_native_window.cc",
    "window/locked_surface.cc",
    "window/surface_control_jni.cc",
    "window/hardware_buffer_jni.cc",
    "darwin_android_native_window_api.cc",
    "darwin_android_egl_platform.cc",
    "darwin_android_sync.cc",
    "darwin_android_surface_texture.cc",
    "darwin_audio_track.mm",
    "darwin_framework_natives.cc",
    "darwin_security_trust.mm",
    "darwin_motion_event_natives.cc",
    "darwin_framework_binder_natives.cc",
    "binder/calling_identity.cc",
    "binder/platform_syscalls.cc",
    "binder/remote_binder_jni.cc",
    "binder/context_manager.cc",
    "binder/service_endpoint.cc",
    "../runtime/framework/wm/client_transaction.cc",
    "../runtime/framework/wm/activity_launch_transaction.cc",
    "../runtime/framework/wm/desktop_window_metadata.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/os/service_process_transport.cc",
    "../runtime/framework/app/main_loop.cc",
    "../runtime/framework/app/kernel_binder_client.cc",
    "../runtime/framework/app/java_exception_report.cc",
    "graphics/graphics_environment.cc",
    "../runtime/framework/app/process_entry.cc",
    "../runtime/framework/app/process_registration.cc",
    "loader/classloader_identity.cc",
    "loader/classloader_namespaces.cc",
    "loader/boot_apex_jni_policy.cc",
    "loader/bionic_provider_set.cc",
    "loader/bionic_symbol_lookup.cc",
    "loader/library_search.cc",
    "loader/elf_graph_cache.cc",
    "loader/namespace_elf_group.cc",
    "loader/android_dlwarning.cc",
    "../runtime/framework/pm/provider_metadata.cc",
    "../runtime/framework/pm/installed_record_source.cc",
    "../runtime/framework/system/process_entry.cc",
    "../runtime/framework/system/kernel_binder_service.cc",
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
    "../runtime/framework/am/application_binding.cc",
    "../runtime/framework/am/attachment.cc",
    "../runtime/framework/am/process_launch.cc",
    "filesystem/archive_filesystem.cc",
    "filesystem/process_authority.cc",
    "process/procfs_jni.cc",
    "process/scheduling_jni.cc",
    "binder/peer_credentials.cc",
    "darwin_framework_sqlite_natives.cc",
    "darwin_framework_system_property_natives.cc",
    "darwin_os_constants.cc",
    "darwin_framework_asset_manager_natives.cc",
    "darwin_framework_render_node_natives.cc",
    "darwin_framework_graphics_runtime.cc",
    "darwin_framework_resource_registration.cc",
    "darwin_framework_system_natives.cc",
    "process/process_name.mm",
    "darwin_framework_animation_natives.cc",
    "darwin_icu_natives.cc",
    "darwin_libcore_natives.cc",
    "darwin_libcore_unicode_natives.cc",
    "darwin_runtime_adapters.cc",
    "darwin_runtime_platform_stubs.cc",
    "darwin_native_bridge_stubs.cc",
    "darwin_jni_shorty.cc",
    "jni/android_varargs.cc",
    "jni/method_call.cc",
    "darwin_jni_proxy_lookup.cc",
    "darwin_jni_proxy_registration.cc",
    "darwin_runtime_elf_lifecycle.cc",
    "darwin_runtime_elf_resolver.cc",
    "darwin_runtime_native_loader.cc",
    "darwin_runtime_jni_registration.cc",
    "darwin_sigchain.cc",
    "fault_handler_arm64_darwin.cc",
];

pub const GRAPHICS_ADAPTER_SOURCES: &[&str] = &[
    "darwin_art_abi_layout.cc",
    "darwin_android_jni_trampoline.cc",
    "darwin_android_elf_image_registry.cc",
    "darwin_android_system_fonts.cc",
    "darwin_android_asset_manager.cc",
    "darwin_android_platform.mm",
    "network/multinetwork.cc",
    "memory/shared_memory.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_media_codec.cc",
    "darwin_provider_owners.cc",
    "darwin_angle_egl.cc",
    "darwin_android_native_window.cc",
    "window/locked_surface.cc",
    "window/surface_control_jni.cc",
    "window/hardware_buffer_jni.cc",
    "darwin_android_native_window_api.cc",
    "darwin_android_egl_platform.cc",
    "darwin_android_sync.cc",
    "darwin_android_surface_texture.cc",
    "darwin_audio_track.mm",
    "darwin_framework_natives.cc",
    "darwin_security_trust.mm",
    "darwin_motion_event_natives.cc",
    "darwin_framework_binder_natives.cc",
    "binder/calling_identity.cc",
    "binder/platform_syscalls.cc",
    "binder/remote_binder_jni.cc",
    "binder/context_manager.cc",
    "binder/service_endpoint.cc",
    "../runtime/framework/wm/client_transaction.cc",
    "../runtime/framework/wm/activity_launch_transaction.cc",
    "../runtime/framework/wm/desktop_window_metadata.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/os/service_process_transport.cc",
    "../runtime/framework/app/main_loop.cc",
    "../runtime/framework/app/kernel_binder_client.cc",
    "../runtime/framework/app/java_exception_report.cc",
    "graphics/graphics_environment.cc",
    "graphics/overlay_properties_natives.cc",
    "../runtime/framework/app/process_entry.cc",
    "../runtime/framework/app/process_registration.cc",
    "loader/classloader_identity.cc",
    "loader/classloader_namespaces.cc",
    "loader/boot_apex_jni_policy.cc",
    "loader/bionic_provider_set.cc",
    "loader/bionic_symbol_lookup.cc",
    "loader/library_search.cc",
    "loader/elf_graph_cache.cc",
    "loader/namespace_elf_group.cc",
    "loader/android_dlwarning.cc",
    "../runtime/framework/pm/provider_metadata.cc",
    "../runtime/framework/pm/installed_record_source.cc",
    "../runtime/framework/system/process_entry.cc",
    "../runtime/framework/system/kernel_binder_service.cc",
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/am/application_binding.cc",
    "../runtime/framework/am/attachment.cc",
    "../runtime/framework/am/process_launch.cc",
    "filesystem/archive_filesystem.cc",
    "filesystem/process_authority.cc",
    "process/procfs_jni.cc",
    "process/scheduling_jni.cc",
    "binder/peer_credentials.cc",
    "darwin_framework_sqlite_natives.cc",
    "darwin_framework_system_property_natives.cc",
    "darwin_os_constants.cc",
    "darwin_framework_asset_manager_natives.cc",
    "darwin_framework_render_node_natives.cc",
    "darwin_framework_graphics_runtime.cc",
    "darwin_framework_resource_registration.cc",
    "darwin_framework_system_natives.cc",
    "process/process_name.mm",
    "darwin_framework_animation_natives.cc",
    "darwin_icu_jni_bridge.cc",
    "darwin_libcore_natives.cc",
    "darwin_libcore_unicode_natives.cc",
    "darwin_runtime_adapters.cc",
    "darwin_runtime_platform_stubs.cc",
    "darwin_native_bridge_stubs.cc",
    "darwin_jni_shorty.cc",
    "jni/android_varargs.cc",
    "jni/method_call.cc",
    "darwin_jni_proxy_lookup.cc",
    "darwin_jni_proxy_registration.cc",
    "darwin_runtime_elf_lifecycle.cc",
    "darwin_runtime_elf_resolver.cc",
    "darwin_runtime_native_loader.cc",
    "darwin_runtime_jni_registration.cc",
    "darwin_sigchain.cc",
    "fault_handler_arm64_darwin.cc",
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
];

pub const COMMON_ADAPTER_SOURCES: &[&str] = &[
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
    "window/surface_control_jni.cc",
    "filesystem/process_authority.cc",
    "loader/classloader_identity.cc",
    "loader/classloader_namespaces.cc",
    "loader/boot_apex_jni_policy.cc",
    "loader/bionic_provider_set.cc",
    "loader/bionic_symbol_lookup.cc",
    "loader/library_search.cc",
    "loader/elf_graph_cache.cc",
    "loader/namespace_elf_group.cc",
    "loader/android_dlwarning.cc",
    "darwin_art_abi_layout.cc",
    "darwin_android_jni_trampoline.cc",
    "darwin_android_elf_image_registry.cc",
    "darwin_android_system_fonts.cc",
    "darwin_android_asset_manager.cc",
    "darwin_android_platform.mm",
    "network/multinetwork.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_provider_owners.cc",
    "darwin_framework_animation_natives.cc",
    "darwin_runtime_adapters.cc",
    "darwin_runtime_platform_stubs.cc",
    "darwin_native_bridge_stubs.cc",
    "darwin_jni_shorty.cc",
    "jni/android_varargs.cc",
    "jni/method_call.cc",
    "darwin_jni_proxy_lookup.cc",
    "darwin_jni_proxy_registration.cc",
    "darwin_runtime_elf_lifecycle.cc",
    "darwin_runtime_elf_resolver.cc",
    "darwin_runtime_native_loader.cc",
    "darwin_runtime_jni_registration.cc",
    "darwin_sigchain.cc",
    "fault_handler_arm64_darwin.cc",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFlavor {
    Headless,
    Graphics,
}

impl RuntimeFlavor {
    pub const fn real_graphics(self) -> bool {
        matches!(self, Self::Graphics)
    }

    pub const fn output_dir(self) -> &'static str {
        match self {
            Self::Headless => "runtime-bootstrap",
            Self::Graphics => "runtime-graphics-bootstrap",
        }
    }

    pub const fn archive_name(self) -> &'static str {
        match self {
            Self::Headless => "libart-runtime-bootstrap-darwin.a",
            Self::Graphics => "libart-runtime-graphics-bootstrap-darwin.a",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jni_argument_adapter_is_shared_production_source() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            for source in ["jni/android_varargs.cc", "jni/method_call.cc"] {
                assert_eq!(sources.iter().filter(|&&s| s == source).count(), 1);
            }
        }
    }

    #[test]
    fn flavor_contracts_are_distinct_and_stable() {
        assert!(!RuntimeFlavor::Headless.real_graphics());
        assert!(RuntimeFlavor::Graphics.real_graphics());
        assert_ne!(
            RuntimeFlavor::Headless.output_dir(),
            RuntimeFlavor::Graphics.output_dir()
        );
        assert_ne!(
            RuntimeFlavor::Headless.archive_name(),
            RuntimeFlavor::Graphics.archive_name()
        );
    }

    #[test]
    fn surface_control_jni_has_one_owner_in_every_runtime() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "window/surface_control_jni.cc")
                    .count(),
                1
            );
        }
    }
}
