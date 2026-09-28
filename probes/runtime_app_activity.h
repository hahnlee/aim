#pragma once

#include <jni.h>

#include "runtime_app_resources.h"

namespace art {
class Thread;
}

namespace aim_app_activity {

struct Bundle {
  jclass activity_class = nullptr;
  jclass context_theme_wrapper_class = nullptr;
  jclass window_class = nullptr;
  jclass phone_window_class = nullptr;
  jobject probe_context = nullptr;
  jobject application = nullptr;
  jobject window = nullptr;
  jobject probe_theme = nullptr;
};

int prepare(JNIEnv* env, art::Thread* self, jobject* activity_instance,
            jclass probe_activity_class, jclass probe_context_class,
            const aim_app_resources::Bundle* resources,
            jobject package_manager, bool run_apk_app,
            bool use_framework_resources, const char* apk_app_package,
            const char* apk_app_activity, bool application_only, Bundle* out);

void release(JNIEnv* env, Bundle* bundle);

}  // namespace aim_app_activity
