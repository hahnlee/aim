#pragma once

#include <jni.h>

namespace aim {

bool RegisterIcuCharsetNatives(JNIEnv* env);
void ShutdownIcuCharsetNatives();

}  // namespace aim
