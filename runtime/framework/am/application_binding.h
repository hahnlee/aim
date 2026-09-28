#pragma once
#include <jni.h>
namespace aim::framework::am {
bool DispatchApplicationBinding(JNIEnv* env, jobject endpoint, jobject info,
    jobject resources, jstring process_name, jobject providers);
bool RegisterActivityManager(JNIEnv* env, jclass endpoint);
}
