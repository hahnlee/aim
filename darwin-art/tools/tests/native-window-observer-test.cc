#include "../../compat/darwin_angle_egl.h"
#include "../../compat/media/consumer_buffer.h"

#include <cassert>
#include <cstdio>
#include <csignal>
#include <unistd.h>

#include <system/window.h>

struct Context {
  void* window;
  int releases = 0;
};
static void Queue(void*, AHardwareBuffer*, int32_t, int, int32_t) {}
static void Release(void* opaque) {
  auto* context = static_cast<Context*>(opaque);
  // Real consumer teardown may return slots or query its still-retained
  // producer. This takes the actual producer mutex, not a mocked lock.
  assert(darwin_art_android_ANativeWindow_next_frame_number(context->window) == 1);
  ++context->releases;
}
int main() {
  // Bound a regression deadlock without terminating any app/profile process.
  std::signal(SIGALRM, [](int) { _exit(124); });
  alarm(5);
  void* window = darwin_art_android_ANativeWindow_create(16, 16, 1);
  assert(window != nullptr);
  auto* native_window = static_cast<ANativeWindow*>(window);
  // HWUI obtains the BLAST transaction target through this exact Android
  // perform operation. Returning success without initializing the out value
  // turns an arbitrary stack word into a permanently-future frame id.
  assert(ANativeWindow_getNextFrameId(native_window) == 1);
  Context first{window}, second{window};
  assert(!darwin_art_android_ANativeWindow_set_owned_queue_callback(
      nullptr, Queue, &first, Release));
  assert(first.releases == 0);
  assert(darwin_art_android_ANativeWindow_set_owned_queue_callback(
      window, Queue, &first, Release));
  assert(darwin_art_android_ANativeWindow_set_owned_queue_callback(
      window, Queue, &second, Release));
  assert(first.releases == 1 && second.releases == 0);
  assert(darwin_art_android_ANativeWindow_set_owned_queue_callback(
      window, nullptr, nullptr, nullptr));
  assert(first.releases == 1 && second.releases == 1);
  assert(darwin_art_android_ANativeWindow_set_owned_queue_callback(
      window, nullptr, nullptr, nullptr));
  assert(first.releases == 1 && second.releases == 1);
  darwin_art_android_ANativeWindow_release(window);
  window = darwin_art_android_ANativeWindow_create(16, 16, 1);
  assert(window != nullptr);
  {
    darwin_art::media::OwnedConsumerBuffer lease(window, -1, -1, nullptr);
    darwin_art_android_ANativeWindow_release(window);
    assert(darwin_art_android_ANativeWindow_is_managed(window));
    assert(darwin_art_android_ANativeWindow_next_frame_number(window) == 1);
  }
  // Registry query accepts the retired opaque identity without dereferencing it.
  assert(!darwin_art_android_ANativeWindow_is_managed(window));
  alarm(0);
  std::puts("native-window-observer: PASS real producer reentrant release, exactly once");
}
