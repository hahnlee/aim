#pragma once
#include <jni.h>

namespace darwin_art::security {
// com.android.internal.security.VerityUtils natives on a host filesystem
// without fs-verity.
bool RegisterVerityUtilsNatives(JNIEnv* env);
}
