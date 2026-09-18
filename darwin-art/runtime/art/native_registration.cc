#include "native_registration.h"

#include "../../compat/darwin_framework_natives.h"
#include "../../compat/jni/scoped_local_frame.h"
#include "process_state.h"
#include "darwin_art/android_runtime_host.h"

#include "runtime.h"

namespace darwin_art::runtime_art {

int StartNativeRegistration(JNIEnv* env, art::Thread* self) {
  if (env == nullptr || self == nullptr) return 4;

  if (!art::Runtime::Current()->Start() || env->ExceptionCheck()) return 4;

  // Binder and ClassLoader callbacks need the process VM in every flavor.
  // This is VM ownership, not installation of a graphics/resource backend.
  if (darwin_art_android_runtime_install(env) != DARWIN_ART_ANDROID_RUNTIME_OK)
    return 38;
  darwin_art_process::record_framework_vm_bound();

  {
    darwin_art_jni_scope::ScopedLocalFrame framework_frame(env);
    if (!framework_frame.valid()) return 34;
    if (!darwin_art::RegisterFrameworkNatives(env)) return 26;
  }
#if defined(DARWIN_ART_REAL_GRAPHICS)
  if (!darwin_art::RegisterFrameworkResourceNatives(env)) return 39;
  if (!darwin_art::RegisterFrameworkGraphicsNatives(env)) return 35;
#endif
  return 0;
}

}  // namespace darwin_art::runtime_art
