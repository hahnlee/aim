#pragma once

#include <jni.h>

namespace aim_app_resources {

// Local JNI references created while constructing the detached Android
// resource graph. The caller owns the bundle for the duration of the
// Activity/Window transaction and must release it before its local frame is
// popped.
struct Bundle {
  jclass activity_info_class = nullptr;
  jclass application_class = nullptr;
  jclass asset_manager_class = nullptr;
  jclass apk_assets_class = nullptr;
  jobject activity_info = nullptr;
  jobject application = nullptr;
  jobject asset_manager = nullptr;
  jobject framework_apk_assets = nullptr;
  jobject app_apk_assets = nullptr;
  jstring framework_res_path = nullptr;
  jstring app_apk_path = nullptr;
  jobjectArray configured_apk_assets = nullptr;
  jobject probe_resources = nullptr;
};

int prepare(JNIEnv* env, jclass probe_resources_class,
            bool use_framework_resources, jint window_scale,
            const char* framework_res_apk, const char* app_apk_path,
            Bundle* out);

void release(JNIEnv* env, Bundle* bundle);

}  // namespace aim_app_resources
