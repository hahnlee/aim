#include "../../compat/art/boot_native_libraries.h"
#include "../../compat/darwin_framework_natives.h"
#include "../../compat/darwin_icu_natives.h"
#include "../../compat/darwin_libcore_natives.h"
#include "../../compat/darwin_openjdk_natives.h"
#include "../../compat/jni/scoped_local_frame.h"

namespace darwin_art::runtime_art {
bool RegisterBootNativeLibraries(JNIEnv* env) {
  if (!platform::art::LoadOpenJdkBootLibrary(env)) return false;
  // Equivalent to libcore JNI_OnLoad's local-reference scope on Android.
  darwin_art_jni_scope::ScopedLocalFrame frame(env);
  if (!frame.valid()) return false;
  if (!RegisterEarlySystemLog(env) || env->ExceptionCheck()) return false;
  if (!RegisterLibcoreNatives(env) || env->ExceptionCheck()) return false;
  register_java_lang_Math(env);
  if (env->ExceptionCheck()) return false;
  if (!RegisterManagedLoadNatives(env) || env->ExceptionCheck()) return false;
  return RegisterIcuCharsetNatives(env) && !env->ExceptionCheck();
}
}  // namespace darwin_art::runtime_art
