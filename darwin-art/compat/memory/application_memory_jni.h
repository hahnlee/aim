#pragma once
#include <jni.h>

namespace android {
// Original AOSP libandroid_runtime registrars, linked from the pinned archive.
int register_com_android_internal_os_ApplicationSharedMemory(JNIEnv* env);
int register_android_app_PropertyInvalidatedCache(JNIEnv* env);
}
