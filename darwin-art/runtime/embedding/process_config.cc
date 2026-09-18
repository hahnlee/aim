#include "process_config.h"

#include <cstddef>
#include <cstdlib>

#include "../framework/system/class_path_contract.h"

namespace darwin_art::embedding {
namespace {

std::string Env(const char* name) {
  const char* value = std::getenv(name);
  return value == nullptr ? std::string() : std::string(value);
}

bool HasEnv(const char* name) { return std::getenv(name) != nullptr; }

bool Present(const std::string& value) { return !value.empty(); }

}  // namespace

int ValidateProcessConfig(const darwin_art_process_config_t* config,
                          const darwin_art_process_result_t* run_result,
                          ProcessConfigBounds* bounds, std::string* error) {
  constexpr uint32_t kLegacyConfigSize = static_cast<uint32_t>(
      offsetof(darwin_art_process_config_t, graphics_session_context));
  if (config == nullptr || run_result == nullptr || bounds == nullptr ||
      error == nullptr || config->struct_size < kLegacyConfigSize ||
      run_result->struct_size < sizeof(*run_result) ||
      config->abi_version != DARWIN_ART_ABI_VERSION ||
      run_result->abi_version != DARWIN_ART_ABI_VERSION ||
      config->core_oj_jar == nullptr || config->core_libart_jar == nullptr ||
      config->framework_jar == nullptr || config->core_icu4j_jar == nullptr ||
      config->app_dex == nullptr) {
    if (error != nullptr) *error = "invalid ABI/configuration";
    return 64;
  }
  if ((config->provider_context != nullptr || config->provider_acquire != nullptr ||
       config->provider_release != nullptr) &&
      (config->provider_context == nullptr || config->provider_acquire == nullptr ||
       config->provider_release == nullptr)) {
    *error = "incomplete provider hook table";
    return 64;
  }
  bounds->heap_initial_bytes = config->heap_initial_bytes == 0
                                   ? 64u * 1024u * 1024u
                                   : config->heap_initial_bytes;
  bounds->heap_maximum_bytes = config->heap_maximum_bytes == 0
                                   ? 64u * 1024u * 1024u
                                   : config->heap_maximum_bytes;
  if (bounds->heap_initial_bytes > bounds->heap_maximum_bytes ||
      bounds->heap_maximum_bytes > 256u * 1024u * 1024u) {
    *error = "invalid heap bounds";
    return 65;
  }
  return 0;
}

int LoadProcessConfig(ProcessConfigOptions* options, std::string* error) {
  if (options == nullptr || error == nullptr) return 48;
  *options = ProcessConfigOptions{};
  options->apk_app_package = Env("DARWIN_ART_APK_APP_PACKAGE");
  options->apk_app_activity = Env("DARWIN_ART_APK_APP_ACTIVITY");
  options->apk_app_descriptor = Env("DARWIN_ART_APK_APP_DESCRIPTOR");
  options->apk_app_support_dex = Env("DARWIN_ART_APK_APP_SUPPORT_DEX");
  options->apk_app_resource_apk = Env("DARWIN_ART_APK_APP_RESOURCE_APK");
  options->apk_app_native_path = Env("DARWIN_ART_APK_APP_NATIVE_PATH");
  options->framework_res_apk = Env("DARWIN_ART_FRAMEWORK_RES_APK");
  options->android_filesystem_root = Env("DARWIN_ART_ANDROID_FILESYSTEM_ROOT");
  options->android_system_root = Env("DARWIN_ART_ANDROID_SYSTEM_ROOT");
  options->android_system_native_dir =
      Env("DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR");
  const std::string window_scale = Env("DARWIN_ART_WINDOW_SCALE");
  options->has_framework_res_apk = HasEnv("DARWIN_ART_FRAMEWORK_RES_APK");
  options->has_window_scale = HasEnv("DARWIN_ART_WINDOW_SCALE");

  options->has_apk_app_identity_environment =
      HasEnv("DARWIN_ART_APK_APP_PACKAGE") ||
      HasEnv("DARWIN_ART_APK_APP_ACTIVITY") ||
      HasEnv("DARWIN_ART_APK_APP_DESCRIPTOR");
  const bool has_system_server_mode = HasEnv("DARWIN_ART_SYSTEM_SERVER_MODE");
  if (has_system_server_mode) {
    if (Env("DARWIN_ART_SYSTEM_SERVER_MODE") != "1") {
      *error = "ART Android system process mode is invalid";
      return 48;
    }
    options->system_server_mode = true;
  }
  options->run_apk_app =
      Present(options->apk_app_package) && Present(options->apk_app_activity) &&
      options->apk_app_descriptor.size() >= 3 &&
      options->apk_app_descriptor.size() <= 513 &&
      options->apk_app_descriptor.front() == 'L' &&
      options->apk_app_descriptor.back() == ';' &&
      Present(options->apk_app_support_dex) &&
      options->apk_app_resource_apk.starts_with('/') &&
      options->framework_res_apk.starts_with('/');
  options->use_framework_resources =
      options->has_framework_res_apk && options->framework_res_apk.starts_with('/');
  if (!options->has_window_scale || window_scale == "1") {
    options->window_scale = 1;
  } else if (window_scale == "2") {
    options->window_scale = 2;
  } else {
    *error = "ART Android APK app environment is incomplete or invalid";
    return 48;
  }

  if (options->system_server_mode) {
    // SystemServer is a shared Android process, not an APK process.  It still
    // receives the package identity and common immutable inputs needed by the
    // framework class loader, but never receives an Activity identity.
    if (options->apk_app_package != "android" ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->apk_app_support_dex.c_str()) ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->apk_app_resource_apk.c_str()) ||
        !options->has_framework_res_apk ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->framework_res_apk.c_str()) ||
        options->apk_app_resource_apk != options->framework_res_apk ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->android_filesystem_root.c_str()) ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->android_system_root.c_str()) ||
        !darwin_art::framework::system::IsValidSystemPath(
            options->android_system_native_dir.c_str()) ||
        options->android_system_root !=
            options->android_filesystem_root + "/system" ||
        options->android_system_native_dir !=
            options->android_system_root + "/lib64" ||
        HasEnv("DARWIN_ART_APK_APP_ACTIVITY") ||
        HasEnv("DARWIN_ART_APK_APP_DESCRIPTOR")) {
      *error = "ART Android system process environment is incomplete or invalid";
      return 48;
    }
    options->run_apk_app = false;
    options->use_framework_resources = true;
    return 0;
  }

  if ((options->has_apk_app_identity_environment && !options->run_apk_app) ||
      (options->has_framework_res_apk && !options->use_framework_resources)) {
    *error = "ART Android APK app environment is incomplete or invalid";
    return 48;
  }
  return 0;
}

}  // namespace darwin_art::embedding
