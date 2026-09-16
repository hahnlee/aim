#pragma once
#include <jni.h>

namespace darwin_art::window {
// Mirrors Surface's mLock ownership while the caller acquires its native
// window reference. Reading an identity then unlocking before retain races
// Surface.release(). Never clears a pending Java exception.
class LockedSurface {
 public:
  LockedSurface(JNIEnv* env, jobject surface);
  ~LockedSurface();
  LockedSurface(const LockedSurface&) = delete;
  LockedSurface& operator=(const LockedSurface&) = delete;
  jlong identity() const { return identity_; }
 private:
  JNIEnv* env_;
  jobject lock_ = nullptr;
  bool entered_ = false;
  jlong identity_ = 0;
};
}
