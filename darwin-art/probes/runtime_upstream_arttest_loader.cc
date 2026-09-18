#include <iostream>

#include "jni.h"

// AOSP run-test's shared libarttest announces successful loading and unloading.
// Use the process stream just like test/004-JniTest/jni_test.cc: JNI_OnUnload
// runs after the VM has detached the shutdown thread, so no JNIEnv exists.
#if !defined(DARWIN_ART_TEST_HAS_JNI_ONLOAD)
extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM*, void*) {
  std::cout << "JNI_OnLoad called" << std::endl;
  return JNI_VERSION_1_6;
}
#endif

#if !defined(DARWIN_ART_TEST_HAS_JNI_ONUNLOAD)
extern "C" JNIEXPORT void JNICALL JNI_OnUnload(JavaVM*, void*) {
  std::cout << "JNI_OnUnload called" << std::endl;
}
#endif
