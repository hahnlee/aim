#include "system_context.h"

namespace darwin_art::framework::system {
jobject CreateSystemContext(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck() || env->PushLocalFrame(8) < 0)
    return nullptr;
  jclass owner = env->FindClass("android/app/ActivityThread");
  jmethodID main = owner == nullptr ? nullptr : env->GetStaticMethodID(
      owner, "systemMain", "()Landroid/app/ActivityThread;");
  jobject thread = main == nullptr ? nullptr : env->CallStaticObjectMethod(owner, main);
  jobject context = nullptr;
  if (thread != nullptr && !env->ExceptionCheck()) {
    jmethodID get = env->GetMethodID(owner, "getSystemContext",
                                    "()Landroid/app/ContextImpl;");
    if (get != nullptr) context = env->CallObjectMethod(thread, get);
  }
  // No field fabrication, null-context substitute, retry or exception clearing.
  return env->PopLocalFrame(env->ExceptionCheck() ? nullptr : context);
}
}
