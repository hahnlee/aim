#include "runtime/embedding/process_config.h"

#include <cassert>
#include <cstdlib>
#include <iostream>
#include <string>

namespace {

constexpr const char* kNames[] = {
    "DARWIN_ART_SYSTEM_SERVER_MODE",
    "DARWIN_ART_APK_APP_PACKAGE",
    "DARWIN_ART_APK_APP_ACTIVITY",
    "DARWIN_ART_APK_APP_DESCRIPTOR",
    "DARWIN_ART_APK_APP_SUPPORT_DEX",
    "DARWIN_ART_APK_APP_RESOURCE_APK",
    "DARWIN_ART_FRAMEWORK_RES_APK",
    "DARWIN_ART_ANDROID_FILESYSTEM_ROOT",
    "DARWIN_ART_ANDROID_SYSTEM_ROOT",
    "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR",
    "DARWIN_ART_WINDOW_SCALE",
};

void Clear() {
  for (const char* name : kNames) assert(unsetenv(name) == 0);
}

void Set(const char* name, const char* value) {
  assert(setenv(name, value, 1) == 0);
}

int Load(darwin_art::embedding::ProcessConfigOptions* options) {
  std::string error;
  return darwin_art::embedding::LoadProcessConfig(options, &error);
}

void SystemBase() {
  Set("DARWIN_ART_SYSTEM_SERVER_MODE", "1");
  Set("DARWIN_ART_APK_APP_PACKAGE", "android");
  Set("DARWIN_ART_APK_APP_SUPPORT_DEX", "/image/system/framework/services.dex");
  Set("DARWIN_ART_APK_APP_RESOURCE_APK",
      "/image/system/framework/framework-res.apk");
  Set("DARWIN_ART_FRAMEWORK_RES_APK",
      "/image/system/framework/framework-res.apk");
  Set("DARWIN_ART_ANDROID_FILESYSTEM_ROOT", "/image");
  Set("DARWIN_ART_ANDROID_SYSTEM_ROOT", "/image/system");
  Set("DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR", "/image/system/lib64");
}

void AssertSystemSuccess() {
  darwin_art::embedding::ProcessConfigOptions options;
  assert(Load(&options) == 0);
  assert(options.system_server_mode);
  assert(!options.run_apk_app);
  assert(options.use_framework_resources);
  assert(options.apk_app_package == "android");
  assert(options.android_filesystem_root == "/image");
}

void AssertSystemFailure() {
  darwin_art::embedding::ProcessConfigOptions options;
  assert(Load(&options) == 48);
}

}  // namespace

int main() {
  Clear();
  SystemBase();
  AssertSystemSuccess();

  // Activity/descriptor are capabilities, not optional empty metadata.  A
  // leaked variable must reject a system process even when its value is empty.
  for (const char* name : {"DARWIN_ART_APK_APP_ACTIVITY",
                           "DARWIN_ART_APK_APP_DESCRIPTOR"}) {
    Set(name, "");
    AssertSystemFailure();
    Clear();
    SystemBase();
  }

  for (const char* mode : {"", "0", "01", "2"}) {
    Set("DARWIN_ART_SYSTEM_SERVER_MODE", mode);
    AssertSystemFailure();
    Clear();
    SystemBase();
  }

  for (const char* name : {"DARWIN_ART_APK_APP_RESOURCE_APK",
                           "DARWIN_ART_FRAMEWORK_RES_APK",
                           "DARWIN_ART_ANDROID_FILESYSTEM_ROOT",
                           "DARWIN_ART_ANDROID_SYSTEM_ROOT",
                           "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR"}) {
    unsetenv(name);
    AssertSystemFailure();
    SystemBase();
  }

  for (const char* name : {"DARWIN_ART_APK_APP_SUPPORT_DEX",
                           "DARWIN_ART_APK_APP_RESOURCE_APK",
                           "DARWIN_ART_FRAMEWORK_RES_APK",
                           "DARWIN_ART_ANDROID_FILESYSTEM_ROOT"}) {
    const std::string env_name(name);
    const char* original = "/image";
    if (env_name == "DARWIN_ART_APK_APP_SUPPORT_DEX")
      original = "/image/system/framework/services.dex";
    else if (env_name == "DARWIN_ART_APK_APP_RESOURCE_APK" ||
             env_name == "DARWIN_ART_FRAMEWORK_RES_APK")
      original = "/image/system/framework/framework-res.apk";
    for (const char* malformed : {"", "relative", "/image/../escape",
                                  "/image/./child", "/image/"}) {
      Set(name, malformed);
      AssertSystemFailure();
      Set(name, original);
    }
  }

  for (const char* name : {"DARWIN_ART_ANDROID_SYSTEM_ROOT",
                           "DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR"}) {
    const char* original = std::string(name) == "DARWIN_ART_ANDROID_SYSTEM_ROOT"
                               ? "/image/system"
                               : "/image/system/lib64";
    for (const char* malformed : {"", "relative", "/image/../escape",
                                  "/image/system/"}) {
      Set(name, malformed);
      AssertSystemFailure();
      Set(name, original);
    }
  }

  Set("DARWIN_ART_APK_APP_RESOURCE_APK", "/image/system/framework/app.apk");
  AssertSystemFailure();
  Set("DARWIN_ART_APK_APP_RESOURCE_APK", "/image/system/framework/framework-res.apk");
  Set("DARWIN_ART_ANDROID_SYSTEM_ROOT", "/different/system");
  AssertSystemFailure();
  Set("DARWIN_ART_ANDROID_SYSTEM_ROOT", "/image/system");

  // Normal APK validation retains its existing malformed-identity behavior.
  Clear();
  Set("DARWIN_ART_APK_APP_PACKAGE", "com.example.app");
  Set("DARWIN_ART_APK_APP_ACTIVITY", "MainActivity");
  Set("DARWIN_ART_APK_APP_DESCRIPTOR", "not-a-descriptor");
  Set("DARWIN_ART_APK_APP_SUPPORT_DEX", "/app/support.dex");
  Set("DARWIN_ART_APK_APP_RESOURCE_APK", "/app/app.apk");
  Set("DARWIN_ART_FRAMEWORK_RES_APK", "/image/system/framework/framework-res.apk");
  AssertSystemFailure();
  Clear();

  std::cout << "process config: validated system mode isolation and app rejection PASS\n";
}
