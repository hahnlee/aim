#include "key_character_map_jni.h"

#include "system_keyboard_maps_jni.h"

namespace android {
int register_android_view_KeyEvent(JNIEnv*);
int register_android_view_KeyCharacterMap(JNIEnv*);
}

namespace darwin_art::input {
bool RegisterKeyCharacterMapNatives(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  // Initialize the original converter before maps generate KeyEvent arrays.
  // Original JNI owns pointers, matching and complete Parcel payloads.
  if (android::register_android_view_KeyEvent(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  if (android::register_android_view_KeyCharacterMap(env) != JNI_OK ||
      env->ExceptionCheck()) return false;
  return RegisterSystemKeyboardMapsNatives(env);
}
}  // namespace darwin_art::input
