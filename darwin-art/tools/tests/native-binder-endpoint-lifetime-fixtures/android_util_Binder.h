#pragma once

#include <binder/IBinder.h>
#include <jni.h>

namespace android {
sp<IBinder> ibinderForJavaObject(JNIEnv* env, jobject obj);
}
