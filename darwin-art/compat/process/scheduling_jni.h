#pragma once
#include <jni.h>
namespace darwin_art::process {
void SetThreadPriority(JNIEnv*, jclass, jint);
void SetCanSelfBackground(JNIEnv*, jclass, jboolean);
void SetThreadPriorityForTid(JNIEnv*, jclass, jint, jint);
jint GetThreadPriority(JNIEnv*, jclass, jint);
jint GetProcessGroup(JNIEnv*, jclass, jint);
void SetThreadGroup(JNIEnv*, jclass, jint, jint);
}
