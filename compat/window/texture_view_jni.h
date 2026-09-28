#pragma once

#include <jni.h>

namespace aim::window {

// Registers the four Android-owned TextureView JNI methods.  The native
// window is a retained SurfaceTexture producer held by TextureView.mNativeWindow.
bool RegisterTextureViewNatives(JNIEnv* env);

}  // namespace aim::window
