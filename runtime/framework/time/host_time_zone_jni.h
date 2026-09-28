#pragma once

#include <jni.h>

namespace aim::framework::time {

bool RegisterHostTimeZoneProvider(JNIEnv* env, jclass provider_class);

}  // namespace aim::framework::time
