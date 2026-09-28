#pragma once
#include <jni.h>

namespace aim::process {
void SetArgV0(JNIEnv* env, jclass, jstring name);
}
