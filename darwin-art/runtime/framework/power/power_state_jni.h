#pragma once

#include <jni.h>

namespace darwin_art::framework::power {

bool RegisterPowerStateProvider(JNIEnv* env, jclass provider_class);

}  // namespace darwin_art::framework::power
