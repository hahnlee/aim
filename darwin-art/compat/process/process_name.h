#pragma once
#include <jni.h>

namespace darwin_art::process {
void SetArgV0(JNIEnv* env, jclass, jstring name);
}
