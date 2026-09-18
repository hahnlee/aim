#pragma once
#include <jni.h>

namespace darwin_art::window {

// Registers the android.view.Surface JNI owner without exposing ANativeWindow
// or BLAST internals to the framework registrar.
bool RegisterSurfaceNatives(JNIEnv* env);

}  // namespace darwin_art::window

