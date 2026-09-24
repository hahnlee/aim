#pragma once

#include <jni.h>

namespace darwin_art::framework::display {

// bindApplication must advertise the attaching process's task display
// qualifiers (including its launch Activity's orientation) before the
// application inflates any resource-dependent Views. ActivityTask owns them.
inline jobject ConfigurationForApplicationTask(JNIEnv* env, jobject application,
                                               jobject base) {
  if (application == nullptr || base == nullptr || env->ExceptionCheck()) return nullptr;
  jclass interface_type = env->FindClass("android/os/IInterface");
  jmethodID as_binder = interface_type == nullptr
      ? nullptr
      : env->GetMethodID(interface_type, "asBinder", "()Landroid/os/IBinder;");
  jobject binder = as_binder == nullptr ? nullptr
      : env->CallObjectMethod(application, as_binder);
  env->DeleteLocalRef(interface_type);
  if (binder == nullptr || env->ExceptionCheck()) return nullptr;
  jclass owner = env->FindClass("dev/darwinart/runtime/wm/TaskGeometryController");
  if (owner == nullptr) return nullptr;
  jmethodID configure = env->GetStaticMethodID(
      owner, "processConfiguration",
      "(Landroid/os/IBinder;Landroid/content/res/Configuration;)"
      "Landroid/content/res/Configuration;");
  jobject result = configure == nullptr ? nullptr
      : env->CallStaticObjectMethod(owner, configure, binder, base);
  env->DeleteLocalRef(owner);
  env->DeleteLocalRef(binder);
  return result;
}

}  // namespace darwin_art::framework::display
