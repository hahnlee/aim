#include <android/native_window.h>

#include <cerrno>
#include <cstdint>

namespace {
struct AndroidNativeBaseAbi {
  int32_t magic;
  int32_t version;
  void* reserved[4];
  void (*inc_ref)(AndroidNativeBaseAbi* base);
  void (*dec_ref)(AndroidNativeBaseAbi* base);
};

struct AndroidNativeWindowAbi {
  AndroidNativeBaseAbi common;
  uint32_t flags;
  int32_t min_swap_interval;
  int32_t max_swap_interval;
  float xdpi;
  float ydpi;
  intptr_t oem[4];
  int (*set_swap_interval)(AndroidNativeWindowAbi*, int);
  int (*dequeue_buffer_deprecated)(AndroidNativeWindowAbi*, void**);
  int (*lock_buffer_deprecated)(AndroidNativeWindowAbi*, void*);
  int (*queue_buffer_deprecated)(AndroidNativeWindowAbi*, void*);
  int (*query)(const AndroidNativeWindowAbi*, int, int*);
  int (*perform)(AndroidNativeWindowAbi*, int, ...);
};

constexpr int kNativeWindowIsValid = 17;
constexpr int kNativeWindowSetBuffersDataspace = 19;
constexpr int kNativeWindowDataspace = 20;

int32_t QueryWindow(ANativeWindow* native_window, int what) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  int value = 0;
  const int status = window->query(window, what, &value);
  return status < 0 ? status : value;
}
}  // namespace

// AOSP nativebase's incStrong/decStrong dispatch to these callbacks. The
// public facade must accept any Android Surface owner, not cast it to Darwin.
extern "C" void ANativeWindow_acquire(ANativeWindow* native_window) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  window->common.inc_ref(&window->common);
}

extern "C" void ANativeWindow_release(ANativeWindow* native_window) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  window->common.dec_ref(&window->common);
}

extern "C" int32_t ANativeWindow_getWidth(ANativeWindow* window) {
  return QueryWindow(window, 0);
}
extern "C" int32_t ANativeWindow_getHeight(ANativeWindow* window) {
  return QueryWindow(window, 1);
}
extern "C" int32_t ANativeWindow_getFormat(ANativeWindow* window) {
  return QueryWindow(window, 2);
}

extern "C" int32_t ANativeWindow_setBuffersDataSpace(
    ANativeWindow* native_window, int32_t dataspace) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  int valid = 0;
  if (window == nullptr || window->query == nullptr ||
      window->perform == nullptr ||
      window->query(window, kNativeWindowIsValid, &valid) != 0 || valid == 0) {
    return -EINVAL;
  }
  return window->perform(window, kNativeWindowSetBuffersDataspace, dataspace);
}

extern "C" int32_t ANativeWindow_getBuffersDataSpace(
    ANativeWindow* native_window) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  int dataspace = 0;
  if (window == nullptr || window->query == nullptr ||
      window->query(window, kNativeWindowDataspace, &dataspace) != 0) {
    return -EINVAL;
  }
  return dataspace;
}

// Match AOSP's NDK facade: the producer owns frame-rate validation/state.
// Do not convert an unimplemented producer operation into successful advice.
extern "C" int32_t ANativeWindow_setFrameRate(
    ANativeWindow* native_window, float frame_rate, int8_t compatibility) {
  return ANativeWindow_setFrameRateWithChangeStrategy(native_window, frame_rate,
      compatibility, ANATIVEWINDOW_CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
}

extern "C" int32_t ANativeWindow_setFrameRateWithChangeStrategy(
    ANativeWindow* native_window, float frame_rate, int8_t compatibility,
    int8_t strategy) {
  auto* window = reinterpret_cast<AndroidNativeWindowAbi*>(native_window);
  int valid = 0;
  if (!window || !window->query || !window->perform ||
      window->query(window, kNativeWindowIsValid, &valid) != 0 || !valid) return -EINVAL;
  // system/window.h: NATIVE_WINDOW_SET_FRAME_RATE, default variadic promotions.
  return window->perform(window, 40, static_cast<double>(frame_rate),
      static_cast<int>(compatibility), static_cast<int>(strategy));
}
