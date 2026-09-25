use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use darwin_art_build_contract::{GRAPHICS_ADAPTER_SOURCES, HEADLESS_ADAPTER_SOURCES};

use super::super::ninja_path;
use super::atomic;

pub(crate) fn graph_inputs(root: &Path) -> Vec<PathBuf> {
    // This digest is the invalidation boundary for native objects, not for
    // the Rust command that happens to emit the Ninja file.  Rust orchestration
    // changes are intentionally excluded here: changing an xtask, bootstrap
    // CLI, or Cargo manifest must not force hundreds of unchanged C++/ObjC++
    // translation units to rebuild.  Bump GRAPH_VERSION when the graph
    // policy/command generation itself changes.
    let mut paths = vec![
        PathBuf::from("sources.lock"),
        // Platform fatal-handler adaptation has a separately staged producer.
        // Track the pristine source and actual archive consumed by final links.
        PathBuf::from("_aosp/art/runtime/runtime_linux.cc"),
        PathBuf::from("_build/runtime-platform/libart-platform-darwin.a"),
        PathBuf::from("compat/filesystem/archive_open.cc"),
        PathBuf::from("bootclasspath.lock"),
        // The canonical builder writes this stamp after preparing the shared
        // runtime cache. Including it prevents a graph emitted before
        // preparation from being reused after the cache becomes promotable.
        PathBuf::from("_build/runtime-common/cache-identity"),
        PathBuf::from("tools/build-android-elf-jni-fixture.sh"),
        PathBuf::from("tools/audit-android16-graphics-closure.sh"),
        // Metal Skia is a separate provider edge; track its shadow recipe and
        // pinned source patches so an ABI patch cannot leave a stale archive.
        PathBuf::from("tools/build-android16-skia-metal-gpu.sh"),
        PathBuf::from("patches/skia/0001-darwin-hwui-disable-coretext-utils.patch"),
        PathBuf::from("patches/skia/0002-darwin-hwui-export-cross-tu-abi.patch"),
        PathBuf::from("patches/skia/0003-darwin-ahb-gl-texture-2d.patch"),
        PathBuf::from("tools/audit-jit-layout.sh"),
        PathBuf::from("tools/jit-layout-smoke.cc"),
        PathBuf::from("probes/runtime_network_probe.cc"),
        PathBuf::from("probes/runtime_network_probe.h"),
        PathBuf::from("probes/runtime_acceptance_phases.cc"),
        PathBuf::from("probes/runtime_acceptance_phases.h"),
        PathBuf::from("probes/runtime_acceptance_state.cc"),
        PathBuf::from("probes/runtime_acceptance_state.h"),
        PathBuf::from("probes/runtime_hwui_probe.cc"),
        PathBuf::from("probes/runtime_hwui_probe.h"),
        PathBuf::from("probes/runtime_entry_probe.cc"),
        PathBuf::from("probes/headless_graphics_fixture_natives.cc"),
        PathBuf::from("probes/headless_graphics_fixture_natives.h"),
        PathBuf::from("probes/headless_resources_fixture_natives.cc"),
        PathBuf::from("probes/headless_resources_fixture_natives.h"),
        PathBuf::from("runtime/art/native_registration.cc"),
        PathBuf::from("runtime/art/native_registration.h"),
        PathBuf::from("runtime/art/boot_native_registration.cc"),
        PathBuf::from("compat/art/boot_native_libraries.cc"),
        PathBuf::from("compat/art/boot_native_libraries.h"),
        PathBuf::from("probes/runtime_registration_fixture.cc"),
        PathBuf::from("probes/runtime_registration_fixture.h"),
        PathBuf::from("probes/runtime_network_loader.cc"),
        PathBuf::from("runtime/art/system_class_loader.cc"),
        PathBuf::from("runtime/art/system_class_loader.h"),
        PathBuf::from("probes/runtime_app_bootstrap.cc"),
        PathBuf::from("probes/runtime_app_bootstrap.h"),
        PathBuf::from("probes/runtime_app_resources.cc"),
        PathBuf::from("probes/runtime_app_resources.h"),
        PathBuf::from("probes/runtime_app_activity.cc"),
        PathBuf::from("probes/runtime_app_activity.h"),
        PathBuf::from("probes/runtime_app_presentation.cc"),
        PathBuf::from("probes/runtime_app_presentation.h"),
        PathBuf::from("probes/runtime_link_probe.cc"),
        PathBuf::from("probes/runtime_elf_probe.cc"),
        PathBuf::from("probes/runtime_elf_probe.h"),
        PathBuf::from("probes/runtime_abi_probe.cc"),
        PathBuf::from("probes/runtime_abi_probe.h"),
        PathBuf::from("runtime/art/process_state.cc"),
        PathBuf::from("runtime/art/process_state.h"),
        PathBuf::from("runtime/art/vm_bootstrap.cc"),
        PathBuf::from("runtime/art/vm_bootstrap.h"),
        PathBuf::from("runtime/embedding/process_config.cc"),
        PathBuf::from("runtime/embedding/process_config.h"),
        PathBuf::from("runtime/embedding/process_entry.cc"),
        PathBuf::from("runtime/embedding/process_entry.h"),
        PathBuf::from("runtime/embedding/process_shutdown.cc"),
        PathBuf::from("runtime/embedding/process_shutdown.h"),
        PathBuf::from("runtime/art/vm_shutdown.cc"),
        PathBuf::from("runtime/art/vm_shutdown.h"),
        PathBuf::from("runtime/framework/app/process_shutdown.cc"),
        PathBuf::from("runtime/framework/app/process_shutdown.h"),
        PathBuf::from("probes/runtime_fixture_options.cc"),
        PathBuf::from("probes/runtime_fixture_options.h"),
        PathBuf::from("compat/jni/scoped_local_frame.h"),
        PathBuf::from("probes/runtime_upstream_test.h"),
        PathBuf::from("probes/runtime_upstream_arttest.cc"),
        PathBuf::from("_aosp/art/test/common/runtime_state.cc"),
        PathBuf::from("_aosp/art/test/common/stack_inspect.cc"),
        PathBuf::from("probes/runtime_jit_invoke_custom.h"),
        PathBuf::from("probes/runtime_jit_specialized_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_string_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_string_hidden_intrinsics.h"),
        PathBuf::from("probes/runtime_jit_system_arraycopy.h"),
        PathBuf::from("probes/runtime_jit_math_hinvoke.h"),
        PathBuf::from("probes/runtime_jit_crc32.h"),
        PathBuf::from("probes/runtime_jit_memory.h"),
        PathBuf::from("probes/runtime_jit_reference_boxing.h"),
        PathBuf::from("probes/runtime_jit_unsafe_intrinsics.h"),
        PathBuf::from("probes/runtime_shutdown_probe.cc"),
        PathBuf::from("probes/runtime_shutdown_probe.h"),
        PathBuf::from("probes/runtime_frame_probe.cc"),
        PathBuf::from("probes/runtime_frame_probe.h"),
        PathBuf::from("probes/runtime_graphics_probe.cc"),
        PathBuf::from("probes/runtime_graphics_probe.h"),
        PathBuf::from("probes/runtime_graphics_gpu.cc"),
        PathBuf::from("probes/runtime_graphics_gpu.h"),
        PathBuf::from("probes/runtime_graphics_phase.cc"),
        PathBuf::from("probes/runtime_graphics_phase.h"),
        PathBuf::from("probes/runtime_graphics_input.cc"),
        PathBuf::from("runtime/framework/input/event_ingress.cc"),
        PathBuf::from("runtime/framework/input/event_ingress.h"),
        PathBuf::from("runtime/framework/input/channel_owner.cc"),
        PathBuf::from("runtime/framework/input/root_key_authority.cc"),
        PathBuf::from("runtime/framework/input/root_key_authority.h"),
        PathBuf::from("runtime/framework/input/root_key_routing.cc"),
        PathBuf::from("runtime/framework/input/root_key_routing.h"),
        PathBuf::from("runtime/framework/input/root_key_ingress.cc"),
        PathBuf::from("runtime/framework/input/root_key_ingress.h"),
        PathBuf::from("runtime/framework/input/root_key_ingress_registry.cc"),
        PathBuf::from("runtime/framework/input/root_key_ingress_registry.h"),
        PathBuf::from("runtime/framework/input/root_key_ingress_lifetime.h"),
        PathBuf::from("runtime/framework/input/receiver_jni.cc"),
        PathBuf::from("runtime/framework/input/receiver_transport_policy.cc"),
        PathBuf::from("runtime/framework/input/receiver_transport_policy.h"),
        PathBuf::from("runtime/framework/input/receiver_input_consumer.cc"),
        PathBuf::from("runtime/framework/input/receiver_input_consumer.h"),
        PathBuf::from("runtime/framework/input/receiver_packet_consumption.cc"),
        PathBuf::from("runtime/framework/input/receiver_packet_consumption.h"),
        PathBuf::from("runtime/framework/input/input_routing_packet_lease.cc"),
        PathBuf::from("runtime/framework/input/input_routing_packet_lease.h"),
        PathBuf::from("runtime/framework/input/receiver_focus_control.cc"),
        PathBuf::from("runtime/framework/input/receiver_focus_control.h"),
        PathBuf::from("runtime/framework/input/receiver_focus_jni.cc"),
        PathBuf::from("runtime/framework/input/receiver_focus_jni.h"),
        PathBuf::from("runtime/framework/input/input_routing_focus.h"),
        PathBuf::from("runtime/framework/input/input_focus_epoch.h"),
        PathBuf::from("runtime/framework/input/channel_resources.cc"),
        PathBuf::from("runtime/framework/input/channel_identity_catalog.cc"),
        PathBuf::from("runtime/framework/input/input_channel_jni.cc"),
        PathBuf::from("runtime/framework/input/input_channel_jni.h"),
        PathBuf::from("runtime/framework/wm/window_input_publisher_jni.cc"),
        PathBuf::from("runtime/framework/wm/desktop_root_client_jni.mm"),
        PathBuf::from("runtime/framework/wm/desktop_root_client_jni.h"),
        PathBuf::from("runtime/framework/wm/desktop_foreground_authority_jni.cc"),
        PathBuf::from("runtime/framework/wm/desktop_foreground_authority_jni.h"),
        PathBuf::from("runtime/framework/wm/root_key_server_jni.cc"),
        PathBuf::from("runtime/framework/wm/root_key_server_jni.h"),
        PathBuf::from("compat/binder/endpoint_lifetime.h"),
        PathBuf::from("compat/binder/native_endpoint_lifetime.h"),
        PathBuf::from("compat/binder/native_endpoint_lifetime.cc"),
        PathBuf::from("runtime/framework/wm/root_key_decision_jni.cc"),
        PathBuf::from("runtime/framework/wm/root_key_decision_jni.h"),
        PathBuf::from("runtime/framework/wm/window_input_publisher_jni.h"),
        PathBuf::from("runtime/framework/wm/window_input_endpoint_lease.cc"),
        PathBuf::from("runtime/framework/wm/window_input_endpoint_lease.h"),
        PathBuf::from("runtime/framework/input/input_channel_parcel.h"),
        PathBuf::from("runtime/framework/input/channel_identity_catalog.h"),
        PathBuf::from("runtime/framework/input/channel_resources.h"),
        PathBuf::from("runtime/framework/input/channel_owner.h"),
        PathBuf::from("runtime/framework/input/receiver_jni.h"),
        PathBuf::from("runtime/framework/input/key_character_map_jni.cc"),
        PathBuf::from("runtime/framework/input/key_character_map_jni.h"),
        PathBuf::from("runtime/framework/input/packet_dispatch.cc"),
        PathBuf::from("runtime/framework/input/packet_dispatch.h"),
        PathBuf::from("runtime/framework/input/framework_dispatch.h"),
        PathBuf::from("runtime/framework/input/receiver_registry.cc"),
        PathBuf::from("runtime/framework/input/receiver_registry.h"),
        PathBuf::from("runtime/framework/input/receiver_admission.cc"),
        PathBuf::from("runtime/framework/input/receiver_admission.h"),
        PathBuf::from("runtime/framework/input/input_transport.cc"),
        PathBuf::from("runtime/framework/input/transport_registration_authority.cc"),
        PathBuf::from("runtime/framework/input/transport_registration_authority.h"),
        PathBuf::from("runtime/framework/input/channel_endpoint.cc"),
        PathBuf::from("runtime/framework/input/finish_ledger.cc"),
        PathBuf::from("runtime/framework/input/finish_ledger.h"),
        PathBuf::from("runtime/framework/input/input_event_origin.h"),
        PathBuf::from("runtime/framework/input/receiver_finish_owner.cc"),
        PathBuf::from("runtime/framework/input/receiver_finish_owner.h"),
        PathBuf::from("runtime/framework/input/channel_endpoint.h"),
        PathBuf::from("runtime/framework/input/input_channel_jni_resources.h"),
        PathBuf::from("runtime/framework/input/routing_transport_dispatch.cc"),
        PathBuf::from("runtime/framework/input/routing_transport_dispatch.h"),
        PathBuf::from("runtime/framework/input/routing_transport_scheduler.cc"),
        PathBuf::from("runtime/framework/input/routing_transport_scheduler.h"),
        PathBuf::from("runtime/framework/input/channel_routing_continuation.cc"),
        PathBuf::from("runtime/framework/input/channel_routing_continuation.h"),
        PathBuf::from("runtime/framework/input/receiver_endpoint_binding.cc"),
        PathBuf::from("runtime/framework/input/claimed_input_transport_pump.cc"),
        PathBuf::from("runtime/framework/input/claimed_input_transport_pump.h"),
        PathBuf::from("runtime/framework/input/input_resource_progress.cc"),
        PathBuf::from("runtime/framework/input/input_resource_progress.h"),
        PathBuf::from("runtime/framework/input/receiver_endpoint_binding.h"),
        PathBuf::from("runtime/framework/input/receiver_jni_resources.cc"),
        PathBuf::from("runtime/framework/input/receiver_jni_resources.h"),
        PathBuf::from("runtime/framework/input/view_root_input_jni.cc"),
        PathBuf::from("runtime/framework/looper/message_queue_jni.cc"),
        PathBuf::from("runtime/framework/looper/message_queue_jni.h"),
        PathBuf::from("runtime/framework/input/view_root_input_jni.h"),
        PathBuf::from("runtime/framework/input/receiver_routing_access.h"),
        PathBuf::from("runtime/framework/input/receiver_routing_lifecycle.cc"),
        PathBuf::from("runtime/framework/input/receiver_routing_lifecycle.h"),
        PathBuf::from("runtime/framework/input/receiver_retirement_barrier.cc"),
        PathBuf::from("runtime/framework/input/receiver_retirement_barrier.h"),
        PathBuf::from("runtime/framework/input/pending_receiver_retirement.cc"),
        PathBuf::from("runtime/framework/input/pending_receiver_retirement.h"),
        PathBuf::from("runtime/framework/input/receiver_retirement_driver.cc"),
        PathBuf::from("runtime/framework/input/receiver_retirement_driver.h"),
        PathBuf::from("runtime/framework/input/receiver_lifecycle.cc"),
        PathBuf::from("runtime/framework/input/receiver_lifecycle.h"),
        PathBuf::from("runtime/framework/input/input_transport.h"),
        PathBuf::from("runtime/framework/input/input_transport_wire.h"),
        PathBuf::from("runtime/framework/input/input_transport_pump.cc"),
        PathBuf::from("runtime/framework/input/input_transport_readiness.cc"),
        PathBuf::from("runtime/framework/input/input_transport_pump.h"),
        PathBuf::from("runtime/framework/input/input_transport_readiness.h"),
        PathBuf::from("runtime/framework/input/input_routing.cc"),
        PathBuf::from("runtime/framework/input/input_routing_focus.cc"),
        PathBuf::from("runtime/framework/input/input_routing_actions.cc"),
        PathBuf::from("runtime/framework/input/input_routing_actions_internal.h"),
        PathBuf::from("runtime/framework/input/input_routing_state_internal.h"),
        PathBuf::from("runtime/framework/input/input_routing_domain.cc"),
        PathBuf::from("runtime/framework/input/input_routing_domain.h"),
        PathBuf::from("runtime/framework/input/input_window_state.h"),
        PathBuf::from("runtime/framework/input/input_routing.h"),
        PathBuf::from("runtime/framework/display/vsync_source.cc"),
        PathBuf::from("runtime/framework/display/vsync_source.h"),
        PathBuf::from("runtime/embedding/graphics_state.cc"),
        PathBuf::from("runtime/embedding/graphics_state.h"),
        PathBuf::from("probes/graphics_fixture_state.cc"),
        PathBuf::from("probes/graphics_fixture_state.h"),
        PathBuf::from("runtime/embedding/graphics_session.cc"),
        PathBuf::from("runtime/embedding/graphics_session.h"),
        PathBuf::from("runtime/embedding/session_lifetime.cc"),
        PathBuf::from("runtime/embedding/session_lifetime.h"),
        PathBuf::from("compat/graphics/metal_shared_event_provider.mm"),
        PathBuf::from("compat/graphics/metal_display_backing.mm"),
        PathBuf::from("compat/graphics/surface_backing_owner.mm"),
        PathBuf::from("compat/graphics/surface_backing_owner.h"),
        PathBuf::from("compat/graphics/borrowed_texture_backing.h"),
        PathBuf::from("compat/graphics/scanout_diagnostic_capture.h"),
        PathBuf::from("compat/graphics/scanout_diagnostic_capture.mm"),
        PathBuf::from("compat/graphics/metal_shared_event_provider.h"),
        PathBuf::from("compat/graphics/metal_display_backing.h"),
        PathBuf::from("compat/graphics/egl_native_fence_owner.cc"),
        PathBuf::from("compat/graphics/egl_native_fence_owner.h"),
        PathBuf::from("compat/graphics/egl_window_surface_owner.cc"),
        PathBuf::from("compat/graphics/egl_window_surface_owner.h"),
        PathBuf::from("compat/graphics/egl_window_backend.cc"),
        PathBuf::from("compat/graphics/egl_error_state.cc"),
        PathBuf::from("compat/graphics/egl_error_state.h"),
        PathBuf::from("compat/graphics/egl_window_backend.h"),
        PathBuf::from("runtime/framework/input/input_routing_endpoint.h"),
        PathBuf::from("compat/window/native_window_transaction_consumer.cc"),
        PathBuf::from("compat/window/native_window_transaction_consumer.h"),
        PathBuf::from("compat/window/native_window_buffer_queue.cc"),
        PathBuf::from("compat/window/native_window_buffer_queue.h"),
        PathBuf::from("compat/window/remote_surface_producer.cc"),
        PathBuf::from("compat/window/remote_surface_producer.h"),
        PathBuf::from("compat/window/surface_transaction_builder.cc"),
        PathBuf::from("compat/window/surface_transaction_builder.h"),
        PathBuf::from("probes/runtime_graphics_vsync_diagnostic.cc"),
        PathBuf::from("probes/runtime_graphics_cpu_stubs.cc"),
        PathBuf::from("probes/runtime_jni_acceptance_probe.cc"),
        PathBuf::from("probes/runtime_jni_acceptance_probe.h"),
        // The JNI acceptance TU includes this transitive JIT checkpoint
        // fixture; track it explicitly so header-only regression changes
        // invalidate the native graph and cannot leave a stale probe object.
        PathBuf::from("probes/runtime_jit_loop_checkpoint.h"),
        PathBuf::from("probes/runtime_graphics_probe_internal.h"),
        PathBuf::from("probes/runtime_apk_graph.cc"),
        PathBuf::from("probes/runtime_apk_graph.h"),
        PathBuf::from("compat/darwin_surface_bridge.mm"),
        PathBuf::from("compat/darwin_surface_bridge.h"),
        PathBuf::from("compat/filesystem/document_panel.mm"),
        PathBuf::from("compat/filesystem/document_panel.h"),
        PathBuf::from("compat/input/darwin_hardware_key_translation.mm"),
        PathBuf::from("compat/input/darwin_hardware_key_translation.h"),
        PathBuf::from("compat/window/desktop_root_events.mm"),
        PathBuf::from("compat/window/desktop_root_events.h"),
        PathBuf::from("compat/window/desktop_foreground_provider.cc"),
        PathBuf::from("compat/window/desktop_foreground_provider.h"),
        PathBuf::from("compat/window/desktop_root_target.mm"),
        PathBuf::from("compat/window/desktop_root_target.h"),
        PathBuf::from("compat/window/desktop_root_surface.mm"),
        PathBuf::from("compat/window/desktop_root_surface.h"),
        PathBuf::from("compat/window/appkit_window_delegate.mm"),
        PathBuf::from("compat/window/appkit_window_delegate.h"),
        PathBuf::from("compat/window/appkit_content_view.mm"),
        PathBuf::from("compat/window/appkit_content_view.h"),
        PathBuf::from("compat/window/application_identity.mm"),
        PathBuf::from("compat/window/application_identity.h"),
        PathBuf::from("compat/window/surface_scanout_owner.mm"),
        PathBuf::from("compat/window/surface_scanout_owner.h"),
        PathBuf::from("compat/darwin_surface_internal.h"),
        PathBuf::from("compat/darwin_surface_gpu_bridge.mm"),
        PathBuf::from("compat/darwin_audio_track.h"),
        PathBuf::from("compat/darwin_android_asset_manager.h"),
        PathBuf::from("compat/darwin_android_platform.h"),
        PathBuf::from("compat/network/multinetwork.h"),
        PathBuf::from("compat/darwin_provider_owners.cc"),
        PathBuf::from("compat/darwin_provider_owners.h"),
        // Remote unwind's Mach task cache is header-only; keep it explicit so
        // retry/permission changes invalidate the provider object and cannot
        // silently reuse a stale graphics archive.
        PathBuf::from("compat/darwin_remote_task_cache.h"),
        PathBuf::from("compat/darwin_unwindstack_linux_abi.h"),
        // The libcore Linux archive is produced by a standalone shell edge,
        // so its C++ inputs must be explicit graph inputs rather than relying
        // on the shell script's timestamp to represent all three objects.
        // This keeps system/metadata edits and syscall edits incremental while
        // preserving the generated 135-entry registrar boundary.
        PathBuf::from("compat/libcore_darwin_linux.cc"),
        PathBuf::from("compat/libcore_darwin_linux_system_natives.cc"),
        PathBuf::from("compat/libcore_darwin_linux_syscalls.cc"),
        PathBuf::from("compat/libcore_darwin_linux.h"),
        PathBuf::from("compat/darwin_asynchronous_close_monitor.cc"),
        PathBuf::from("probes/android16_asynchronous_close_monitor_smoke.cc"),
        PathBuf::from("probes/android16_asynchronous_close_monitor_jni.cc"),
        PathBuf::from("compat/darwin_libcore_filesystem_bridge.c"),
        PathBuf::from("compat/darwin_openjdk_nio_copy.c"),
        PathBuf::from("compat/darwin_openjdk_nio_fs_redirect.h"),
        PathBuf::from("probes/android16_unix_filesystem_jni.c"),
        PathBuf::from("probes/unix-filesystem/UnixFileSystemDarwinSmoke.java"),
        PathBuf::from("upstream/android16-libcore-darwin-linux.lock"),
        PathBuf::from("upstream/android16-asynchronous-close-monitor.lock"),
        PathBuf::from("upstream/android16-os-constants.lock"),
        PathBuf::from("upstream/android16-os-constants-values.tsv"),
        PathBuf::from("upstream/android16-unix-filesystem-darwin.lock"),
        PathBuf::from("upstream/android16-system-natives-darwin.lock"),
        PathBuf::from("tools/bionic-errno-tls/include/darwin_art_bionic_errno.h"),
    ];
    // Keep this graph tied to the production bootstrap closure. Acceptance
    // probes and unrelated native sources must not rotate the persistent
    // runtime cache identity. The adapter manifest is the source of truth for
    // implementation TUs; compatibility headers/includes are still collected
    // recursively because they participate in compiler dependency checks.
    let mut adapter_sources = HEADLESS_ADAPTER_SOURCES
        .iter()
        .chain(GRAPHICS_ADAPTER_SOURCES.iter())
        .map(|source| PathBuf::from("compat").join(source))
        .collect::<Vec<_>>();
    adapter_sources.sort();
    adapter_sources.dedup();
    paths.extend(adapter_sources);
    for directory in [
        "include",
        "patches/art",
        "patches/boringssl",
        "patches/libcore-openjdk",
        "patches/application-shared-memory",
        "patches/ziparchive",
        "patches/package-dex-usage",
        "tools/tests/package-dex-usage",
        "crates/darwin-art-elf-loader/src",
        "tools/android-jni-proxy/include",
        "tools/android-jni-proxy/generated",
        "tools/android-dl-iterate-phdr-provider/include",
        "tools/bionic-dns-facade/include",
        "tools/bionic-dso-lifecycle-facade/include",
        "tools/bionic-fs-facade/include",
        "tools/bionic-ioctl-facade/include",
        "tools/bionic-provider-namespace/include",
        "tools/bionic-sendfile-facade/include",
        "tools/bionic-socket-broker-adapter/include",
        "tools/bionic-stdio-facade/include",
        "tools/bionic-strftime-facade/include",
    ] {
        collect_files(&root.join(directory), root, &mut paths);
    }
    collect_compat_support_files(&root.join("compat"), root, &mut paths);
    paths.push(root.join("platform/darwin/android_runtime_host.cc"));
    paths
        .push(root.join("_aosp/frameworks/base/core/jni/include/android_runtime/AndroidRuntime.h"));
    collect_files(&root.join("tools/system-properties"), root, &mut paths);
    paths.extend(super::input_keymaps::inputs(root));
    for script in [
        "build-bionic-runtime-provider-closure.sh",
        "build-android16-asynchronous-close-monitor.sh",
        "build-android16-libcore-darwin-linux.sh",
        "build-android16-os-constants-darwin.sh",
        "build-android16-unix-filesystem-darwin.sh",
        "build-android16-openjdkjvm-darwin.sh",
        "build-android16-file-input-stream-darwin.sh",
        "build-android16-file-descriptor-darwin.sh",
        "build-android16-system-natives-darwin.sh",
        "build-android16-openjdk-named-jni-owner.sh",
        "build-android16-unix-native-dispatcher-darwin.sh",
        "build-android16-openjdk-nio-mapping.sh",
        "build-android16-libcore-memory-darwin.sh",
        "build-android16-android-util-log.sh",
        "build-android16-virtual-ref-base-ptr.sh",
        "build-android16-application-shared-memory.sh",
        "build-android16-debugstore.sh",
        "build-android16-activity-thread.sh",
        "build-android16-system-properties.sh",
        "build-android16-tracing-perfetto.sh",
    ] {
        paths.push(PathBuf::from("tools").join(script));
    }
    paths.extend([
        PathBuf::from("upstream/android16-application-shared-memory.lock"),
        PathBuf::from("upstream/android16-debugstore.lock"),
        PathBuf::from("upstream/android16-activity-thread.lock"),
        PathBuf::from("upstream/android16-system-properties.lock"),
        PathBuf::from("tools/activity-thread/bindings.h"),
        PathBuf::from("runtime/framework/app/activity_thread_jni.h"),
        PathBuf::from("compat/loader/android_dlwarning.cc"),
        PathBuf::from("upstream/android16-bionic-linker-config.sources"),
        PathBuf::from("upstream/android16-tracing-perfetto.lock"),
        PathBuf::from("upstream/android16-tracing-perfetto.sources"),
        PathBuf::from("patches/tracing-perfetto/0001-explicit-dynamic-track-name.patch"),
        PathBuf::from("tools/tracing-perfetto-smoke.cc"),
        PathBuf::from("tools/debugstore/Cargo.toml"),
        PathBuf::from("tools/debugstore/Cargo.lock"),
        PathBuf::from("tools/debugstore/build.rs"),
        PathBuf::from("tools/debugstore/smoke.cc"),
        PathBuf::from("compat/memory/application_memory.cc"),
        PathBuf::from("compat/memory/application_descriptor.cc"),
        PathBuf::from("compat/memory/application_descriptor.h"),
        PathBuf::from("compat/memory/shared_memory_handle.h"),
        PathBuf::from("compat/binder/context_manager.h"),
        PathBuf::from("compat/loader/classloader_identity.h"),
        PathBuf::from("compat/loader/library_search.h"),
        PathBuf::from("compat/loader/elf_graph_cache.h"),
        PathBuf::from("compat/loader/namespace_elf_group.h"),
        PathBuf::from("compat/window/locked_surface.h"),
        PathBuf::from("compat/window/hardware_buffer_jni.h"),
        PathBuf::from("runtime/framework/wm/client_transaction.h"),
        PathBuf::from("runtime/framework/wm/window_title.h"),
        PathBuf::from("runtime/framework/app/main_loop.h"),
        PathBuf::from("runtime/framework/app/process_entry.h"),
        PathBuf::from("runtime/framework/system/process_entry.h"),
        PathBuf::from("runtime/framework/connectivity/network_path_abi.h"),
        PathBuf::from("runtime/framework/connectivity/network_provider_jni.h"),
        PathBuf::from("runtime/framework/power/power_state_platform.h"),
        PathBuf::from("runtime/framework/power/power_state_jni.h"),
        PathBuf::from("runtime/framework/app/process_registration.h"),
        PathBuf::from("runtime/framework/pm/DexInstructionSets.java"),
        PathBuf::from("runtime/framework/pm/DexUsageStore.java"),
        PathBuf::from("runtime/framework/pm/DexLoadReports.java"),
        PathBuf::from("runtime/framework/pm/InstalledPackageInfos.java"),
        PathBuf::from("runtime/framework/pm/InstalledPackageParser.java"),
        PathBuf::from("runtime/framework/pm/InstalledPackageRecord.java"),
        PathBuf::from("runtime/framework/pm/installed_record_source.h"),
        PathBuf::from("runtime/framework/pm/PackageManagerEndpoint.java"),
        PathBuf::from("runtime/framework/pm/PackageRecords.java"),
        PathBuf::from("upstream/android16-package-dex-usage.lock"),
        PathBuf::from("tools/build-android16-package-dex-usage.sh"),
        PathBuf::from("tools/test-android16-package-dex-usage.sh"),
        PathBuf::from("tools/binder-context-manager-test.cc"),
        PathBuf::from("tools/tests/application-descriptor-table.h"),
        PathBuf::from("compat/memory/system_region.cc"),
        PathBuf::from("tools/application-shared-memory-layout-test.cc"),
        PathBuf::from("tools/system-region-test.cc"),
        PathBuf::from("probes/android-elf-jni-fixture/child.c"),
        PathBuf::from("probes/android-elf-jni-fixture/child.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_child.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_child.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_grandchild.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_grandchild.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_root.c"),
        PathBuf::from("probes/android-elf-jni-fixture/generic_root.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/grandchild.c"),
        PathBuf::from("probes/android-elf-jni-fixture/grandchild.exports.map"),
        PathBuf::from("probes/android-elf-jni-fixture/host_provider.c"),
        PathBuf::from("probes/android-elf-jni-fixture/native_fixture.c"),
        PathBuf::from("tools/android-jni-proxy/src/proxy.c"),
        PathBuf::from("tools/android-jni-proxy/src/aapcs64_call.S"),
        PathBuf::from("tools/android-jni-proxy/sources.lock"),
    ]);
    paths.sort();
    paths.dedup();
    paths.retain(|path| root.join(path).is_file());
    paths
}

