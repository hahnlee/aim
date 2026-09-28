#include "native_registration.h"

#include "../../compat/darwin_framework_natives.h"
#include "../../compat/jni/scoped_local_frame.h"
#include "process_state.h"
#include "aim/android_runtime_host.h"

#include "runtime.h"

namespace aim::runtime_art {

int StartNativeRegistration(JNIEnv* env, art::Thread* self) {
  if (env == nullptr || self == nullptr) return 4;

  if (!art::Runtime::Current()->Start() || env->ExceptionCheck()) return 4;

  // Binder and ClassLoader callbacks need the process VM in every flavor.
  // This is VM ownership, not installation of a graphics/resource backend.
  if (aim_android_runtime_install(env) != AIM_ANDROID_RUNTIME_OK)
    return 38;
  aim_process::record_framework_vm_bound();

  {
    aim_jni_scope::ScopedLocalFrame framework_frame(env);
    if (!framework_frame.valid()) return 34;
    if (!aim::RegisterFrameworkNatives(env)) return 26;
  }
#if defined(AIM_REAL_GRAPHICS)
  if (!aim::RegisterFrameworkResourceNatives(env)) return 39;
  if (!aim::RegisterFrameworkGraphicsNatives(env)) return 35;
#endif
  return 0;
}

}  // namespace aim::runtime_art
