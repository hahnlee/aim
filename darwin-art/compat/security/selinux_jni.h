#pragma once
#include <jni.h>

namespace darwin_art::security {
// android.os.SELinux natives for a host without SELinux.
bool RegisterSELinuxNatives(JNIEnv* env);
}
