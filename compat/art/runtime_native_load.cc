#include <jni.h>

// Both runtime flavors expose the same Android native-load ABI. The AOSP JVM
// owner performs class-loader lookup, loading and error reporting; the Darwin
// boundary must never report success without loading a library. RegisterNatives
// normally installs this owner, while standard JNI resolution needs these names.
extern "C" jstring JVM_NativeLoad(JNIEnv*, jstring, jobject, jclass);

extern "C" JNIEXPORT jstring Java_java_lang_Runtime_nativeLoad(
    JNIEnv* env, jclass, jstring filename, jobject loader, jclass caller) {
  return JVM_NativeLoad(env, filename, loader, caller);
}

extern "C" JNIEXPORT jstring
Java_java_lang_Runtime_nativeLoad__Ljava_lang_String_2Ljava_lang_ClassLoader_2Ljava_lang_Class_2(
    JNIEnv* env, jclass ignored, jstring filename, jobject loader, jclass caller) {
  return Java_java_lang_Runtime_nativeLoad(env, ignored, filename, loader, caller);
}
