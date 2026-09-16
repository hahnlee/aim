#pragma once
#include <jni.h>
namespace android {
// Original AOSP registrars, provided by the pinned framework app JNI archive.
int register_android_app_Activity(JNIEnv*);
int register_android_app_ActivityThread(JNIEnv*);
}
