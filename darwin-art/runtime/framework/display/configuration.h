#pragma once

#include <jni.h>

namespace darwin_art::framework::display {

// AMS binding and WMS launch must advertise the same display qualifiers as
// relayout, before the application inflates any resource-dependent Views.
inline jobject ConfigurationForBuiltInDisplay(JNIEnv* env, jobject base) {
  if (base == nullptr || env->ExceptionCheck()) return nullptr;
  jclass owner = env->FindClass(
      "dev/darwinart/runtime/display/BuiltInDisplayConfiguration");
  if (owner == nullptr) return nullptr;
  jmethodID configure = env->GetStaticMethodID(
      owner, "configuration",
      "(Landroid/content/res/Configuration;)Landroid/content/res/Configuration;");
  jobject result = configure == nullptr ? nullptr
      : env->CallStaticObjectMethod(owner, configure, base);
  env->DeleteLocalRef(owner);
  return result;
}

}  // namespace darwin_art::framework::display
