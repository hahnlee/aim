#include "runtime/embedding/process_config.h"

#include <cassert>
#include <cstdlib>
#include <iostream>
#include <string>

namespace {

constexpr const char* kNames[] = {
    "AIM_SYSTEM_SERVER_MODE",
    "AIM_APK_APP_PACKAGE",
    "AIM_APK_APP_ACTIVITY",
    "AIM_APK_APP_DESCRIPTOR",
    "AIM_APK_APP_SUPPORT_DEX",
    "AIM_APK_APP_RESOURCE_APK",
    "AIM_FRAMEWORK_RES_APK",
    "AIM_ANDROID_FILESYSTEM_ROOT",
    "AIM_ANDROID_SYSTEM_ROOT",
    "AIM_ANDROID_SYSTEM_NATIVE_DIR",
    "AIM_WINDOW_SCALE",
};

void Clear() {
  for (const char* name : kNames) assert(unsetenv(name) == 0);
}

void Set(const char* name, const char* value) {
  assert(setenv(name, value, 1) == 0);
}

int Load(aim::embedding::ProcessConfigOptions* options) {
  std::string error;
  return aim::embedding::LoadProcessConfig(options, &error);
}

void SystemBase() {
  Set("AIM_SYSTEM_SERVER_MODE", "1");
  Set("AIM_APK_APP_PACKAGE", "android");
  Set("AIM_APK_APP_SUPPORT_DEX", "/image/system/framework/services.dex");
  Set("AIM_APK_APP_RESOURCE_APK",
      "/image/system/framework/framework-res.apk");
  Set("AIM_FRAMEWORK_RES_APK",
      "/image/system/framework/framework-res.apk");
  Set("AIM_ANDROID_FILESYSTEM_ROOT", "/image");
  Set("AIM_ANDROID_SYSTEM_ROOT", "/image/system");
  Set("AIM_ANDROID_SYSTEM_NATIVE_DIR", "/image/system/lib64");
}

void AssertSystemSuccess() {
  aim::embedding::ProcessConfigOptions options;
  assert(Load(&options) == 0);
  assert(options.system_server_mode);
  assert(!options.run_apk_app);
  assert(options.use_framework_resources);
  assert(options.apk_app_package == "android");
  assert(options.android_filesystem_root == "/image");
}

void AssertSystemFailure() {
  aim::embedding::ProcessConfigOptions options;
  assert(Load(&options) == 48);
}

}  // namespace

int main() {
  Clear();
  SystemBase();
  AssertSystemSuccess();

  // Activity/descriptor are capabilities, not optional empty metadata.  A
  // leaked variable must reject a system process even when its value is empty.
  for (const char* name : {"AIM_APK_APP_ACTIVITY",
                           "AIM_APK_APP_DESCRIPTOR"}) {
    Set(name, "");
    AssertSystemFailure();
    Clear();
    SystemBase();
  }

  for (const char* mode : {"", "0", "01", "2"}) {
    Set("AIM_SYSTEM_SERVER_MODE", mode);
    AssertSystemFailure();
    Clear();
    SystemBase();
  }

  for (const char* name : {"AIM_APK_APP_RESOURCE_APK",
                           "AIM_FRAMEWORK_RES_APK",
                           "AIM_ANDROID_FILESYSTEM_ROOT",
                           "AIM_ANDROID_SYSTEM_ROOT",
                           "AIM_ANDROID_SYSTEM_NATIVE_DIR"}) {
    unsetenv(name);
    AssertSystemFailure();
    SystemBase();
  }

  for (const char* name : {"AIM_APK_APP_SUPPORT_DEX",
                           "AIM_APK_APP_RESOURCE_APK",
                           "AIM_FRAMEWORK_RES_APK",
                           "AIM_ANDROID_FILESYSTEM_ROOT"}) {
    const std::string env_name(name);
    const char* original = "/image";
    if (env_name == "AIM_APK_APP_SUPPORT_DEX")
      original = "/image/system/framework/services.dex";
    else if (env_name == "AIM_APK_APP_RESOURCE_APK" ||
             env_name == "AIM_FRAMEWORK_RES_APK")
      original = "/image/system/framework/framework-res.apk";
    for (const char* malformed : {"", "relative", "/image/../escape",
                                  "/image/./child", "/image/"}) {
      Set(name, malformed);
      AssertSystemFailure();
      Set(name, original);
    }
  }

  for (const char* name : {"AIM_ANDROID_SYSTEM_ROOT",
                           "AIM_ANDROID_SYSTEM_NATIVE_DIR"}) {
    const char* original = std::string(name) == "AIM_ANDROID_SYSTEM_ROOT"
                               ? "/image/system"
                               : "/image/system/lib64";
    for (const char* malformed : {"", "relative", "/image/../escape",
                                  "/image/system/"}) {
      Set(name, malformed);
      AssertSystemFailure();
      Set(name, original);
    }
  }

  Set("AIM_APK_APP_RESOURCE_APK", "/image/system/framework/app.apk");
  AssertSystemFailure();
  Set("AIM_APK_APP_RESOURCE_APK", "/image/system/framework/framework-res.apk");
  Set("AIM_ANDROID_SYSTEM_ROOT", "/different/system");
  AssertSystemFailure();
  Set("AIM_ANDROID_SYSTEM_ROOT", "/image/system");

  // Normal APK validation retains its existing malformed-identity behavior.
  Clear();
  Set("AIM_APK_APP_PACKAGE", "com.example.app");
  Set("AIM_APK_APP_ACTIVITY", "MainActivity");
  Set("AIM_APK_APP_DESCRIPTOR", "not-a-descriptor");
  Set("AIM_APK_APP_SUPPORT_DEX", "/app/support.dex");
  Set("AIM_APK_APP_RESOURCE_APK", "/app/app.apk");
  Set("AIM_FRAMEWORK_RES_APK", "/image/system/framework/framework-res.apk");
  AssertSystemFailure();
  Clear();

  std::cout << "process config: validated system mode isolation and app rejection PASS\n";
}
