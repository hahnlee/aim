#pragma once
#include <jni.h>

namespace aim::framework::pm {
// Registers DarwinInstalld's transport natives (profile daemon installd ops).
bool RegisterDarwinInstalld(JNIEnv* env, jclass installd);
}
