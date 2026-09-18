#include <android/native_window.h>
#include <android/rect.h>
#include <jni.h>

struct ACanvas;

#if !defined(DARWIN_ART_REAL_GRAPHICS)

// Surface.lockCanvas is part of the framework ABI in every flavor, but the
// CPU/headless runtime deliberately does not own HWUI's Skia canvas object.
// Keep the ABI total without pulling the graphics apex (and its codec/Skia
// closure) into the headless dylib.  The real graphics flavor supplies the
// AOSP implementations from android_canvas.cpp.
extern "C" bool ACanvas_isSupportedPixelFormat(int32_t) { return false; }

extern "C" ACanvas* ACanvas_getNativeHandleFromJava(JNIEnv*, jobject) {
  return nullptr;
}

extern "C" bool ACanvas_setBuffer(ACanvas*, const ANativeWindow_Buffer*, int32_t) {
  return false;
}

extern "C" void ACanvas_clipRect(ACanvas*, const ARect*, bool) {}

#endif
