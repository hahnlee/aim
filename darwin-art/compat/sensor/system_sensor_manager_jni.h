#pragma once

#include <jni.h>

namespace darwin_art::sensor {
// Registers the pinned Android 16 SystemSensorManager and BaseEventQueue JNI
// contracts. The sensor inventory and manager identity belong to sensor_ndk.
bool RegisterSystemSensorManagerNatives(JNIEnv* env);
}
