#pragma once
#include <jni.h>

namespace darwin_art::framework::system {
// Called once during system startup on the prepared main Looper. Returns a
// local reference to AOSP's actual system Context. Failure is terminal because
// ActivityThread.systemMain publishes process-wide state before returning.
jobject CreateSystemContext(JNIEnv* env);
}
