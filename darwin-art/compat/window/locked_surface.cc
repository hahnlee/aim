#include "locked_surface.h"

namespace darwin_art::window {
LockedSurface::LockedSurface(JNIEnv* env, jobject surface) : env_(env) {
  if (!env || !surface || env->ExceptionCheck()) return;
  jclass type = env->GetObjectClass(surface);
  if (!type) return;
  jfieldID lock_field = env->GetFieldID(type, "mLock", "Ljava/lang/Object;");
  jfieldID native_field = nullptr;
  if (lock_field && !env->ExceptionCheck())
    native_field = env->GetFieldID(type, "mNativeObject", "J");
  env->DeleteLocalRef(type);
  if (!native_field || env->ExceptionCheck()) return;
  lock_ = env->GetObjectField(surface, lock_field);
  if (!lock_ || env->ExceptionCheck()) return;
  if (env->MonitorEnter(lock_) != JNI_OK) return;
  entered_ = true;
  const jlong identity = env->GetLongField(surface, native_field);
  if (!env->ExceptionCheck()) identity_ = identity;
}
LockedSurface::~LockedSurface() {
  if (entered_) env_->MonitorExit(lock_);
  if (lock_) env_->DeleteLocalRef(lock_);
}
}