pub(crate) fn is_fixture_input(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "probes")
        || path.starts_with("tools/tests")
        || path.starts_with("_aosp/art/test")
        || path == Path::new("tools/build-android-elf-jni-fixture.sh")
}

// Before per-object depfiles exist, the canonical bootstrap producer must
// observe narrow-owner headers too. They remain outside the broad cache digest.
pub(crate) fn is_bootstrap_fallback_header(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "h")
        && is_global_digest_excluded(path)
        && !is_fixture_input(path)
}

// Canonical fallback compilation reads every declared product adapter even
// before its cached per-object producer can be emitted. Keep these sources
// explicit rather than relying on the intentionally scoped ART digest.
pub(crate) fn bootstrap_fallback_adapter_sources() -> Vec<PathBuf> {
    let mut sources = HEADLESS_ADAPTER_SOURCES
        .iter()
        .chain(GRAPHICS_ADAPTER_SOURCES.iter())
        .map(|source| PathBuf::from("compat").join(source))
        .collect::<Vec<_>>();
    sources.sort();
    sources.dedup();
    sources
}

#[cfg(test)]
mod fallback_header_tests {
    use super::*;

    #[test]
    fn newly_scoped_adapter_sources_remain_fallback_dependencies() {
        let sources = bootstrap_fallback_adapter_sources();
        assert!(sources.contains(&PathBuf::from(
            "compat/input/darwin_hardware_key_translation.mm"
        )));
        assert!(sources.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(sources.iter().all(|source| !is_fixture_input(source)));
        for source in HEADLESS_ADAPTER_SOURCES
            .iter()
            .chain(GRAPHICS_ADAPTER_SOURCES)
        {
            assert!(sources.contains(&PathBuf::from("compat").join(source)));
        }
    }

    #[test]
    fn separated_product_headers_remain_fallback_dependencies() {
        for header in [
            "runtime/framework/input/channel_endpoint.h",
            "runtime/framework/input/input_channel_jni_resources.h",
            "runtime/framework/input/routing_transport_dispatch.h",
        ] {
            assert!(is_bootstrap_fallback_header(Path::new(header)));
        }
        assert!(!is_bootstrap_fallback_header(Path::new(
            "probes/runtime_hwui_probe.h"
        )));
        assert!(!is_bootstrap_fallback_header(Path::new(
            "runtime/framework/input/channel_endpoint.cc"
        )));
    }
}

// Excluding a scoped production owner from the broad bootstrap digest does
// not make it a fixture. Its own content stamp/depfiles own invalidation.
pub(crate) fn is_global_digest_excluded(path: &Path) -> bool {
    if path.starts_with("probes") || path == Path::new("tools/build-android-elf-jni-fixture.sh") {
        return true;
    }
    matches!(
        path.to_string_lossy().as_ref(),
        "probes/runtime_network_probe.cc"
            | "probes/runtime_network_probe.h"
            | "probes/runtime_acceptance_phases.cc"
            | "probes/runtime_acceptance_phases.h"
            | "probes/runtime_acceptance_state.cc"
            | "probes/runtime_acceptance_state.h"
            | "probes/runtime_hwui_probe.cc"
            | "probes/runtime_hwui_probe.h"
            | "probes/runtime_entry_probe.cc"
            | "runtime/art/native_registration.cc"
            | "runtime/art/native_registration.h"
            | "runtime/art/boot_native_registration.cc"
            | "compat/art/boot_native_libraries.cc"
            | "probes/runtime_registration_fixture.cc"
            | "probes/runtime_registration_fixture.h"
            | "probes/runtime_app_bootstrap.cc"
            | "probes/runtime_app_bootstrap.h"
            | "probes/runtime_app_resources.cc"
            | "probes/runtime_app_resources.h"
            | "probes/runtime_app_activity.cc"
            | "probes/runtime_app_activity.h"
            | "probes/runtime_app_presentation.cc"
            | "probes/runtime_app_presentation.h"
            | "probes/runtime_link_probe.cc"
            | "probes/runtime_elf_probe.cc"
            | "probes/runtime_elf_probe.h"
            | "probes/runtime_abi_probe.cc"
            | "probes/runtime_abi_probe.h"
            | "runtime/art/process_state.cc"
            | "runtime/art/process_state.h"
            | "probes/runtime_fixture_options.cc"
            | "probes/runtime_fixture_options.h"
            | "runtime/art/system_class_loader.cc"
            | "runtime/art/system_class_loader.h"
            | "compat/jni/scoped_local_frame.h"
            | "probes/runtime_upstream_test.h"
            | "probes/runtime_upstream_arttest.cc"
            | "_aosp/art/test/common/runtime_state.cc"
            | "_aosp/art/test/common/stack_inspect.cc"
            | "probes/runtime_jit_invoke_custom.h"
            | "probes/runtime_jit_specialized_intrinsics.h"
            | "probes/runtime_jit_string_intrinsics.h"
            | "probes/runtime_jit_string_hidden_intrinsics.h"
            | "probes/runtime_jit_system_arraycopy.h"
            | "probes/runtime_jit_math_hinvoke.h"
            | "probes/runtime_jit_crc32.h"
            | "probes/runtime_jit_memory.h"
            | "probes/runtime_jit_reference_boxing.h"
            | "probes/runtime_jit_unsafe_intrinsics.h"
            | "probes/runtime_shutdown_probe.cc"
            | "probes/runtime_shutdown_probe.h"
            | "probes/runtime_frame_probe.cc"
            | "probes/runtime_frame_probe.h"
            | "probes/runtime_graphics_probe.cc"
            | "probes/runtime_graphics_probe.h"
            | "probes/runtime_graphics_gpu.cc"
            | "probes/runtime_graphics_gpu.h"
            | "probes/runtime_graphics_phase.cc"
            | "probes/runtime_graphics_phase.h"
            | "probes/runtime_graphics_input.cc"
            | "runtime/framework/input/event_ingress.cc"
            | "runtime/framework/input/event_ingress.h"
            | "runtime/framework/input/channel_owner.cc"
            | "runtime/framework/input/receiver_jni.cc"
            | "runtime/framework/input/receiver_transport_policy.cc"
            | "runtime/framework/input/receiver_transport_policy.h"
            | "runtime/framework/input/receiver_input_consumer.cc"
            | "runtime/framework/input/receiver_input_consumer.h"
            | "runtime/framework/input/receiver_packet_consumption.cc"
            | "runtime/framework/input/receiver_packet_consumption.h"
            | "runtime/framework/input/input_routing_packet_lease.cc"
            | "runtime/framework/input/input_routing_packet_lease.h"
            | "runtime/framework/input/receiver_focus_control.cc"
            | "runtime/framework/input/receiver_focus_control.h"
            | "runtime/framework/input/receiver_focus_jni.cc"
            | "runtime/framework/input/receiver_focus_jni.h"
            | "runtime/framework/input/input_routing_focus.h"
            | "runtime/framework/input/input_focus_epoch.h"
            | "runtime/framework/input/channel_resources.cc"
            | "runtime/framework/input/channel_identity_catalog.cc"
            | "runtime/framework/input/input_channel_jni.cc"
            | "runtime/framework/input/input_channel_jni.h"
            | "runtime/framework/wm/window_input_publisher_jni.cc"
            | "runtime/framework/wm/desktop_root_client_jni.mm"
            | "runtime/framework/wm/desktop_root_client_jni.h"
            | "runtime/framework/wm/desktop_foreground_authority_jni.cc"
            | "runtime/framework/wm/desktop_foreground_authority_jni.h"
            | "runtime/framework/wm/root_key_server_jni.cc"
            | "runtime/framework/wm/root_key_server_jni.h"
            | "compat/binder/endpoint_lifetime.h"
            | "compat/binder/native_endpoint_lifetime.h"
            | "compat/binder/native_endpoint_lifetime.cc"
            | "runtime/framework/wm/root_key_decision_jni.cc"
            | "runtime/framework/wm/root_key_decision_jni.h"
            | "runtime/framework/input/root_key_authority.cc"
            | "runtime/framework/input/root_key_authority.h"
            | "runtime/framework/input/root_key_routing.cc"
            | "runtime/framework/input/root_key_routing.h"
            | "runtime/framework/input/root_key_ingress.cc"
            | "runtime/framework/input/root_key_ingress.h"
            | "runtime/framework/input/root_key_ingress_registry.cc"
            | "runtime/framework/input/root_key_ingress_registry.h"
            | "runtime/framework/input/root_key_ingress_lifetime.h"
            | "runtime/framework/wm/window_input_publisher_jni.h"
            | "runtime/framework/wm/window_input_endpoint_lease.cc"
            | "runtime/framework/wm/window_input_endpoint_lease.h"
            | "runtime/framework/input/input_channel_parcel.h"
            | "runtime/framework/input/channel_identity_catalog.h"
            | "runtime/framework/input/channel_resources.h"
            | "runtime/framework/input/channel_owner.h"
            | "runtime/framework/input/receiver_jni.h"
            | "runtime/framework/input/key_character_map_jni.cc"
            | "runtime/framework/input/key_character_map_jni.h"
            | "runtime/framework/input/packet_dispatch.cc"
            | "runtime/framework/input/packet_dispatch.h"
            | "runtime/framework/input/framework_dispatch.h"
            | "runtime/framework/input/receiver_registry.cc"
            | "runtime/framework/input/receiver_registry.h"
            | "runtime/framework/input/receiver_admission.cc"
            | "runtime/framework/input/receiver_admission.h"
            | "runtime/framework/input/input_transport.cc"
            | "runtime/framework/input/transport_registration_authority.cc"
            | "runtime/framework/input/transport_registration_authority.h"
            | "runtime/framework/input/channel_endpoint.cc"
            | "runtime/framework/input/finish_ledger.cc"
            | "runtime/framework/input/finish_ledger.h"
            | "runtime/framework/input/input_event_origin.h"
            | "runtime/framework/input/receiver_finish_owner.cc"
            | "runtime/framework/input/receiver_finish_owner.h"
            | "runtime/framework/input/channel_endpoint.h"
            | "runtime/framework/input/input_channel_jni_resources.h"
            | "runtime/framework/input/routing_transport_dispatch.cc"
            | "runtime/framework/input/routing_transport_dispatch.h"
            | "runtime/framework/input/routing_transport_scheduler.cc"
            | "runtime/framework/input/routing_transport_scheduler.h"
            | "runtime/framework/input/channel_routing_continuation.cc"
            | "runtime/framework/input/channel_routing_continuation.h"
            | "runtime/framework/input/receiver_endpoint_binding.cc"
            | "runtime/framework/input/claimed_input_transport_pump.cc"
            | "runtime/framework/input/claimed_input_transport_pump.h"
            | "runtime/framework/input/input_resource_progress.cc"
            | "runtime/framework/input/input_resource_progress.h"
            | "runtime/framework/input/receiver_endpoint_binding.h"
            | "runtime/framework/input/receiver_jni_resources.cc"
            | "runtime/framework/input/receiver_jni_resources.h"
            | "runtime/framework/input/view_root_input_jni.cc"
            | "runtime/framework/looper/message_queue_jni.cc"
            | "runtime/framework/looper/message_queue_jni.h"
            | "runtime/framework/input/view_root_input_jni.h"
            | "runtime/framework/input/receiver_routing_access.h"
            | "runtime/framework/input/receiver_routing_lifecycle.cc"
            | "runtime/framework/input/receiver_routing_lifecycle.h"
            | "runtime/framework/input/receiver_retirement_barrier.cc"
            | "runtime/framework/input/receiver_retirement_barrier.h"
            | "runtime/framework/input/pending_receiver_retirement.cc"
            | "runtime/framework/input/pending_receiver_retirement.h"
            | "runtime/framework/input/receiver_retirement_driver.cc"
            | "runtime/framework/input/receiver_retirement_driver.h"
            | "runtime/framework/input/receiver_lifecycle.cc"
            | "runtime/framework/input/receiver_lifecycle.h"
            | "runtime/framework/input/input_transport.h"
            | "runtime/framework/input/input_transport_wire.h"
            | "runtime/framework/input/input_transport_pump.cc"
            | "runtime/framework/input/input_transport_readiness.cc"
            | "runtime/framework/input/input_transport_pump.h"
            | "runtime/framework/input/input_transport_readiness.h"
            | "runtime/framework/input/input_routing.cc"
            | "runtime/framework/input/input_routing_focus.cc"
            | "runtime/framework/input/input_routing_actions.cc"
            | "runtime/framework/input/input_routing_actions_internal.h"
            | "runtime/framework/input/input_routing_state_internal.h"
            | "runtime/framework/input/input_routing_domain.cc"
            | "runtime/framework/input/input_routing_domain.h"
            | "runtime/framework/input/input_window_state.h"
            | "runtime/framework/input/input_routing.h"
            | "runtime/framework/display/vsync_source.cc"
            | "runtime/framework/display/vsync_source.h"
            | "probes/graphics_fixture_state.cc"
            | "probes/graphics_fixture_state.h"
            | "runtime/embedding/graphics_session.cc"
            | "runtime/embedding/graphics_session.h"
            | "runtime/embedding/session_lifetime.cc"
            | "runtime/embedding/session_lifetime.h"
            | "compat/graphics/metal_shared_event_provider.mm"
            | "compat/graphics/metal_display_backing.mm"
            | "compat/graphics/surface_backing_owner.mm"
            | "compat/graphics/surface_backing_owner.h"
            | "compat/graphics/borrowed_texture_backing.h"
            | "compat/graphics/scanout_diagnostic_capture.h"
            | "compat/graphics/scanout_diagnostic_capture.mm"
            | "compat/graphics/metal_shared_event_provider.h"
            | "compat/graphics/metal_display_backing.h"
            | "compat/graphics/egl_native_fence_owner.cc"
            | "compat/graphics/egl_native_fence_owner.h"
            | "compat/graphics/egl_window_surface_owner.cc"
            | "compat/graphics/egl_window_surface_owner.h"
            | "compat/graphics/egl_window_backend.cc"
            | "compat/graphics/egl_error_state.cc"
            | "compat/graphics/egl_error_state.h"
            | "compat/graphics/egl_window_backend.h"
            | "runtime/framework/input/input_routing_endpoint.h"
            | "compat/window/native_window_transaction_consumer.cc"
            | "compat/window/native_window_transaction_consumer.h"
            | "compat/window/native_window_buffer_queue.cc"
            | "compat/window/native_window_buffer_queue.h"
            | "compat/window/remote_surface_producer.cc"
            | "compat/window/remote_surface_producer.h"
            | "compat/window/surface_transaction_builder.cc"
            | "compat/window/surface_transaction_builder.h"
            | "probes/runtime_graphics_vsync_diagnostic.cc"
            | "probes/runtime_jni_acceptance_probe.cc"
            | "probes/runtime_jni_acceptance_probe.h"
            | "probes/runtime_graphics_probe_internal.h"
            | "probes/runtime_apk_graph.cc"
            | "probes/runtime_apk_graph.h"
            | "compat/darwin_surface_bridge.mm"
            | "compat/darwin_surface_bridge.h"
            | "compat/filesystem/document_panel.mm"
            | "compat/filesystem/document_panel.h"
            | "compat/input/darwin_hardware_key_translation.mm"
            | "compat/input/darwin_hardware_key_translation.h"
            | "compat/window/desktop_root_events.mm"
            | "compat/window/desktop_root_events.h"
            | "compat/window/desktop_foreground_provider.cc"
            | "compat/window/desktop_foreground_provider.h"
            | "compat/window/desktop_root_target.mm"
            | "compat/window/desktop_root_target.h"
            | "compat/window/desktop_root_surface.mm"
            | "compat/window/desktop_root_surface.h"
            | "compat/window/appkit_window_delegate.mm"
            | "compat/window/appkit_window_delegate.h"
            | "compat/window/appkit_content_view.mm"
            | "compat/window/appkit_content_view.h"
            | "compat/window/application_identity.mm"
            | "compat/window/application_identity.h"
            | "compat/window/surface_scanout_owner.mm"
            | "compat/window/surface_scanout_owner.h"
            | "compat/darwin_surface_internal.h"
            | "compat/darwin_surface_gpu_bridge.mm"
    )
}

pub(crate) fn collect_files(directory: &Path, root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Finder's directory presentation metadata is not a build input.
        // Keep this exact-name exclusion narrow: dot-prefixed source/config
        // files and genuine audit inputs still belong to their producers.
        if path.file_name().is_some_and(|name| name == ".DS_Store") {
            continue;
        }
        if path.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("target" | ".git" | "__pycache__")
            ) {
                continue;
            }
            collect_files(&path, root, output);
        } else if path.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            output.push(relative.to_path_buf());
        }
    }
}

