#pragma once
#include <jni.h>
namespace darwin_art::framework::am {
bool DispatchApplicationBinding(JNIEnv* env, jobject endpoint, jobject info,
    jobject resources, jstring process_name, jobject providers);
using PackageResolver = jstring (*)(JNIEnv*, jclass, jstring);
bool RegisterActivityManager(JNIEnv* env, jclass endpoint, PackageResolver resolver);
}
