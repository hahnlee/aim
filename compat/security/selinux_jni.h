#pragma once
#include <jni.h>

namespace aim::security {
// android.os.SELinux natives for a host without SELinux.
bool RegisterSELinuxNatives(JNIEnv* env);
}
