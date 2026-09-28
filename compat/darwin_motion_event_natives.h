#pragma once

#include <jni.h>

namespace aim {

// Registers the Android 16 MotionEvent JNI surface used by obtain(),
// View.dispatchTouchEvent(), and recycle().  The implementation is a small
// C++ archive boundary so the Java object never owns a Rust pointer or a
// borrowed host allocation.
bool RegisterMotionEventNatives(JNIEnv* env);

}  // namespace aim
