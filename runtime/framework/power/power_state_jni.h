#pragma once

#include <jni.h>

namespace aim::framework::power {

bool RegisterPowerStateProvider(JNIEnv* env, jclass provider_class);
bool RegisterBatteryStateProvider(JNIEnv* env, jclass provider_class);

}  // namespace aim::framework::power
