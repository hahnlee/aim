#pragma once

#include <jni.h>

namespace aim::window {

// Registers the Android hardware SyncFence JNI methods owned by this window
// subsystem. The implementation keeps the Android handle/refcount contract
// while delegating descriptor close and waits to the narrow Darwin providers.
bool RegisterSyncFenceNatives(JNIEnv* env);

}  // namespace aim::window
