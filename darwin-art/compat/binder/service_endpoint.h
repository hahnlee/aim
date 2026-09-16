#pragma once
#include <jni.h>
namespace darwin_art {
// registry remains a caller-owned JNI reference on this thread. The transport
// does not create Android services or fabricate lifecycle/readiness results.
int ServeBinderServiceEndpoint(JNIEnv* env, jobject registry, const char* path);
}
