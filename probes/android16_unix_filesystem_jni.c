#include <jni.h>
#include <stddef.h>

JNIEXPORT void JNICALL Java_java_io_UnixFileSystem_initIDs(JNIEnv*, jclass);
JNIEXPORT jstring JNICALL Java_java_io_UnixFileSystem_canonicalize0(
    JNIEnv*, jobject, jstring, jboolean);
JNIEXPORT jint JNICALL Java_java_io_UnixFileSystem_getBooleanAttributes0(
    JNIEnv*, jobject, jstring);
JNIEXPORT jlong JNICALL Java_java_io_UnixFileSystem_getNameMax0(
    JNIEnv*, jobject, jstring);
JNIEXPORT jboolean JNICALL Java_java_io_UnixFileSystem_setPermission0(
    JNIEnv*, jobject, jobject, jint, jboolean, jboolean);
JNIEXPORT jlong JNICALL Java_java_io_UnixFileSystem_getLastModifiedTime0(
    JNIEnv*, jobject, jobject);
JNIEXPORT jboolean JNICALL Java_java_io_UnixFileSystem_createFileExclusively0(
    JNIEnv*, jclass, jstring);
JNIEXPORT jobjectArray JNICALL Java_java_io_UnixFileSystem_list0(
    JNIEnv*, jobject, jobject);
JNIEXPORT jboolean JNICALL Java_java_io_UnixFileSystem_createDirectory0(
    JNIEnv*, jobject, jobject);
JNIEXPORT jboolean JNICALL Java_java_io_UnixFileSystem_setLastModifiedTime0(
    JNIEnv*, jobject, jobject, jlong);
JNIEXPORT jboolean JNICALL Java_java_io_UnixFileSystem_setReadOnly0(
    JNIEnv*, jobject, jobject);
JNIEXPORT jlong JNICALL Java_java_io_UnixFileSystem_getSpace0(
    JNIEnv*, jobject, jobject, jint);

static JNINativeMethod kSmokeMethods[] = {
    {"initIDs", "()V", (void*)Java_java_io_UnixFileSystem_initIDs},
    {"canonicalize0", "(Ljava/lang/String;Z)Ljava/lang/String;",
     (void*)Java_java_io_UnixFileSystem_canonicalize0},
    {"getBooleanAttributes0", "(Ljava/lang/String;)I",
     (void*)Java_java_io_UnixFileSystem_getBooleanAttributes0},
    {"getNameMax0", "(Ljava/lang/String;)J",
     (void*)Java_java_io_UnixFileSystem_getNameMax0},
    {"setPermission0", "(Ljava/io/File;IZZ)Z",
     (void*)Java_java_io_UnixFileSystem_setPermission0},
    {"getLastModifiedTime0", "(Ljava/io/File;)J",
     (void*)Java_java_io_UnixFileSystem_getLastModifiedTime0},
    {"createFileExclusively0", "(Ljava/lang/String;)Z",
     (void*)Java_java_io_UnixFileSystem_createFileExclusively0},
    {"list0", "(Ljava/io/File;)[Ljava/lang/String;",
     (void*)Java_java_io_UnixFileSystem_list0},
    {"createDirectory0", "(Ljava/io/File;)Z",
     (void*)Java_java_io_UnixFileSystem_createDirectory0},
    {"setLastModifiedTime0", "(Ljava/io/File;J)Z",
     (void*)Java_java_io_UnixFileSystem_setLastModifiedTime0},
    {"setReadOnly0", "(Ljava/io/File;)Z",
     (void*)Java_java_io_UnixFileSystem_setReadOnly0},
    {"getSpace0", "(Ljava/io/File;I)J",
     (void*)Java_java_io_UnixFileSystem_getSpace0},
};

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void* reserved) {
  (void)reserved;
  JNIEnv* env = NULL;
  if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) {
    return JNI_ERR;
  }
  jclass smoke = (*env)->FindClass(
      env, "dev/aim/probe/UnixFileSystemDarwinSmoke");
  if (smoke == NULL ||
      (*env)->RegisterNatives(
          env, smoke, kSmokeMethods,
          (jint)(sizeof(kSmokeMethods) / sizeof(kSmokeMethods[0]))) != JNI_OK) {
    (*env)->DeleteLocalRef(env, smoke);
    return JNI_ERR;
  }
  (*env)->DeleteLocalRef(env, smoke);
  return JNI_VERSION_1_6;
}
