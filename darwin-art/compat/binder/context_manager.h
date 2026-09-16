#pragma once

#include <jni.h>

namespace darwin_art {
// Returns a local reference to this process's system service-manager capability.
// One manager/channel per process; no fixture or app-local service fallback.
// The global reference and channel live until process exit, like ProcessState's
// context object. Connection errors remain Java exceptions and are not cached.
jobject GetSystemContextObject(JNIEnv* env);
}  // namespace darwin_art
