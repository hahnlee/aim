#include "host_display_facts_jni.h"

#import <AppKit/AppKit.h>
#include <CoreGraphics/CoreGraphics.h>

#include <iterator>

namespace darwin_art::framework::display {
namespace {

bool Online(CGDirectDisplayID display) {
  uint32_t count = 0;
  CGDirectDisplayID displays[32];
  if (CGGetOnlineDisplayList(32, displays, &count) != kCGErrorSuccess) return false;
  for (uint32_t i = 0; i < count; ++i) {
    if (displays[i] == display) return true;
  }
  return false;
}

// {builtIn, xDpi, yDpi}: pixels of the current mode per physical inch.
jfloatArray NativeDescribe(JNIEnv* env, jclass, jint display_id) {
  const auto display = static_cast<CGDirectDisplayID>(display_id);
  if (!Online(display)) return nullptr;
  const CGSize millimeters = CGDisplayScreenSize(display);
  CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display);
  if (mode == nullptr) return nullptr;
  const double pixel_width = static_cast<double>(CGDisplayModeGetPixelWidth(mode));
  const double pixel_height = static_cast<double>(CGDisplayModeGetPixelHeight(mode));
  CGDisplayModeRelease(mode);
  if (millimeters.width <= 0 || millimeters.height <= 0 || pixel_width <= 0 ||
      pixel_height <= 0) {
    return nullptr;
  }
  const jfloat values[] = {CGDisplayIsBuiltin(display) ? 1.0f : 0.0f,
                           static_cast<jfloat>(pixel_width * 25.4 / millimeters.width),
                           static_cast<jfloat>(pixel_height * 25.4 / millimeters.height)};
  jfloatArray result = env->NewFloatArray(std::size(values));
  if (result != nullptr) env->SetFloatArrayRegion(result, 0, std::size(values), values);
  return result;
}

jstring NativeName(JNIEnv* env, jclass, jint display_id) {
  @autoreleasepool {
    for (NSScreen* screen in NSScreen.screens) {
      NSNumber* number = screen.deviceDescription[@"NSScreenNumber"];
      if (number != nil && number.unsignedIntValue == static_cast<uint32_t>(display_id)) {
        NSString* name = screen.localizedName;
        return name == nil ? nullptr : env->NewStringUTF(name.UTF8String);
      }
    }
  }
  return nullptr;
}

}  // namespace

bool RegisterHostDisplayFacts(JNIEnv* env, jclass facts_class) {
  if (env == nullptr || facts_class == nullptr || env->ExceptionCheck()) return false;
  JNINativeMethod methods[] = {
      {const_cast<char*>("nativeDescribe"), const_cast<char*>("(I)[F"),
       reinterpret_cast<void*>(NativeDescribe)},
      {const_cast<char*>("nativeName"), const_cast<char*>("(I)Ljava/lang/String;"),
       reinterpret_cast<void*>(NativeName)},
  };
  return env->RegisterNatives(facts_class, methods, std::size(methods)) == JNI_OK;
}

}  // namespace darwin_art::framework::display
