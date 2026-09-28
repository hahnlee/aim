#pragma once

#include <jni.h>

namespace aim::binder {

int ServeRpcContext(JNIEnv* env, jobject root, const char* path);

}  // namespace aim::binder
