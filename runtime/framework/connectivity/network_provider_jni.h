#pragma once

#include <jni.h>

namespace aim::framework::connectivity {

bool RegisterNetworkPathProvider(JNIEnv* env, jclass provider_class);

}  // namespace aim::framework::connectivity
