#pragma once
#include <jni.h>

namespace darwin_art::framework::pm {
// Registers DarwinInstalld's transport natives (profile daemon installd ops).
bool RegisterDarwinInstalld(JNIEnv* env, jclass installd);
}
