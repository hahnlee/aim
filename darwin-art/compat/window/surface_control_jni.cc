#include "surface_control_jni.h"
#include <cstdio>
#include <cstdlib>
#include <cstdint>

struct ASurfaceControl;
extern "C" void ASurfaceControl_acquire(ASurfaceControl*);

namespace {
[[noreturn]] void InvalidSurfaceControl() {
  std::fputs("ASurfaceControl_fromJava requires a live android.view.SurfaceControl\n", stderr);
  std::abort();
}
}

// Android native/android/surface_control.cpp retains the control owned by the
// supplied Java object. Never manufacture a second layer for this conversion.
// Native storage still belongs to the existing SurfaceControl owner; replacing
// that owner with libgui must update the retain implementation as one contract.
extern "C" ASurfaceControl* ASurfaceControl_fromJava(JNIEnv* env, jobject object) {
  if (!env || !object) InvalidSurfaceControl();
  if (env->ExceptionCheck()) return nullptr;
  jclass type = env->FindClass("android/view/SurfaceControl");
  if (!type) return nullptr;  // Preserve the VM's class-resolution exception.
  if (!env->IsInstanceOf(object, type)) {
    env->DeleteLocalRef(type);
    InvalidSurfaceControl();
  }
  jfieldID field = env->GetFieldID(type, "mNativeObject", "J");
  env->DeleteLocalRef(type);
  if (!field || env->ExceptionCheck()) return nullptr;
  jlong native = env->GetLongField(object, field);
  if (env->ExceptionCheck()) return nullptr;
  if (!native) InvalidSurfaceControl();
  auto* control = reinterpret_cast<ASurfaceControl*>(static_cast<uintptr_t>(native));
  ASurfaceControl_acquire(control);
  return control;
}
