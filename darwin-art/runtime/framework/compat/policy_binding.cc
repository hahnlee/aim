#include "policy_binding.h"

namespace darwin_art::framework::compat {
namespace {
constexpr const char* kOwner = "dev/darwinart/runtime/compat/SystemCompatPolicy";
}
bool InitializeSystemPolicy(JNIEnv* env, jobject context) {
  if (env == nullptr || context == nullptr || env->ExceptionCheck()) return false;
  jclass owner = env->FindClass(kOwner);
  jmethodID initialize = owner == nullptr ? nullptr : env->GetStaticMethodID(
      owner, "initialize", "(Landroid/content/Context;)V");
  if (initialize != nullptr) env->CallStaticVoidMethod(owner, initialize, context);
  env->DeleteLocalRef(owner);
  return initialize != nullptr && !env->ExceptionCheck();
}

bool EvaluateApplicationChanges(JNIEnv* env, jobject application,
                                ApplicationChanges* result) {
  if (env == nullptr || application == nullptr || result == nullptr ||
      env->ExceptionCheck()) return false;
  *result = {};
  jclass owner = env->FindClass(kOwner);
  jmethodID disabled = owner == nullptr ? nullptr : env->GetStaticMethodID(
      owner, "disabledChanges", "(Landroid/content/pm/ApplicationInfo;)[J");
  if (disabled == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(owner);
    return false;
  }
  result->disabled = static_cast<jlongArray>(
      env->CallStaticObjectMethod(owner, disabled, application));
  if (result->disabled == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(owner);
    return false;
  }
  jmethodID loggable = env->GetStaticMethodID(
      owner, "loggableChanges", "(Landroid/content/pm/ApplicationInfo;)[J");
  if (loggable != nullptr) result->loggable = static_cast<jlongArray>(
      env->CallStaticObjectMethod(owner, loggable, application));
  env->DeleteLocalRef(owner);
  return result->loggable != nullptr && !env->ExceptionCheck();
}
}
