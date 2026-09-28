#pragma once

#include <jni.h>

namespace aim::framework::am {

// Registers the narrow profile-daemon process mechanism used by the
// system-process ActivityManager owner. Android lifecycle policy stays Java-side.
bool RegisterProcessLauncher(JNIEnv* env);

}  // namespace aim::framework::am
