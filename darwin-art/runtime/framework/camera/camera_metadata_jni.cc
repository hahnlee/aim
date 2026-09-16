#include "camera_metadata_jni.h"

#include <array>

namespace darwin_art::camera {
namespace {

// CameraMetadataNative.setupGlobalVendorTagDescriptor() treats zero as
// android::OK. A host with no published camera devices has no vendor metadata
// namespace to install, so the empty descriptor is already complete.
jint SetupGlobalVendorTagDescriptor(JNIEnv*, jclass) { return 0; }

}  // namespace

bool RegisterCameraMetadataNatives(JNIEnv* env) {
  jclass metadata =
      env->FindClass("android/hardware/camera2/impl/CameraMetadataNative");
  if (metadata == nullptr) {
    return false;
  }

  const std::array<JNINativeMethod, 1> methods{{
      {const_cast<char*>("nativeSetupGlobalVendorTagDescriptor"),
       const_cast<char*>("()I"),
       reinterpret_cast<void*>(&SetupGlobalVendorTagDescriptor)},
  }};
  const bool registered =
      env->RegisterNatives(metadata, methods.data(), methods.size()) == JNI_OK;
  env->DeleteLocalRef(metadata);
  return registered;
}

}  // namespace darwin_art::camera
