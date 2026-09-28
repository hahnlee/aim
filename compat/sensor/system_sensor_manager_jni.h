#pragma once

#include <jni.h>

namespace aim::sensor {
// Registers the pinned Android 16 SystemSensorManager and BaseEventQueue JNI
// contracts. The sensor inventory and manager identity belong to sensor_ndk.
bool RegisterSystemSensorManagerNatives(JNIEnv* env);
}
