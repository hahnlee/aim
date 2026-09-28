#pragma once

#include <jni.h>

namespace aim::framework::system {

// Attach the Java service directory to original libbinder's handle-zero
// dispatch and start the process-owned Binder worker pool.
bool StartKernelBinderService(JNIEnv* env, jobject service_directory);

}  // namespace aim::framework::system
