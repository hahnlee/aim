#pragma once
#include <jni.h>

namespace aim::window {

// Registers the android.view.Surface JNI owner without exposing ANativeWindow
// or BLAST internals to the framework registrar.
bool RegisterSurfaceNatives(JNIEnv* env);

}  // namespace aim::window

