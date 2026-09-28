#ifndef AIM_ICU_JNI_H_
#define AIM_ICU_JNI_H_

#include <jni.h>

#ifdef __cplusplus
extern "C" {
#endif

// Static-host equivalents of libicu_jni's JNI_OnLoad/JNI_OnUnload. The
// distinct names prevent collisions when several Android JNI libraries are
// linked into one Darwin executable instead of loaded as separate DSOs.
JNIEXPORT jint aim_icu_jni_on_load(JavaVM* vm, void* reserved);
JNIEXPORT void aim_icu_jni_on_unload(JavaVM* vm, void* reserved);

#ifdef __cplusplus
}
#endif

#endif  // AIM_ICU_JNI_H_
