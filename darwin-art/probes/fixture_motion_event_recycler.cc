#include "fixture_motion_event_recycler.h"

namespace darwin_art_graphics_fixture {

namespace {

void DeleteLocal(JNIEnv* env, jobject object) {
  if (env != nullptr && object != nullptr) env->DeleteLocalRef(object);
}

// Calls recycle while preserving the exception that caused entry into this
// cleanup. If there was no original exception, a recycle exception remains
// pending for the caller.
bool RecyclePreserving(JNIEnv* env, jobject event, jmethodID recycle) {
  if (env == nullptr || event == nullptr || recycle == nullptr) return false;
  const bool had_original = env->ExceptionCheck() == JNI_TRUE;
  jthrowable original = had_original ? env->ExceptionOccurred() : nullptr;
  if (had_original && original == nullptr) {
    // Do not clear an exception that JNI could not materialize for us; there
    // is no safe way to preserve or replace it.
    return false;
  }
  if (had_original) env->ExceptionClear();
  env->CallVoidMethod(event, recycle);
  const bool had_cleanup = env->ExceptionCheck() == JNI_TRUE;
  jthrowable cleanup = had_cleanup ? env->ExceptionOccurred() : nullptr;
  if (had_cleanup && cleanup == nullptr) {
    if (original != nullptr) {
      env->ExceptionClear();
      env->Throw(original);
      DeleteLocal(env, original);
    }
    return false;
  }
  if (had_cleanup) env->ExceptionClear();
  if (had_original) {
    if (cleanup != nullptr) DeleteLocal(env, cleanup);
    if (original != nullptr) {
      env->Throw(original);
      DeleteLocal(env, original);
    }
    return false;
  }
  if (cleanup != nullptr) {
    env->Throw(cleanup);
    DeleteLocal(env, cleanup);
    return false;
  }
  return true;
}

}  // namespace

FixtureMotionEventRecycler::FixtureMotionEventRecycler(JNIEnv* env, jobject event,
                                                       jmethodID recycle)
    : env_(env), recycle_(recycle) {
  if (env_ == nullptr || event == nullptr || recycle_ == nullptr) return;

  // Do not call NewLocalRef while an exception is pending. The borrowed event
  // is still a live argument, so recycle it directly while preserving that
  // original exception before returning an invalid scope.
  if (env_->ExceptionCheck()) {
    (void)RecycleBorrowed(env_, event, recycle_);
    return;
  }
  event_ = env_->NewLocalRef(event);
  if (event_ == nullptr || env_->ExceptionCheck()) {
    jobject cleanup_event = event_ != nullptr ? event_ : event;
    (void)RecycleBorrowed(env_, cleanup_event, recycle_);
    DeleteLocal(env_, event_);
    event_ = nullptr;
    return;
  }
  valid_ = true;
}

FixtureMotionEventRecycler::~FixtureMotionEventRecycler() {
  if (valid_ && !recycled_) (void)Recycle();
  DeleteLocal(env_, event_);
  event_ = nullptr;
}

bool FixtureMotionEventRecycler::RecycleBorrowed(JNIEnv* env, jobject event,
                                                 jmethodID recycle) {
  return RecyclePreserving(env, event, recycle);
}

bool FixtureMotionEventRecycler::Recycle() {
  if (!valid_ || event_ == nullptr || recycle_ == nullptr) return false;
  if (recycled_) return env_ != nullptr && !env_->ExceptionCheck();
  recycled_ = true;
  return RecyclePreserving(env_, event_, recycle_);
}

}  // namespace darwin_art_graphics_fixture
