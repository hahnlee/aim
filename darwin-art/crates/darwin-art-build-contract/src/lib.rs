//! Small, dependency-free contracts shared by native build orchestration.
//!
//! This crate deliberately contains no filesystem or process code.  It is the
//! stable Rust source of truth for identities that must agree between the
//! canonical ART builder and the incremental Ninja graph generator.

pub mod native_install;
pub mod support_java;

/// Bump when the common runtime/adapters include or command contract changes.
/// A mismatch disables cache promotion until the canonical builder repopulates
/// `_build/runtime-common`.
pub const RUNTIME_CACHE_IDENTITY: &str =
    "darwin-art-runtime-core-cache-v33-product-fixture-separation";

/// Identity of the generated native Ninja graph. Keeping this beside the
/// runtime cache contract prevents the canonical builder and graph emitter
/// from silently disagreeing about graph format or edge ownership.
pub const NATIVE_GRAPH_VERSION: &str = "darwin-art-native-graph-v42-typed-client-receipt";

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
    "filesystem/document_panel.mm",
    "input/darwin_hardware_key_translation.mm",
    "window/desktop_root_events.mm",
    "window/desktop_foreground_provider.cc",
    "window/desktop_root_target.mm",
    "window/desktop_root_surface.mm",
    "window/appkit_window_delegate.mm",
    "window/appkit_content_view.mm",
    "window/application_identity.mm",
    "window/surface_scanout_owner.mm",
    "graphics/metal_shared_event_provider.mm",
    "graphics/metal_display_backing.mm",
    "graphics/surface_backing_owner.mm",
    "graphics/scanout_diagnostic_capture.mm",
    "graphics/egl_native_fence_owner.cc",
    "graphics/egl_window_surface_owner.cc",
    "graphics/egl_window_backend.cc",
    "graphics/egl_error_state.cc",
    "../runtime/embedding/session_lifetime.cc",
    "graphics/hardware_buffer_owner.mm",
    "looper/android_looper_owner.cc",
    "looper/reusable_task.cc",
    "looper/android_choreographer_owner.cc",
    "network/multinetwork.cc",
    "memory/shared_memory.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "surfaceflinger/retained_layer_state.cc",
    "surfaceflinger/socket_transport.cc",
    "surfaceflinger/client_receipt.cc",
    "surfaceflinger/client_transport_darwin.mm",
    "surfaceflinger/transaction_reply.cc",
    "surfaceflinger/iosurface_backing.mm",
    "surfaceflinger/output_owner.cc",
    "window/display_output.mm",
    "window/composition_fence_monitor.cc",
    "surfaceflinger/output_registry.cc",
    "surfaceflinger/service_ingress.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/framework_media_jni.cc",
    "media/media_extractor_jni.cc",
    "input/velocity_tracker_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_media_codec.cc",
    "darwin_provider_owners.cc",
    "darwin_angle_egl.cc",
    "graphics/composition_buffer_lease.cc",
    "graphics/egl_ahb_image_owner.cc",
    "graphics/composition_consumer.cc",
    "graphics/egl_context_dispatch.cc",
    "darwin_android_native_window.cc",
    "window/locked_surface.cc",
    "window/surface_control_jni.cc",
    "window/surface_control_state.cc",
    "window/surface_control_registry.cc",
    "window/surface_control_ready_transaction.cc",
    "window/surface_control_submit_darwin.mm",
    "window/surface_jni.cc",
    "window/blast_transaction_state.cc",
    "window/surface_transaction_merge.cc",
    "window/surface_transaction_builder.cc",
    "window/surface_transaction_lifetime.cc",
    "window/native_window_transaction_consumer.cc",
    "window/native_window_buffer_queue.cc",
    "window/surface_transaction_submission.cc",
    "window/blast_buffer_queue_jni.cc",
    "window/hardware_buffer_jni.cc",
    "darwin_android_native_window_api.cc",
    "darwin_android_egl_platform.cc",
    "darwin_android_sync.cc",
    "darwin_android_surface_texture.cc",
    "darwin_audio_track.mm",
    "media/audio_system_jni.cc",
    "media/audio_track_jni.cc",
    "window/sync_fence_jni.cc",
    "art/runtime_native_load.cc",
    "darwin_framework_natives.cc",
    "darwin_security_trust.mm",
    "darwin_motion_event_natives.cc",
    "darwin_framework_binder_natives.cc",
    "binder/calling_identity.cc",
    "binder/platform_syscalls.cc",
    "binder/remote_binder_jni.cc",
    "binder/remote_binder_identity_jni.cc",
    "binder/wire_channel_lifetime.cc",
    "binder/native_endpoint_lifetime.cc",
    "binder/context_manager.cc",
    "binder/service_endpoint.cc",
    "../runtime/framework/wm/client_transaction.cc",
    "../runtime/framework/wm/activity_launch_transaction.cc",
    "../runtime/framework/wm/desktop_window_metadata.cc",
    "../runtime/framework/wm/desktop_root_client_jni.mm",
    "../runtime/framework/wm/desktop_foreground_authority_jni.cc",
    "../runtime/framework/wm/root_key_server_jni.cc",
    "../runtime/framework/wm/root_key_decision_jni.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/os/service_process_transport.cc",
    "../runtime/framework/input/channel_owner.cc",
    "../runtime/framework/input/root_key_authority.cc",
    "../runtime/framework/input/root_key_routing.cc",
    "../runtime/framework/input/root_key_ingress.cc",
    "../runtime/framework/input/root_key_ingress_registry.cc",
    "../runtime/framework/input/receiver_jni.cc",
    "../runtime/framework/input/receiver_transport_policy.cc",
    "../runtime/framework/input/receiver_input_consumer.cc",
    "../runtime/framework/input/receiver_packet_consumption.cc",
    "../runtime/framework/input/input_routing_packet_lease.cc",
    "../runtime/framework/input/receiver_focus_control.cc",
    "../runtime/framework/input/receiver_focus_jni.cc",
    "../runtime/framework/input/channel_resources.cc",
    "../runtime/framework/input/channel_identity_catalog.cc",
    "../runtime/framework/input/input_channel_jni.cc",
    "../runtime/framework/wm/window_input_publisher_jni.cc",
    "../runtime/framework/wm/window_input_endpoint_lease.cc",
    "../runtime/framework/input/key_character_map_jni.cc",
    "../runtime/framework/input/packet_dispatch.cc",
    "../runtime/framework/input/receiver_registry.cc",
    "../runtime/framework/input/receiver_admission.cc",
    "../runtime/framework/input/input_transport.cc",
    "../runtime/framework/input/input_framed_reader.cc",
    "../runtime/framework/input/transport_registration_authority.cc",
    "../runtime/framework/input/channel_endpoint.cc",
    "../runtime/framework/input/finish_ledger.cc",
    "../runtime/framework/input/receiver_finish_owner.cc",
    "../runtime/framework/input/routing_transport_dispatch.cc",
    "../runtime/framework/input/routing_transport_scheduler.cc",
    "../runtime/framework/input/channel_routing_continuation.cc",
    "../runtime/framework/input/receiver_endpoint_binding.cc",
    "../runtime/framework/input/claimed_input_transport_pump.cc",
    "../runtime/framework/input/input_resource_progress.cc",
    "../runtime/framework/input/receiver_jni_resources.cc",
    "../runtime/framework/input/receiver_routing_lifecycle.cc",
    "../runtime/framework/input/receiver_retirement_barrier.cc",
    "../runtime/framework/input/pending_receiver_retirement.cc",
    "../runtime/framework/input/receiver_retirement_driver.cc",
    "../runtime/framework/input/receiver_lifecycle.cc",
    "../runtime/framework/input/view_root_input_jni.cc",
    "../runtime/framework/input/input_transport_pump.cc",
    "../runtime/framework/input/input_transport_readiness.cc",
    "../runtime/framework/input/input_routing_domain.cc",
    "../runtime/framework/input/input_routing_focus.cc",
    "../runtime/framework/input/input_routing_actions.cc",
    "../runtime/framework/input/input_routing.cc",
    "process/host_services.cc",
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
    "../runtime/framework/system/system_context.cc",
    "../runtime/framework/compat/policy_binding.cc",
    "../runtime/framework/system/kernel_binder_service.cc",
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
    "../runtime/framework/am/application_binding.cc",
    "../runtime/framework/am/attachment.cc",
    "../runtime/framework/am/process_launch.cc",
    "../runtime/framework/am/connection_death_jni.cc",
    "filesystem/archive_filesystem.cc",
    "filesystem/guest_config.cc",
    "filesystem/guest_file.cc",
    "filesystem/process_authority.cc",
    "process/procfs_jni.cc",
    "process/scheduling_jni.cc",
    "binder/peer_credentials.cc",
    "darwin_framework_sqlite_natives.cc",
    "darwin_framework_system_property_natives.cc",
    "darwin_os_constants.cc",
    "darwin_framework_graphics_runtime.cc",
    "darwin_framework_resource_registration.cc",
    "darwin_framework_system_natives.cc",
    "../runtime/framework/looper/message_queue_jni.cc",
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
    "loader/graphics_ndk_symbols.cc",
    "darwin_art_abi_layout.cc",
    "darwin_android_jni_trampoline.cc",
    "darwin_android_elf_image_registry.cc",
    "darwin_android_system_fonts.cc",
    "darwin_android_asset_manager.cc",
    "darwin_android_platform.mm",
    "filesystem/document_panel.mm",
    "input/darwin_hardware_key_translation.mm",
    "window/desktop_root_events.mm",
    "window/desktop_foreground_provider.cc",
    "window/desktop_root_target.mm",
    "window/desktop_root_surface.mm",
    "window/appkit_window_delegate.mm",
    "window/appkit_content_view.mm",
    "window/application_identity.mm",
    "window/surface_scanout_owner.mm",
    "graphics/metal_shared_event_provider.mm",
    "graphics/metal_display_backing.mm",
    "graphics/surface_backing_owner.mm",
    "graphics/scanout_diagnostic_capture.mm",
    "graphics/egl_native_fence_owner.cc",
    "graphics/egl_window_surface_owner.cc",
    "graphics/egl_window_backend.cc",
    "graphics/egl_error_state.cc",
    "../runtime/embedding/session_lifetime.cc",
    "graphics/hardware_buffer_owner.mm",
    "looper/android_looper_owner.cc",
    "looper/reusable_task.cc",
    "looper/android_choreographer_owner.cc",
    "network/multinetwork.cc",
    "memory/shared_memory.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "surfaceflinger/retained_layer_state.cc",
    "surfaceflinger/socket_transport.cc",
    "surfaceflinger/client_receipt.cc",
    "surfaceflinger/client_transport_darwin.mm",
    "surfaceflinger/transaction_reply.cc",
    "surfaceflinger/iosurface_backing.mm",
    "surfaceflinger/output_owner.cc",
    "window/display_output.mm",
    "window/composition_fence_monitor.cc",
    "surfaceflinger/output_registry.cc",
    "surfaceflinger/service_ingress.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/framework_media_jni.cc",
    "media/media_extractor_jni.cc",
    "input/velocity_tracker_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_media_codec.cc",
    "darwin_provider_owners.cc",
    "darwin_angle_egl.cc",
    "graphics/composition_buffer_lease.cc",
    "graphics/egl_ahb_image_owner.cc",
    "graphics/composition_consumer.cc",
    "graphics/egl_context_dispatch.cc",
    "darwin_android_native_window.cc",
    "window/locked_surface.cc",
    "window/surface_control_jni.cc",
    "window/surface_control_state.cc",
    "window/surface_control_registry.cc",
    "window/surface_control_ready_transaction.cc",
    "window/surface_control_submit_darwin.mm",
    "window/surface_jni.cc",
    "window/blast_transaction_state.cc",
    "window/surface_transaction_merge.cc",
    "window/surface_transaction_builder.cc",
    "window/surface_transaction_lifetime.cc",
    "window/native_window_transaction_consumer.cc",
    "window/native_window_buffer_queue.cc",
    "window/surface_transaction_submission.cc",
    "window/blast_buffer_queue_jni.cc",
    "window/hardware_buffer_jni.cc",
    "darwin_android_native_window_api.cc",
    "darwin_android_egl_platform.cc",
    "darwin_android_sync.cc",
    "darwin_android_surface_texture.cc",
    "darwin_audio_track.mm",
    "media/audio_system_jni.cc",
    "media/audio_track_jni.cc",
    "window/sync_fence_jni.cc",
    "art/runtime_native_load.cc",
    "darwin_framework_natives.cc",
    "darwin_security_trust.mm",
    "darwin_motion_event_natives.cc",
    "darwin_framework_binder_natives.cc",
    "binder/calling_identity.cc",
    "binder/platform_syscalls.cc",
    "binder/remote_binder_jni.cc",
    "binder/remote_binder_identity_jni.cc",
    "binder/wire_channel_lifetime.cc",
    "binder/native_endpoint_lifetime.cc",
    "binder/context_manager.cc",
    "binder/service_endpoint.cc",
    "../runtime/framework/wm/client_transaction.cc",
    "../runtime/framework/wm/activity_launch_transaction.cc",
    "../runtime/framework/wm/desktop_window_metadata.cc",
    "../runtime/framework/wm/desktop_root_client_jni.mm",
    "../runtime/framework/wm/desktop_foreground_authority_jni.cc",
    "../runtime/framework/wm/root_key_server_jni.cc",
    "../runtime/framework/wm/root_key_decision_jni.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/os/service_process_transport.cc",
    "../runtime/framework/input/channel_owner.cc",
    "../runtime/framework/input/root_key_authority.cc",
    "../runtime/framework/input/root_key_routing.cc",
    "../runtime/framework/input/root_key_ingress.cc",
    "../runtime/framework/input/root_key_ingress_registry.cc",
    "../runtime/framework/input/receiver_jni.cc",
    "../runtime/framework/input/receiver_transport_policy.cc",
    "../runtime/framework/input/receiver_input_consumer.cc",
    "../runtime/framework/input/receiver_packet_consumption.cc",
    "../runtime/framework/input/input_routing_packet_lease.cc",
    "../runtime/framework/input/receiver_focus_control.cc",
    "../runtime/framework/input/receiver_focus_jni.cc",
    "../runtime/framework/input/channel_resources.cc",
    "../runtime/framework/input/channel_identity_catalog.cc",
    "../runtime/framework/input/input_channel_jni.cc",
    "../runtime/framework/wm/window_input_publisher_jni.cc",
    "../runtime/framework/wm/window_input_endpoint_lease.cc",
    "../runtime/framework/input/key_character_map_jni.cc",
    "../runtime/framework/input/packet_dispatch.cc",
    "../runtime/framework/input/receiver_registry.cc",
    "../runtime/framework/input/receiver_admission.cc",
    "../runtime/framework/input/input_transport.cc",
    "../runtime/framework/input/input_framed_reader.cc",
    "../runtime/framework/input/transport_registration_authority.cc",
    "../runtime/framework/input/channel_endpoint.cc",
    "../runtime/framework/input/finish_ledger.cc",
    "../runtime/framework/input/receiver_finish_owner.cc",
    "../runtime/framework/input/routing_transport_dispatch.cc",
    "../runtime/framework/input/routing_transport_scheduler.cc",
    "../runtime/framework/input/channel_routing_continuation.cc",
    "../runtime/framework/input/receiver_endpoint_binding.cc",
    "../runtime/framework/input/claimed_input_transport_pump.cc",
    "../runtime/framework/input/input_resource_progress.cc",
    "../runtime/framework/input/receiver_jni_resources.cc",
    "../runtime/framework/input/receiver_routing_lifecycle.cc",
    "../runtime/framework/input/receiver_retirement_barrier.cc",
    "../runtime/framework/input/pending_receiver_retirement.cc",
    "../runtime/framework/input/receiver_retirement_driver.cc",
    "../runtime/framework/input/receiver_lifecycle.cc",
    "../runtime/framework/input/view_root_input_jni.cc",
    "../runtime/framework/input/input_transport_pump.cc",
    "../runtime/framework/input/input_transport_readiness.cc",
    "../runtime/framework/input/input_routing_domain.cc",
    "../runtime/framework/input/input_routing_focus.cc",
    "../runtime/framework/input/input_routing_actions.cc",
    "../runtime/framework/input/input_routing.cc",
    "process/host_services.cc",
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
    "../runtime/framework/system/system_context.cc",
    "../runtime/framework/compat/policy_binding.cc",
    "../runtime/framework/system/kernel_binder_service.cc",
    "../runtime/framework/connectivity/network_path_platform.mm",
    "../runtime/framework/connectivity/network_provider_jni.cc",
    "../runtime/framework/power/power_state_platform.mm",
    "../runtime/framework/power/power_state_jni.cc",
    "../runtime/framework/camera/camera_metadata_jni.cc",
    "../runtime/framework/am/application_binding.cc",
    "../runtime/framework/am/attachment.cc",
    "../runtime/framework/am/process_launch.cc",
    "../runtime/framework/am/connection_death_jni.cc",
    "filesystem/archive_filesystem.cc",
    "filesystem/guest_config.cc",
    "filesystem/guest_file.cc",
    "filesystem/process_authority.cc",
    "process/procfs_jni.cc",
    "process/scheduling_jni.cc",
    "binder/peer_credentials.cc",
    "darwin_framework_sqlite_natives.cc",
    "darwin_framework_system_property_natives.cc",
    "darwin_os_constants.cc",
    "darwin_framework_graphics_runtime.cc",
    "darwin_framework_resource_registration.cc",
    "darwin_framework_system_natives.cc",
    "../runtime/framework/looper/message_queue_jni.cc",
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
    "window/surface_control_state.cc",
    "window/surface_control_registry.cc",
    "window/surface_control_ready_transaction.cc",
    "window/surface_control_submit_darwin.mm",
    "window/surface_jni.cc",
    "window/blast_transaction_state.cc",
    "window/surface_transaction_merge.cc",
    "window/surface_transaction_builder.cc",
    "window/surface_transaction_lifetime.cc",
    "window/native_window_transaction_consumer.cc",
    "window/native_window_buffer_queue.cc",
    "window/surface_transaction_submission.cc",
    "window/blast_buffer_queue_jni.cc",
    "filesystem/process_authority.cc",
    "process/host_services.cc",
    "filesystem/guest_config.cc",
    "filesystem/guest_file.cc",
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
    "graphics/metal_shared_event_provider.mm",
    "graphics/metal_display_backing.mm",
    "graphics/surface_backing_owner.mm",
    "graphics/scanout_diagnostic_capture.mm",
    "graphics/egl_native_fence_owner.cc",
    "graphics/egl_window_surface_owner.cc",
    "graphics/egl_window_backend.cc",
    "graphics/egl_error_state.cc",
    "graphics/hardware_buffer_owner.mm",
    "looper/android_looper_owner.cc",
    "looper/reusable_task.cc",
    "looper/android_choreographer_owner.cc",
    "window/sync_fence_jni.cc",
    "art/runtime_native_load.cc",
    "network/multinetwork.cc",
    "surfaceflinger/metal_composer.mm",
    "surfaceflinger/service_darwin.mm",
    "surfaceflinger/composition_queue.cc",
    "surfaceflinger/retained_layer_state.cc",
    "surfaceflinger/socket_transport.cc",
    "surfaceflinger/client_receipt.cc",
    "surfaceflinger/client_transport_darwin.mm",
    "surfaceflinger/transaction_reply.cc",
    "surfaceflinger/iosurface_backing.mm",
    "surfaceflinger/output_owner.cc",
    "window/display_output.mm",
    "window/composition_fence_monitor.cc",
    "surfaceflinger/output_registry.cc",
    "surfaceflinger/service_ingress.cc",
    "darwin_android_media_ndk.cc",
    "media/format.cc",
    "media/consumer_buffer.cc",
    "media/image_consumer_queue.cc",
    "media/image_reader_jni.cc",
    "media/framework_media_jni.cc",
    "media/media_extractor_jni.cc",
    "input/velocity_tracker_jni.cc",
    "media/image_reader_ndk.cc",
    "darwin_provider_owners.cc",
    "media/audio_system_jni.cc",
    "media/audio_track_jni.cc",
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
    fn runtime_lifetime_owners_are_explicit_product_sources() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            for owner in [
                "looper/reusable_task.cc",
                "../runtime/framework/input/receiver_jni.cc",
                "../runtime/framework/input/receiver_transport_policy.cc",
                "../runtime/framework/input/receiver_input_consumer.cc",
                "../runtime/framework/input/receiver_packet_consumption.cc",
                "../runtime/framework/input/input_routing_packet_lease.cc",
                "../runtime/framework/input/receiver_focus_control.cc",
                "../runtime/framework/input/receiver_focus_jni.cc",
                "../runtime/framework/input/receiver_registry.cc",
                "../runtime/framework/input/receiver_admission.cc",
                "../runtime/framework/input/input_transport.cc",
                "../runtime/framework/input/input_framed_reader.cc",
                "../runtime/framework/input/transport_registration_authority.cc",
                "../runtime/framework/input/channel_endpoint.cc",
                "../runtime/framework/input/finish_ledger.cc",
                "../runtime/framework/input/receiver_finish_owner.cc",
                "../runtime/framework/input/routing_transport_dispatch.cc",
                "../runtime/framework/input/routing_transport_scheduler.cc",
                "../runtime/framework/input/channel_routing_continuation.cc",
                "../runtime/framework/input/receiver_endpoint_binding.cc",
                "../runtime/framework/input/claimed_input_transport_pump.cc",
                "../runtime/framework/input/input_resource_progress.cc",
                "../runtime/framework/input/receiver_jni_resources.cc",
                "../runtime/framework/input/receiver_routing_lifecycle.cc",
                "../runtime/framework/input/receiver_retirement_barrier.cc",
                "../runtime/framework/input/pending_receiver_retirement.cc",
                "../runtime/framework/input/receiver_retirement_driver.cc",
                "../runtime/framework/input/receiver_lifecycle.cc",
                "../runtime/framework/input/view_root_input_jni.cc",
                "../runtime/framework/looper/message_queue_jni.cc",
                "../runtime/framework/input/input_transport_pump.cc",
                "../runtime/framework/input/input_transport_readiness.cc",
                "../runtime/framework/input/input_routing_domain.cc",
                "../runtime/framework/input/input_routing.cc",
                "../runtime/framework/input/key_character_map_jni.cc",
                "window/native_window_buffer_queue.cc",
                "../runtime/embedding/session_lifetime.cc",
                "graphics/metal_shared_event_provider.mm",
                "graphics/metal_display_backing.mm",
                "graphics/surface_backing_owner.mm",
                "graphics/scanout_diagnostic_capture.mm",
                "graphics/egl_native_fence_owner.cc",
                "graphics/egl_window_surface_owner.cc",
                "graphics/egl_window_backend.cc",
                "graphics/egl_error_state.cc",
            ] {
                assert_eq!(sources.iter().filter(|&&source| source == owner).count(), 1);
            }
        }
        assert!(COMMON_ADAPTER_SOURCES.contains(&"graphics/metal_shared_event_provider.mm"));
        assert!(COMMON_ADAPTER_SOURCES.contains(&"graphics/egl_native_fence_owner.cc"));
        assert!(COMMON_ADAPTER_SOURCES.contains(&"graphics/egl_window_surface_owner.cc"));
        assert!(COMMON_ADAPTER_SOURCES.contains(&"graphics/egl_window_backend.cc"));
    }

    #[test]
    fn graphics_ndk_imports_are_flavor_owned() {
        assert!(GRAPHICS_ADAPTER_SOURCES.contains(&"loader/graphics_ndk_symbols.cc"));
        assert!(!HEADLESS_ADAPTER_SOURCES.contains(&"loader/graphics_ndk_symbols.cc"));
        assert!(!COMMON_ADAPTER_SOURCES.contains(&"loader/graphics_ndk_symbols.cc"));
        for sources in [GRAPHICS_ADAPTER_SOURCES, HEADLESS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "darwin_runtime_elf_resolver.cc")
                    .count(),
                1
            );
        }
        assert!(!COMMON_ADAPTER_SOURCES.contains(&"darwin_runtime_elf_resolver.cc"));
    }

    #[test]
    fn product_sources_exclude_fixture_owners() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            for source in sources {
                assert!(!source.split('/').any(|part| part == "probes"), "{source}");
                assert!(!source.contains("fixture"), "{source}");
                assert!(!source.ends_with("_probe.cc"), "{source}");
                assert!(!source.ends_with("_probe.mm"), "{source}");
            }
        }
    }

    #[test]
    fn blast_and_surface_transaction_have_one_owner_per_runtime() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            for owner in [
                "window/blast_transaction_state.cc",
                "window/blast_buffer_queue_jni.cc",
                "window/surface_transaction_merge.cc",
                "window/surface_transaction_builder.cc",
                "window/surface_transaction_lifetime.cc",
                "window/native_window_transaction_consumer.cc",
                "window/surface_transaction_submission.cc",
            ] {
                assert_eq!(
                    sources.iter().filter(|&&s| s == owner).count(),
                    1,
                    "{owner}"
                );
            }
        }
    }

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
    fn document_panels_have_a_filesystem_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "filesystem/document_panel.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn root_key_policy_and_server_identity_have_distinct_product_owners() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            for owner in [
                "../runtime/framework/input/root_key_authority.cc",
                "../runtime/framework/input/root_key_routing.cc",
                "../runtime/framework/input/root_key_ingress.cc",
                "../runtime/framework/input/root_key_ingress_registry.cc",
                "../runtime/framework/wm/root_key_server_jni.cc",
                "../runtime/framework/wm/root_key_decision_jni.cc",
            ] {
                assert_eq!(sources.iter().filter(|&&s| s == owner).count(), 1);
            }
        }
    }

    #[test]
    fn remote_binder_identity_has_one_jni_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "binder/remote_binder_identity_jni.cc")
                    .count(),
                1
            );
        }
        assert!(!COMMON_ADAPTER_SOURCES.contains(&"binder/remote_binder_identity_jni.cc"));
    }

    #[test]
    fn desktop_root_target_has_an_explicit_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "window/desktop_root_target.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn appkit_content_view_has_an_explicit_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "window/appkit_content_view.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn application_identity_has_a_narrow_host_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "window/application_identity.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn surface_scanout_has_one_narrow_host_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "window/surface_scanout_owner.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn wms_publication_has_an_explicit_owner_in_both_products() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            for owner in [
                "../runtime/framework/wm/window_input_publisher_jni.cc",
                "../runtime/framework/wm/window_input_endpoint_lease.cc",
            ] {
                assert_eq!(sources.iter().filter(|&&source| source == owner).count(), 1);
            }
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "../runtime/framework/input/input_channel_jni.cc")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn focus_action_and_publication_have_distinct_product_objects() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            for owner in [
                "input_routing.cc",
                "input_routing_focus.cc",
                "input_routing_actions.cc",
            ] {
                assert_eq!(
                    sources
                        .iter()
                        .filter(|source| source.ends_with(owner))
                        .count(),
                    1
                );
            }
        }
        assert!(
            !COMMON_ADAPTER_SOURCES
                .contains(&"../runtime/framework/input/input_routing_actions.cc")
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

    #[test]
    fn host_services_has_one_owner_in_every_runtime() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "process/host_services.cc")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn hardware_buffer_storage_has_one_owner_in_every_runtime() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&s| s == "graphics/hardware_buffer_owner.mm")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn framed_input_reader_has_one_owner_in_every_runtime() {
        for sources in [HEADLESS_ADAPTER_SOURCES, GRAPHICS_ADAPTER_SOURCES] {
            assert_eq!(sources.iter().filter(|&&source| source == "../runtime/framework/input/input_framed_reader.cc").count(), 1);
        }
        // The narrow provider archive does not own the transport aggregate.
        assert!(!COMMON_ADAPTER_SOURCES.contains(&"../runtime/framework/input/input_transport.cc"));
        assert!(
            !COMMON_ADAPTER_SOURCES.contains(&"../runtime/framework/input/input_framed_reader.cc")
        );
    }

    #[test]
    fn composition_fence_monitor_has_one_owner_in_every_runtime() {
        for sources in [
            HEADLESS_ADAPTER_SOURCES,
            GRAPHICS_ADAPTER_SOURCES,
            COMMON_ADAPTER_SOURCES,
        ] {
            assert_eq!(
                sources
                    .iter()
                    .filter(|&&source| source == "window/composition_fence_monitor.cc")
                    .count(),
                1
            );
        }
    }
}
