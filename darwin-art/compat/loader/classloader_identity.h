#pragma once
#include <cstdint>
#include <jni.h>

namespace darwin_art::loader {
// JNI identity owner, independent of resident libraries. Zero denotes boot or
// failure/missing; non-boot generations are never reused. Weak refs do not pin
// an otherwise unreachable loader. Caller must have a live JNIEnv.
uint64_t FindClassLoaderIdentity(JNIEnv* env, jobject loader);
uint64_t EnsureClassLoaderIdentity(JNIEnv* env, jobject loader);
void ReleaseClassLoaderIdentities(JNIEnv* env);
}
