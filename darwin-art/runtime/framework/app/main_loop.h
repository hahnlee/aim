#pragma once
#include <jni.h>

namespace darwin_art::framework::app {
enum class ApplicationMainLoopExit {
  kInvalidEnvironment,
  kRuntimeInit,
  kLooperLookup,
  kWrongLooper,
  kActivityThreadLookup,
  kAlreadyAttached,
  kActivityThreadConstruction,
  kActivityManagerAttachment,
  kLooperDispatch,
};

// Entry after native registration, before the Android main Looper exists. Runs
// the original ActivityThread.main() process entry; no probe contexts/activities.
// Does not return successfully: process shutdown or a Java failure ends it.
ApplicationMainLoopExit RunPreparedApplicationMainLoop(JNIEnv* env);
}
