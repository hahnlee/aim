#pragma once

#include <jni.h>

namespace darwin_art::graphics {

// Registers the framework OverlayProperties lifetime ABI. The display
// service reports no dedicated hardware-overlay plane on the Metal backend;
// these natives still have to exist because the Parcelable CREATOR owns a
// NativeAllocationRegistry in the Android framework class initializer.
bool RegisterOverlayPropertiesNatives(JNIEnv* env);

}  // namespace darwin_art::graphics
