#pragma once
#include <jni.h>

namespace darwin_art::input {

// Owns android.view.VelocityTracker allocation and motion-history state.
bool RegisterVelocityTrackerNatives(JNIEnv* env);

}  // namespace darwin_art::input

