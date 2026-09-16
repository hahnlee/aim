#pragma once

#include <jni.h>

namespace art {
class Thread;
}

namespace darwin_art::framework::app {

// Starts the Android-owned application process. ActivityThread, LoadedApk,
// ContextImpl, and framework transactions own application setup and dispatch.
// No Probe classes, custom app ClassLoader, or manual Activity callbacks are
// involved in this entry.
int RunApplicationProcess(JNIEnv* env, art::Thread* self);

}  // namespace darwin_art::framework::app
