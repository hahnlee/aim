#pragma once
#include <jni.h>
namespace android {
// Original AOSP registrar; no alternate event store in the JNI adapter.
int register_com_android_internal_os_DebugStore(JNIEnv*);
}
