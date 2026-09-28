#ifndef AIM_PROCESS_PROCFS_JNI_H_
#define AIM_PROCESS_PROCFS_JNI_H_

#include <jni.h>

namespace aim::process {

// Android 16 android.os.Process proc parsers. Files are opened through the
// guest filesystem provider; callers never receive a host procfs descriptor.
void ReadProcLines(JNIEnv*, jobject, jstring, jobjectArray, jlongArray);
jboolean ReadProcFile(JNIEnv*, jobject, jstring, jintArray, jobjectArray,
                      jlongArray, jfloatArray);
jboolean ParseProcLine(JNIEnv*, jobject, jbyteArray, jint, jint, jintArray,
                       jobjectArray, jlongArray, jfloatArray);
jlong GetFreeMemory(JNIEnv*, jobject);
jlong GetTotalMemory(JNIEnv*, jobject);

}  // namespace aim::process

#endif  // AIM_PROCESS_PROCFS_JNI_H_
