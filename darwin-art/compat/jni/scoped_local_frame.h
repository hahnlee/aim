#pragma once

#include <jni.h>
#include <unistd.h>

namespace darwin_art_jni_scope {

// Small JNI boundary helpers shared by process phases.  Keeping local-frame
// lifetime out of the orchestration TU makes every early-return path use the
// same RAII rule without coupling it to ART bootstrap state.
class ScopedLocalFrame final {
 public:
  explicit ScopedLocalFrame(JNIEnv* env)
      : env_(env), pushed_(env != nullptr && env->PushLocalFrame(256) == JNI_OK) {}

  ~ScopedLocalFrame() {
    if (pushed_) {
      env_->PopLocalFrame(nullptr);
    }
  }

  bool valid() const { return pushed_; }

 private:
  JNIEnv* env_;
  bool pushed_;
};

inline jint HostPageSize(JNIEnv*, jclass) { return getpagesize(); }

}  // namespace darwin_art_jni_scope
