#pragma once
#include <jni.h>

namespace aim::framework::system {
// Registers HostCommandService's relay natives (profile daemon host commands).
bool RegisterHostCommandService(JNIEnv* env, jclass service);
}
