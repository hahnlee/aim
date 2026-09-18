#pragma once

#include <jni.h>

namespace darwin_art::input {
// Requires the original android_view_KeyCharacterMap JNI owner to be registered
// first. Resource selection belongs to SystemKeyboardMaps, not this adapter.
bool RegisterSystemKeyboardMapsNatives(JNIEnv* env);
}