#[cfg(test)]
mod collector_tests {
    use super::*;

    #[test]
    fn finder_metadata_does_not_widen_producer_inputs() {
        let root = std::env::temp_dir().join(format!(
            "darwin-art-collector-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("nested")).unwrap();
        for name in [
            ".DS_Store",
            "nested/.DS_Store",
            "nested/provider.c",
            "nested/.config",
            "nested/audit.sh",
        ] {
            fs::write(root.join(name), b"fixture").unwrap();
        }
        let mut inputs = Vec::new();
        collect_files(&root, &root, &mut inputs);
        inputs.sort();
        assert_eq!(
            inputs,
            vec![
                PathBuf::from("nested/.config"),
                PathBuf::from("nested/audit.sh"),
                PathBuf::from("nested/provider.c")
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}

/// Collect compatibility headers and generated include fragments without
/// reintroducing every native implementation into the global graph digest.
/// Implementation TUs are listed explicitly by the shared adapter contract;
/// depfiles remain authoritative for any transitive include discovered by the
/// compiler.
fn collect_compat_support_files(directory: &Path, root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_compat_support_files(&path, root, output);
            continue;
        }
        let is_support = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "h" | "hh" | "hpp" | "inc"));
        if is_support && let Ok(relative) = path.strip_prefix(root) {
            output.push(relative.to_path_buf());
        }
    }
}

pub(crate) fn probe_inputs(root: &Path, paths: &[&str]) -> String {
    probe_input_paths(root, paths)
        .into_iter()
        .map(|path| ninja_path(&path))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn probe_input_paths(root: &Path, paths: &[&str]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| root.join(path))
        .filter(|path| path.is_file())
        .collect()
}

/// Materialize a stable content identity for one narrow probe phase.
///
/// The stamp is updated only when the bytes or input path set changes, so a
/// repeated graph generation remains a true Ninja warm no-op.  Keeping one
/// stamp per phase is important: changing graphics state must not make the
/// graphics input/session objects dirty merely because the global graph
/// digest changed.
pub(crate) fn probe_content_stamp(root: &Path, name: &str, paths: &[&str]) -> io::Result<PathBuf> {
    content_stamp(root, "_build/runtime-probes/content-stamps", name, paths)
}

pub(crate) fn native_owner_content_stamp(
    root: &Path,
    name: &str,
    paths: &[&str],
) -> io::Result<PathBuf> {
    if paths.iter().any(|path| is_fixture_input(Path::new(path))) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "production owner content identity cannot depend on fixtures",
        ));
    }
    content_stamp(root, "_build/native-runtime/content-stamps", name, paths)
}

fn content_stamp(root: &Path, directory: &str, name: &str, paths: &[&str]) -> io::Result<PathBuf> {
    let inputs = probe_input_paths(root, paths);
    let mut digest = Sha256::new();
    digest.update(b"darwin-art-probe-content-v1\0");
    for path in &inputs {
        let relative = path.strip_prefix(root).unwrap_or(path);
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(path)?);
        digest.update([0]);
    }
    let content = format!("{:x}\n", digest.finalize());
    let stamp = root.join(directory).join(format!("{name}.sha256"));
    if fs::read_to_string(&stamp).ok().as_deref() != Some(&content) {
        if let Some(parent) = stamp.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic::write(&stamp, content.as_bytes())?;
    }
    Ok(stamp)
}
