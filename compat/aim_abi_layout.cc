#include <cstddef>
#include <cstdint>
#include <type_traits>

#include "aim/aim.h"
#include "darwin_surface_bridge.h"

// Keep the C ABI's layout contract executable on the native side.  The Rust
// mirror is checked independently in aim-engine-sys; this translation
// unit makes a drift in either declaration fail during the native build rather
// than becoming an opaque runtime failure.
static_assert(std::is_standard_layout_v<aim_process_config_t>);
static_assert(std::is_standard_layout_v<aim_binder_authority_hooks_t>);
static_assert(sizeof(aim_binder_authority_hooks_t) == 40);
static_assert(alignof(aim_binder_authority_hooks_t) == 8);
static_assert(offsetof(aim_binder_authority_hooks_t, struct_size) == 0);
static_assert(offsetof(aim_binder_authority_hooks_t, abi_version) == 4);
static_assert(offsetof(aim_binder_authority_hooks_t, context) == 8);
static_assert(offsetof(aim_binder_authority_hooks_t, retain) == 16);
static_assert(offsetof(aim_binder_authority_hooks_t, live) == 24);
static_assert(offsetof(aim_binder_authority_hooks_t, release) == 32);
static_assert(std::is_standard_layout_v<aim_lifecycle_hooks_t>);
static_assert(sizeof(aim_lifecycle_hooks_t) == 48);
static_assert(alignof(aim_lifecycle_hooks_t) == 8);
static_assert(offsetof(aim_lifecycle_hooks_t, context) == 8);
static_assert(offsetof(aim_lifecycle_hooks_t, begin_run) == 16);
static_assert(offsetof(aim_lifecycle_hooks_t, finish_run) == 24);
static_assert(offsetof(aim_lifecycle_hooks_t, begin_shutdown) == 32);
static_assert(offsetof(aim_lifecycle_hooks_t, mark_failed) == 40);
static_assert(std::is_standard_layout_v<aim_service_spawn_request_t>);
static_assert(sizeof(aim_service_spawn_request_t) == 40);
static_assert(offsetof(aim_service_spawn_request_t, component) == 8);
static_assert(offsetof(aim_service_spawn_request_t, instance_name) == 16);
static_assert(offsetof(aim_service_spawn_request_t, process_name) == 24);
static_assert(offsetof(aim_service_spawn_request_t, isolated) == 32);
static_assert(sizeof(aim_service_spawn_result_t) == 16);
static_assert(offsetof(aim_service_spawn_result_t, host_pid) == 8);
static_assert(offsetof(aim_service_spawn_result_t, control_fd) == 12);
static_assert(sizeof(aim_host_services_t) == 32);
static_assert(offsetof(aim_host_services_t, context) == 8);
static_assert(offsetof(aim_host_services_t, spawn_service) == 16);
static_assert(offsetof(aim_host_services_t, release_service) == 24);
static_assert(sizeof(aim_native_loader_config_t) == 40);
static_assert(alignof(aim_native_loader_config_t) == 8);
static_assert(offsetof(aim_native_loader_config_t, linker_config_path) == 8);
static_assert(offsetof(aim_native_loader_config_t, executable_path) == 16);
static_assert(offsetof(aim_native_loader_config_t, library_search_path) == 24);
static_assert(offsetof(aim_native_loader_config_t, android_unwind_path) == 32);
static_assert(sizeof(aim_process_config_t) == 152);
static_assert(offsetof(aim_process_config_t, desktop_surface_context) == 136);
static_assert(offsetof(aim_process_config_t, binder_authority_hooks) == 144);
static_assert(alignof(aim_process_config_t) == 8);
static_assert(offsetof(aim_process_config_t, struct_size) == 0);
static_assert(offsetof(aim_process_config_t, abi_version) == 4);
static_assert(offsetof(aim_process_config_t, core_oj_jar) == 8);
static_assert(offsetof(aim_process_config_t, core_libart_jar) == 16);
static_assert(offsetof(aim_process_config_t, framework_jar) == 24);
static_assert(offsetof(aim_process_config_t, core_icu4j_jar) == 32);
static_assert(offsetof(aim_process_config_t, app_dex) == 40);
static_assert(offsetof(aim_process_config_t, heap_initial_bytes) == 48);
static_assert(offsetof(aim_process_config_t, heap_maximum_bytes) == 56);
static_assert(offsetof(aim_process_config_t, host_context) == 64);
static_assert(offsetof(aim_process_config_t, frame_callback) == 72);
static_assert(offsetof(aim_process_config_t, provider_context) == 80);
static_assert(offsetof(aim_process_config_t, provider_acquire) == 88);
static_assert(offsetof(aim_process_config_t, provider_release) == 96);
static_assert(offsetof(aim_process_config_t, graphics_session_context) == 104);
static_assert(offsetof(aim_process_config_t, lifecycle_hooks) == 112);
static_assert(offsetof(aim_process_config_t, host_services) == 120);
static_assert(offsetof(aim_process_config_t, native_loader_config) == 128);

static_assert(std::is_standard_layout_v<aim_process_result_t>);
static_assert(sizeof(aim_process_result_t) == 36);
static_assert(alignof(aim_process_result_t) == 4);
static_assert(offsetof(aim_process_result_t, struct_size) == 0);
static_assert(offsetof(aim_process_result_t, abi_version) == 4);
static_assert(offsetof(aim_process_result_t, hello_answer) == 8);
static_assert(offsetof(aim_process_result_t, native_round_trip) == 12);
static_assert(offsetof(aim_process_result_t, arraycopy_result) == 16);
static_assert(offsetof(aim_process_result_t, activity_probe_result) == 20);
static_assert(offsetof(aim_process_result_t, lifecycle_result) == 24);
static_assert(offsetof(aim_process_result_t, frame_width) == 28);
static_assert(offsetof(aim_process_result_t, frame_height) == 32);

static_assert(std::is_standard_layout_v<AimSurfaceCreateInfo>);
static_assert(sizeof(AimSurfaceCreateInfo) == 24);
static_assert(alignof(AimSurfaceCreateInfo) == 8);
static_assert(offsetof(AimSurfaceCreateInfo, width) == 0);
static_assert(offsetof(AimSurfaceCreateInfo, height) == 4);
static_assert(offsetof(AimSurfaceCreateInfo, title) == 8);
static_assert(offsetof(AimSurfaceCreateInfo, visible) == 16);
static_assert(offsetof(AimSurfaceCreateInfo, scale_to_display) == 17);
static_assert(std::is_standard_layout_v<AimSurfaceInputSink>);
static_assert(sizeof(AimSurfaceInputSink) == 48);
static_assert(alignof(AimSurfaceInputSink) == 8);
static_assert(offsetof(AimSurfaceInputSink, version) == 0);
static_assert(offsetof(AimSurfaceInputSink, size) == 4);
static_assert(offsetof(AimSurfaceInputSink, context) == 8);
static_assert(offsetof(AimSurfaceInputSink, retain_context) == 16);
static_assert(offsetof(AimSurfaceInputSink, release_context) == 24);
static_assert(offsetof(AimSurfaceInputSink, pointer) == 32);
static_assert(offsetof(AimSurfaceInputSink, key) == 40);

extern "C" int aim_abi_layout_anchor() {
  return 0;
}
