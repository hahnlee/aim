#pragma once

#include <jni.h>

namespace darwin_art::framework::connectivity {

bool RegisterNetworkPathProvider(JNIEnv* env, jclass provider_class);

}  // namespace darwin_art::framework::connectivity
