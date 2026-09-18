#pragma once

#include <jni.h>

#include "darwin_framework_input_hint.h"

namespace darwin_art {

bool InitializeFrameworkGraphicsRuntime();
void ShutdownFrameworkGraphicsRuntime();
// Stop HWUI asynchronous workers while ART/JNI are still alive, before VM
// detach/destruction. This is separate from ICU teardown below.
void ShutdownFrameworkAsyncWorkers();
bool RegisterFrameworkNatives(JNIEnv* env);
bool RegisterMotionEventNatives(JNIEnv* env);
bool RegisterFrameworkBinderNatives(JNIEnv* env);
bool RegisterFrameworkSystemPropertyNatives(JNIEnv* env);
bool RegisterFrameworkAnimationNatives(JNIEnv* env);
bool RegisterFrameworkSqliteNatives(JNIEnv* env);
bool RegisterFrameworkSupportNatives(JNIEnv* env);
bool RegisterFrameworkResourceNatives(JNIEnv* env);
bool RegisterFrameworkGraphicsNatives(JNIEnv* env);

// SurfaceFlinger's display edge is supplied by the host frame clock. Pending
// DisplayEventReceivers are delivered on the Android owner/Looper thread;
// their normal asynchronous Handler path then invokes Choreographer.doFrame.
int DispatchFrameworkPendingVsyncs(JNIEnv* env, jlong frame_time_nanos);


}  // namespace darwin_art
