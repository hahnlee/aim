#include "process_registration.h"
#include "../os/service_process_transport.h"
#include "../wm/desktop_window_metadata.h"
#include "../wm/desktop_root_client_jni.h"
#include "../wm/desktop_root_geometry_jni.h"
#include "../wm/root_key_decision_jni.h"
#include "../wm/desktop_foreground_authority_jni.h"

#include <iostream>

namespace darwin_art::framework::app {

int FinishFrameworkRegistration(JNIEnv* env, bool prepare_looper) {
  if (env == nullptr || env->ExceptionCheck()) return 4;
  if (!darwin_art::framework::os::RegisterSystemServiceClientTransport(env)) return 4;
  if (!darwin_art::framework::wm::RegisterDesktopWindowMetadataClient(env)) return 4;
  if (!darwin_art::framework::wm::RegisterDesktopRootClient(env)) return 4;
  if (!darwin_art::framework::wm::RegisterDesktopRootGeometryClient(env)) return 4;
  if (!darwin_art::framework::wm::RegisterRootKeyDecisionClient(env)) return 4;
  if (!darwin_art::framework::wm::RegisterDesktopForegroundAuthority(env)) return 4;

  // The boot JNI owner already installed AOSP Runtime's complete native
  // table before entering this phase. Do not replace nativeLoad from an app
  // entry point or depend on a weak-symbol alternate registration path.

  if (!prepare_looper) return 0;

  jclass looper_class = env->FindClass("android/os/Looper");
  const jmethodID prepare_main_looper =
      looper_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(looper_class, "prepareMainLooper", "()V");
  if (prepare_main_looper == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(looper_class);
    std::cerr << "ART Android framework: Looper.prepareMainLooper() lookup failed\n";
    return 25;
  }
  env->CallStaticVoidMethod(looper_class, prepare_main_looper);
  env->DeleteLocalRef(looper_class);
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android framework: Looper.prepareMainLooper() failed\n";
    return 25;
  }
  return 0;
}

}  // namespace darwin_art::framework::app
