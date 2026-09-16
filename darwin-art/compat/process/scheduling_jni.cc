#include "scheduling_jni.h"
extern "C" int darwin_art_thread_get_nice(int, int*);
extern "C" int darwin_art_thread_set_nice(int, int);
extern "C" int darwin_art_thread_get_group(int, int*);
extern "C" int darwin_art_thread_set_group(int, int);

namespace darwin_art::process {
namespace {
void SchedulingError(JNIEnv* env, int error) {
  if (error == 0 || env->ExceptionCheck()) return;
  const char* type = error == 3 || error == 22
      ? "java/lang/IllegalArgumentException"
      : error == 1 || error == 13 ? "java/lang/SecurityException"
      : "java/lang/RuntimeException";
  jclass klass = env->FindClass(type);
  if (klass != nullptr) env->ThrowNew(klass,
      error == 3 ? "Thread does not exist in this process" :
      error == 22 ? "Invalid thread priority or identifier" :
      "Darwin thread scheduling operation failed");
  env->DeleteLocalRef(klass);
}
}
void SetThreadPriorityForTid(JNIEnv* env, jclass, jint tid, jint priority) {
  SchedulingError(env, darwin_art_thread_set_nice(tid, priority));
}
void SetThreadPriority(JNIEnv* env, jclass klass, jint priority) {
  SetThreadPriorityForTid(env, klass, 0, priority);
}
jint GetThreadPriority(JNIEnv* env, jclass, jint tid) {
  int value = 0;
  SchedulingError(env, darwin_art_thread_get_nice(tid, &value));
  return value;
}
jint GetProcessGroup(JNIEnv* env, jclass, jint tid) {
  int group = -1;
  SchedulingError(env, darwin_art_thread_get_group(tid, &group));
  return group;
}
void SetThreadGroup(JNIEnv* env, jclass, jint tid, jint group) {
  SchedulingError(env, darwin_art_thread_set_group(tid, group));
}
}
// Dynamic lookup and explicit registration share exactly one implementation.
extern "C" JNIEXPORT void Java_android_os_Process_setThreadPriority__I(
    JNIEnv* env, jclass klass, jint priority) {
  darwin_art::process::SetThreadPriority(env, klass, priority);
}
extern "C" JNIEXPORT void Java_android_os_Process_setThreadPriority__II(
    JNIEnv* env, jclass klass, jint tid, jint priority) {
  darwin_art::process::SetThreadPriorityForTid(env, klass, tid, priority);
}
extern "C" JNIEXPORT jint Java_android_os_Process_getThreadPriority(
    JNIEnv* env, jclass klass, jint tid) {
  return darwin_art::process::GetThreadPriority(env, klass, tid);
}
extern "C" JNIEXPORT jint Java_android_os_Process_getProcessGroup(
    JNIEnv* env, jclass klass, jint tid) {
  return darwin_art::process::GetProcessGroup(env, klass, tid);
}
extern "C" JNIEXPORT void Java_android_os_Process_setThreadGroup(
    JNIEnv* env, jclass klass, jint tid, jint group) {
  darwin_art::process::SetThreadGroup(env, klass, tid, group);
}
