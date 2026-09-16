#pragma once

#include <jni.h>

namespace darwin_art::graphics {

// Process-specialization input corresponding to Android's dumpable state.
void ConfigureGraphicsEnvironment(bool app_debuggable);
void ResetGraphicsEnvironment();

// Android 16 android.os.GraphicsEnvironment native contract.
int RegisterGraphicsEnvironment(JNIEnv* env);

}
