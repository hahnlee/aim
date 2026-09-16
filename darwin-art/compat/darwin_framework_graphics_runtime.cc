#include "darwin_framework_natives.h"
#include "graphics/graphics_environment.h"
#include "graphics/overlay_properties_natives.h"

#if defined(DARWIN_ART_REAL_GRAPHICS)
#include <android/graphics/jni_runtime.h>
#include <androidicuinit/android_icu_init.h>

#include "darwin_art/android_runtime_host.h"
#include "darwin_android_graphics_registration.h"
extern "C" void darwin_art_hwui_shutdown_common_pool();
#endif

namespace {

#if defined(DARWIN_ART_REAL_GRAPHICS)
bool SetJavaSystemProperty(JNIEnv* env, const char* key, const char* value) {
  jclass system = env->FindClass("java/lang/System");
  jmethodID set_property =
      system == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                system, "setProperty",
                "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;");
  jstring java_key = set_property == nullptr ? nullptr : env->NewStringUTF(key);
  jstring java_value =
      java_key == nullptr ? nullptr : env->NewStringUTF(value);
  jobject previous =
      java_value == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->CallStaticObjectMethod(system, set_property, java_key,
                                        java_value);
  const bool success = !env->ExceptionCheck();
  env->DeleteLocalRef(previous);
  env->DeleteLocalRef(java_value);
  env->DeleteLocalRef(java_key);
  env->DeleteLocalRef(system);
  return success;
}

bool ConfigureLayoutlibGraphicsRegistrar(JNIEnv* env) {
  static_assert(darwin_art::android_graphics::kNativeClassCount == 51);
  return SetJavaSystemProperty(env, "method_binding_format", "") &&
         SetJavaSystemProperty(
             env, darwin_art::android_graphics::kNativeClassesPropertyName,
             darwin_art::android_graphics::kNativeClassesCsv);
}
#endif

}  // namespace

namespace darwin_art {

FrameworkGraphicsBackend GetFrameworkGraphicsBackend() {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  return FrameworkGraphicsBackend::kAndroidGraphics;
#else
  return FrameworkGraphicsBackend::kProbeCanvas;
#endif
}

bool InitializeFrameworkGraphicsRuntime() {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  // This must run before ART initializes java.lang.System. Its static
  // initializer publishes ICU/Unicode/CLDR versions and rejects a null CLDR
  // value, while AOSP ICU on Darwin requires explicit external-data setup.
  android_icu_init();
#endif
  return true;
}

void ShutdownFrameworkAsyncWorkers() {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  darwin_art_hwui_shutdown_common_pool();
#endif
}

void ShutdownFrameworkGraphicsRuntime() {
  graphics::ResetGraphicsEnvironment();
#if defined(DARWIN_ART_REAL_GRAPHICS)
  // The ICU JNI unload hook runs first and calls u_cleanup(); release the
  // external Android ICU data mapping only after no ICU consumer remains.
  android_icu_cleanup();
#endif
}

bool InstallFrameworkResourceRuntime(JNIEnv* env) {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  return darwin_art_android_runtime_install(env) ==
         DARWIN_ART_ANDROID_RUNTIME_OK;
#else
  (void)env;
  return true;
#endif
}

bool ShutdownFrameworkResourceRuntime(JNIEnv* env) {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  return darwin_art_android_runtime_uninstall(env) ==
         DARWIN_ART_ANDROID_RUNTIME_OK;
#else
  (void)env;
  return true;
#endif
}

bool RegisterFrameworkGraphicsNatives(JNIEnv* env) {
#if defined(DARWIN_ART_REAL_GRAPHICS)
  // Registration is atomic at the upstream libandroid_runtime boundary. In
  // particular, never register DarwinPaint/DarwinRenderNode alongside native
  // Canvas or Bitmap: their jlong values have unrelated C++ object layouts
  // and eventually cross through Canvas/RenderNode drawing APIs. The caller
  // must invoke this after Runtime::FinishMinimalForDarwinProbe(): the Darwin
  // host uses LayoutlibLoader, whose registrar calls managed System.getProperty.
  if (!ConfigureLayoutlibGraphicsRegistrar(env)) {
    return false;
  }
  init_android_graphics();
  return register_android_graphics_classes(env) >= 0 &&
         graphics::RegisterOverlayPropertiesNatives(env);
#else
  // ProbeCanvas, DarwinPaint, and DarwinRenderNode were registered atomically
  // by RegisterFrameworkNatives().
  (void)env;
  return true;
#endif
}

}  // namespace darwin_art
