#pragma once

#include <jni.h>

namespace darwin_art::framework::app {

// The host supplies an already-prepared ART main Looper. ActivityThread owns
// publication, ConfigurationController, RuntimeInit and AMS attachment, just
// as it does after construction in ActivityThread.main(). Preserve exceptions:
// a missing platform service must not silently select a detached lifecycle.
inline bool AttachApplicationProcess(JNIEnv* env, jobject thread) {
  if (env == nullptr || thread == nullptr || env->ExceptionCheck()) return false;
  jclass type = env->FindClass("android/app/ActivityThread");
  if (type == nullptr) return false;
  jmethodID attach = env->GetMethodID(type, "attach", "(ZJ)V");
  if (attach != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(thread, attach, JNI_FALSE, static_cast<jlong>(0));
  }
  env->DeleteLocalRef(type);
  return attach != nullptr && !env->ExceptionCheck();
}

}  // namespace darwin_art::framework::app
