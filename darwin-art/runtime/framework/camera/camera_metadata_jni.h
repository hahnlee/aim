#pragma once

#include <jni.h>

namespace darwin_art::camera {

// Registers the framework-owned CameraMetadataNative JNI boundary. The
// implementation belongs to the Android camera compatibility component, not
// to an application bridge.
bool RegisterCameraMetadataNatives(JNIEnv* env);

}  // namespace darwin_art::camera
