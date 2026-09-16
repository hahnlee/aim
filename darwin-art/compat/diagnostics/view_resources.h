#pragma once
#include <jni.h>
#include <cstdio>
#include <cstdlib>

namespace darwin_art::diagnostics {
// Read-only, opt-in observation of the application's actual resource owner.
inline void LogViewRootResources(JNIEnv* env, jobject root) {
  if (std::getenv("DARWIN_ART_DEBUG_DISPLAY_CONFIGURATION") == nullptr ||
      root == nullptr || env->ExceptionCheck() || env->PushLocalFrame(24) < 0) return;
  auto inspect = [&] {
    jclass type = env->GetObjectClass(root);
    jfieldID context_field = env->GetFieldID(type, "mContext", "Landroid/content/Context;");
    if (context_field == nullptr) return;
    jobject context = env->GetObjectField(root, context_field);
    if (context == nullptr) return;
    jclass context_type = env->GetObjectClass(context);
    jmethodID get_resources = env->GetMethodID(context_type, "getResources",
        "()Landroid/content/res/Resources;");
    if (get_resources == nullptr) return;
    jobject resources = env->CallObjectMethod(context, get_resources);
    if (resources == nullptr || env->ExceptionCheck()) return;
    jclass resource_type = env->GetObjectClass(resources);
    const char* names[] = {"getConfiguration", "getDisplayMetrics"};
    const char* signatures[] = {"()Landroid/content/res/Configuration;",
                                "()Landroid/util/DisplayMetrics;"};
    for (int i = 0; i < 2; ++i) {
      jmethodID method = env->GetMethodID(resource_type, names[i], signatures[i]);
      if (method == nullptr) return;
      jobject value = env->CallObjectMethod(resources, method);
      if (value == nullptr || env->ExceptionCheck()) return;
      jclass value_type = env->GetObjectClass(value);
      jmethodID stringify = env->GetMethodID(value_type, "toString", "()Ljava/lang/String;");
      if (stringify == nullptr) return;
      auto text = static_cast<jstring>(env->CallObjectMethod(value, stringify));
      if (text == nullptr || env->ExceptionCheck()) return;
      const char* utf = env->GetStringUTFChars(text, nullptr);
      if (utf == nullptr) return;
      std::fprintf(stderr, "ART app ViewRoot resources %s=%s\n", names[i], utf);
      env->ReleaseStringUTFChars(text, utf);
    }
  };
  inspect();
  // Diagnostics must report their own lookup failure without changing input.
  if (env->ExceptionCheck()) {
    env->ExceptionDescribe();
    env->ExceptionClear();
  }
  env->PopLocalFrame(nullptr);
}
}
