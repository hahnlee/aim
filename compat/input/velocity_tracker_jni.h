#pragma once
#include <jni.h>

namespace aim::input {

// Owns android.view.VelocityTracker allocation and motion-history state.
bool RegisterVelocityTrackerNatives(JNIEnv* env);

}  // namespace aim::input

