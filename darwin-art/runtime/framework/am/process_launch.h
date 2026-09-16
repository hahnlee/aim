#pragma once

#include <jni.h>

namespace darwin_art::framework::am {

// Registers the narrow profile-daemon process mechanism used by the
// system-process ActivityManager owner. Android lifecycle policy stays Java-side.
bool RegisterProcessLauncher(JNIEnv* env, jclass endpoint);

}  // namespace darwin_art::framework::am
