#pragma once

#include <jni.h>

namespace darwin_art::framework::app {

// Completes the framework-owned registration boundary after ART boot natives
// have been installed. Activity processes leave main-Looper creation to
// ActivityThread.main(); non-Activity process entries request it explicitly.
// This does not create an application ClassLoader or load APK code.
int FinishFrameworkRegistration(JNIEnv* env, bool prepare_looper);

}  // namespace darwin_art::framework::app
