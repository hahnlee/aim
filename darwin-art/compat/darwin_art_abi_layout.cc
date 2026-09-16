#include <cstddef>
#include <cstdint>
#include <type_traits>

#include "darwin_art/darwin_art.h"
#include "darwin_surface_bridge.h"

// Keep the C ABI's layout contract executable on the native side.  The Rust
// mirror is checked independently in darwin-art-engine-sys; this translation
// unit makes a drift in either declaration fail during the native build rather
// than becoming an opaque runtime failure.
static_assert(std::is_standard_layout_v<darwin_art_process_config_t>);
static_assert(std::is_standard_layout_v<darwin_art_lifecycle_hooks_t>);
static_assert(sizeof(darwin_art_lifecycle_hooks_t) == 48);
static_assert(alignof(darwin_art_lifecycle_hooks_t) == 8);
static_assert(offsetof(darwin_art_lifecycle_hooks_t, context) == 8);
static_assert(offsetof(darwin_art_lifecycle_hooks_t, begin_run) == 16);
static_assert(offsetof(darwin_art_lifecycle_hooks_t, finish_run) == 24);
static_assert(offsetof(darwin_art_lifecycle_hooks_t, begin_shutdown) == 32);
static_assert(offsetof(darwin_art_lifecycle_hooks_t, mark_failed) == 40);
static_assert(std::is_standard_layout_v<darwin_art_service_spawn_request_t>);
static_assert(sizeof(darwin_art_service_spawn_request_t) == 40);
static_assert(offsetof(darwin_art_service_spawn_request_t, component) == 8);
static_assert(offsetof(darwin_art_service_spawn_request_t, instance_name) == 16);
static_assert(offsetof(darwin_art_service_spawn_request_t, process_name) == 24);
static_assert(offsetof(darwin_art_service_spawn_request_t, isolated) == 32);
static_assert(sizeof(darwin_art_service_spawn_result_t) == 16);
static_assert(offsetof(darwin_art_service_spawn_result_t, host_pid) == 8);
static_assert(offsetof(darwin_art_service_spawn_result_t, control_fd) == 12);
static_assert(sizeof(darwin_art_host_services_t) == 32);
static_assert(offsetof(darwin_art_host_services_t, context) == 8);
static_assert(offsetof(darwin_art_host_services_t, spawn_service) == 16);
static_assert(offsetof(darwin_art_host_services_t, release_service) == 24);
static_assert(sizeof(darwin_art_native_loader_config_t) == 40);
static_assert(alignof(darwin_art_native_loader_config_t) == 8);
static_assert(offsetof(darwin_art_native_loader_config_t, linker_config_path) == 8);
static_assert(offsetof(darwin_art_native_loader_config_t, executable_path) == 16);
static_assert(offsetof(darwin_art_native_loader_config_t, library_search_path) == 24);
static_assert(offsetof(darwin_art_native_loader_config_t, android_unwind_path) == 32);
static_assert(sizeof(darwin_art_process_config_t) == 136);
static_assert(alignof(darwin_art_process_config_t) == 8);
static_assert(offsetof(darwin_art_process_config_t, struct_size) == 0);
static_assert(offsetof(darwin_art_process_config_t, abi_version) == 4);
static_assert(offsetof(darwin_art_process_config_t, core_oj_jar) == 8);
static_assert(offsetof(darwin_art_process_config_t, core_libart_jar) == 16);
static_assert(offsetof(darwin_art_process_config_t, framework_jar) == 24);
static_assert(offsetof(darwin_art_process_config_t, core_icu4j_jar) == 32);
static_assert(offsetof(darwin_art_process_config_t, app_dex) == 40);
static_assert(offsetof(darwin_art_process_config_t, heap_initial_bytes) == 48);
static_assert(offsetof(darwin_art_process_config_t, heap_maximum_bytes) == 56);
static_assert(offsetof(darwin_art_process_config_t, host_context) == 64);
static_assert(offsetof(darwin_art_process_config_t, frame_callback) == 72);
static_assert(offsetof(darwin_art_process_config_t, provider_context) == 80);
static_assert(offsetof(darwin_art_process_config_t, provider_acquire) == 88);
static_assert(offsetof(darwin_art_process_config_t, provider_release) == 96);
static_assert(offsetof(darwin_art_process_config_t, graphics_session_context) == 104);
static_assert(offsetof(darwin_art_process_config_t, lifecycle_hooks) == 112);
static_assert(offsetof(darwin_art_process_config_t, host_services) == 120);
static_assert(offsetof(darwin_art_process_config_t, native_loader_config) == 128);

static_assert(std::is_standard_layout_v<darwin_art_process_result_t>);
static_assert(sizeof(darwin_art_process_result_t) == 36);
static_assert(alignof(darwin_art_process_result_t) == 4);
static_assert(offsetof(darwin_art_process_result_t, struct_size) == 0);
static_assert(offsetof(darwin_art_process_result_t, abi_version) == 4);
static_assert(offsetof(darwin_art_process_result_t, hello_answer) == 8);
static_assert(offsetof(darwin_art_process_result_t, native_round_trip) == 12);
static_assert(offsetof(darwin_art_process_result_t, arraycopy_result) == 16);
static_assert(offsetof(darwin_art_process_result_t, activity_probe_result) == 20);
static_assert(offsetof(darwin_art_process_result_t, lifecycle_result) == 24);
static_assert(offsetof(darwin_art_process_result_t, frame_width) == 28);
static_assert(offsetof(darwin_art_process_result_t, frame_height) == 32);

static_assert(std::is_standard_layout_v<DarwinArtSurfaceCreateInfo>);
static_assert(sizeof(DarwinArtSurfaceCreateInfo) == 24);
static_assert(alignof(DarwinArtSurfaceCreateInfo) == 8);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, width) == 0);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, height) == 4);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, title) == 8);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, visible) == 16);
static_assert(offsetof(DarwinArtSurfaceCreateInfo, scale_to_display) == 17);

extern "C" int darwin_art_abi_layout_anchor() {
  return 0;
}
