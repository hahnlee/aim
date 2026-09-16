#pragma once

#include <jni.h>

namespace darwin_art::binder {

int ServeRpcContext(JNIEnv* env, jobject root, const char* path);

}  // namespace darwin_art::binder
