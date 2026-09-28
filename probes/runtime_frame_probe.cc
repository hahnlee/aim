#include "runtime_frame_probe.h"

#include <cstdint>
#include <vector>

namespace aim_frame_probe {
namespace {

void* g_host_context = nullptr;
aim_frame_callback_t g_frame_callback = nullptr;
Dimensions g_dimensions;

}  // namespace

void configure(void* host_context, aim_frame_callback_t callback) {
  g_host_context = host_context;
  g_frame_callback = callback;
  g_dimensions = {};
}

void reset() {
  g_host_context = nullptr;
  g_frame_callback = nullptr;
  g_dimensions = {};
}

jboolean present(JNIEnv* env, jint width, jint height, jintArray argb) {
  if (env == nullptr || width <= 0 || height <= 0 || width > 4096 ||
      height > 4096 || argb == nullptr) {
    return JNI_FALSE;
  }
  const std::size_t pixel_count = static_cast<std::size_t>(width) *
                                  static_cast<std::size_t>(height);
  if (env->GetArrayLength(argb) != static_cast<jsize>(pixel_count)) {
    return JNI_FALSE;
  }
  std::vector<jint> pixels(pixel_count);
  env->GetIntArrayRegion(argb, 0, static_cast<jsize>(pixel_count), pixels.data());
  if (env->ExceptionCheck()) {
    return JNI_FALSE;
  }
  const bool presented =
      g_frame_callback == nullptr ||
      g_frame_callback(
          g_host_context, reinterpret_cast<const std::uint32_t*>(pixels.data()),
          static_cast<std::uint32_t>(width), static_cast<std::uint32_t>(height),
          static_cast<std::size_t>(width) * sizeof(std::uint32_t)) != 0;
  if (presented) {
    record_dimensions(width, height);
  }
  return presented ? JNI_TRUE : JNI_FALSE;
}

void record_dimensions(jint width, jint height) {
  if (width > 0 && height > 0) {
    g_dimensions = {static_cast<std::size_t>(width),
                    static_cast<std::size_t>(height)};
  }
}

Dimensions dimensions() { return g_dimensions; }

}  // namespace aim_frame_probe
