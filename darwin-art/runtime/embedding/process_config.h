#pragma once

#include <cstdint>
#include <string>

#include "darwin_art/darwin_art.h"

namespace darwin_art::embedding {

// Immutable product configuration assembled once at the C ABI boundary.
// Fixture-only switches intentionally live in probes/runtime_fixture_options.h.
struct ProcessConfigOptions final {
  std::string apk_app_package;
  std::string apk_app_activity;
  std::string apk_app_descriptor;
  std::string apk_app_support_dex;
  std::string apk_app_resource_apk;
  std::string apk_app_native_path;
  std::string framework_res_apk;
  std::string android_filesystem_root;
  std::string android_system_root;
  std::string android_system_native_dir;

  bool has_apk_app_identity_environment = false;
  bool has_framework_res_apk = false;
  bool has_window_scale = false;
  bool system_server_mode = false;
  bool run_apk_app = false;
  bool use_framework_resources = false;
  int32_t window_scale = 1;
};

struct ProcessConfigBounds final {
  uint64_t heap_initial_bytes = 0;
  uint64_t heap_maximum_bytes = 0;
};

// Validate the flat C ABI before ART allocates anything. Keeping this at the
// embedding boundary makes the orchestration TU consume checked values.
int ValidateProcessConfig(const darwin_art_process_config_t* config,
                          const darwin_art_process_result_t* run_result,
                          ProcessConfigBounds* bounds, std::string* error);

// Returns 0 on success and 48 for malformed APK/framework/window environment.
int LoadProcessConfig(ProcessConfigOptions* options, std::string* error);

}  // namespace darwin_art::embedding
