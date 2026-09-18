#pragma once

#include <jni.h>

namespace darwin_art_graphics_fixture {

// TEST ONLY: keeps an original MotionEvent alive for the duration of a
// fixture dispatch and gives ownership of exactly one recycle call to this
// scope. The caller supplies the already-resolved recycle method; this class
// performs no JNI lookups and never owns the Java event beyond its local ref.
class FixtureMotionEventRecycler final {
 public:
  FixtureMotionEventRecycler(JNIEnv* env, jobject event, jmethodID recycle);
  ~FixtureMotionEventRecycler();

  FixtureMotionEventRecycler(const FixtureMotionEventRecycler&) = delete;
  FixtureMotionEventRecycler& operator=(const FixtureMotionEventRecycler&) = delete;
  FixtureMotionEventRecycler(FixtureMotionEventRecycler&&) = delete;
  FixtureMotionEventRecycler& operator=(FixtureMotionEventRecycler&&) = delete;

  bool valid() const { return valid_; }

  // Recycles exactly once. Returns false when the original or cleanup call
  // leaves a JNI exception pending. A repeated call is an idempotent success
  // without another Java invocation.
  bool Recycle();

 private:
  static bool RecycleBorrowed(JNIEnv* env, jobject event, jmethodID recycle);

  JNIEnv* env_ = nullptr;
  jobject event_ = nullptr;  // scoped NewLocalRef of the original event
  jmethodID recycle_ = nullptr;
  bool valid_ = false;
  bool recycled_ = false;
};

}  // namespace darwin_art_graphics_fixture
