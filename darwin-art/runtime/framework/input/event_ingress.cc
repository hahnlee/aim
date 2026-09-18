#include "event_ingress.h"

#include <cmath>
#include <cstdlib>
#include <iostream>

#include "darwin_android_platform.h"
#include "darwin_android_time.h"
#include "darwin_framework_input_hint.h"
#include "window/desktop_root_surface.h"
#include "surface_input_context.h"
#include "root_key_authority.h"
#include "root_key_ingress.h"
#include "channel_owner.h"
#include "looper/android_looper_owner.h"
#include "../../art/process_state.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

namespace darwin_art_graphics::input {
namespace {

int64_t monotonic_nanos() { return darwin_art::AndroidUptimeNanos(); }

bool valid_time_pair(int64_t event_time_nanos, int64_t down_time_nanos) {
  return event_time_nanos > 0 && down_time_nanos > 0 &&
         down_time_nanos <= event_time_nanos;
}

int32_t enqueue_result_status(darwin_art::DarwinArtInputEnqueueResult result) {
  return result == darwin_art::DarwinArtInputEnqueueResult::kQueued
             ? 0
             : DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
}

int32_t enqueue_pointer(GraphicsState* state,
                        DarwinArtPointerEventV2 packet) {
  if (state == nullptr || packet.action > DARWIN_ART_POINTER_CANCEL ||
      packet.pointer_count == 0 || packet.pointer_count > 16 ||
      !std::isfinite(packet.x) || !std::isfinite(packet.y) ||
      !std::isfinite(packet.raw_x) || !std::isfinite(packet.raw_y) ||
      !std::isfinite(packet.pressure) || !std::isfinite(packet.size_value)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }

  const int64_t event_time = packet.event_time_nanos > 0
                                 ? static_cast<int64_t>(packet.event_time_nanos)
                                 : monotonic_nanos();
  const int64_t down_time = packet.down_time_nanos > 0
                                ? static_cast<int64_t>(packet.down_time_nanos)
                                : (state->pointer_stream_active
                                       ? state->pointer_down_time_nanos
                                       : event_time);
  if (!valid_time_pair(event_time, down_time)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }

  packet.event_time_nanos = static_cast<uint64_t>(event_time);
  packet.down_time_nanos = static_cast<uint64_t>(down_time);
  const auto result = darwin_art::EnqueueFrameworkPointerPacket(packet);
  const int32_t status = enqueue_result_status(result);
  if (status != 0) return status;

  // The session owns the Android stream lifetime. Update it only after the
  // InputChannel accepted the packet, so a not-ready session cannot leave a
  // phantom stream that changes the next event's downTime.
  if (packet.action == DARWIN_ART_POINTER_DOWN) {
    state->pointer_stream_active = true;
    state->pointer_down_time_nanos = down_time;
  } else if (packet.action == DARWIN_ART_POINTER_UP ||
             packet.action == DARWIN_ART_POINTER_CANCEL) {
    state->pointer_stream_active = false;
    state->pointer_down_time_nanos = 0;
  }
  return 0;
}

int32_t enqueue_key(GraphicsState* state, DarwinArtKeyEventV1 packet) {
  if (state == nullptr || packet.action > 1) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }

  const size_t index = static_cast<size_t>(packet.key_code);
  const bool tracks_key = index < state->key_down_time_nanos.size();
  const int64_t event_time = packet.event_time_nanos > 0
                                 ? static_cast<int64_t>(packet.event_time_nanos)
                                 : monotonic_nanos();
  int64_t down_time = packet.down_time_nanos > 0
                          ? static_cast<int64_t>(packet.down_time_nanos)
                          : (tracks_key ? state->key_down_time_nanos[index] : 0);
  if (down_time <= 0) down_time = event_time;
  if (!valid_time_pair(event_time, down_time)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }

  packet.event_time_nanos = static_cast<uint64_t>(event_time);
  packet.down_time_nanos = static_cast<uint64_t>(down_time);
  // This legacy ABI carries no surface/root identity. GraphicsState owns only
  // timestamps, not a WMS grant. Never guess a process-global focused target;
  // physical keys enter the exact surface's retained Android sink instead.
  return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
}

}  // namespace

int32_t dispatch_pointer(GraphicsState* state, uint32_t action, float x,
                         float y) {
  DarwinArtPointerEventV2 packet{};
  packet.version = 2;
  packet.size = sizeof(DarwinArtPointerEventV2);
  packet.action = action;
  packet.pointer_count = 1;
  packet.x = x;
  packet.y = y;
  packet.raw_x = x;
  packet.raw_y = y;
  packet.pressure = action == DARWIN_ART_POINTER_UP ||
                            action == DARWIN_ART_POINTER_CANCEL
                        ? 0.0f
                        : 1.0f;
  packet.size_value = 1.0f;
  return enqueue_pointer(state, packet);
}

int32_t dispatch_pointer_v2(GraphicsState* state,
                            const DarwinArtPointerEventV2* event) {
  if (event == nullptr || event->version != 2 ||
      event->size < sizeof(DarwinArtPointerEventV2)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  return enqueue_pointer(state, *event);
}

int32_t dispatch_key_v1(GraphicsState* state,
                        const DarwinArtKeyEventV1* event) {
  if (event == nullptr || event->version != 1 ||
      event->size < sizeof(DarwinArtKeyEventV1)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  return enqueue_key(state, *event);
}

int32_t pump_main_looper(GraphicsState* state) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  art::Thread* thread = darwin_art_process::owner_thread_for_callback();
  if (thread == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (art::Thread::Current() != thread ||
      thread->GetState() != art::ThreadState::kNative) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  }
  art::ScopedObjectAccess soa(thread);
  const int native_result = darwin_art_android_platform_poll_current_looper();
  if (native_result < 0 &&
      std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
    std::cerr << "ART Android Looper: native poll failed; continuing queue\n";
  }
  // Java MessageQueue/Choreographer work is dispatched by the framework's
  // owner loop. This production boundary only polls native InputChannel work;
  // fixture queue inspection remains in the test-only input translation unit.
  return 0;
}

namespace {

DarwinArtSurfaceInputResult AndroidPointerSink(
    void*, const DarwinArtPointerEventV2* event) {
  if (event == nullptr) return DARWIN_ART_SURFACE_INPUT_INVALID;
  switch (darwin_art::EnqueueFrameworkPointerPacket(*event)) {
    case darwin_art::DarwinArtInputEnqueueResult::kQueued:
      return DARWIN_ART_SURFACE_INPUT_QUEUED;
    case darwin_art::DarwinArtInputEnqueueResult::kBackpressured:
      return DARWIN_ART_SURFACE_INPUT_BACKPRESSURED;
    case darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel:
      return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
  }
  return DARWIN_ART_SURFACE_INPUT_INVALID;
}

DarwinArtSurfaceInputResult AndroidKeySink(
    void* opaque, const DarwinArtKeyEventV1* event) {
  if (opaque == nullptr || event == nullptr || event->version != 1 ||
      event->size < sizeof(DarwinArtKeyEventV1) || event->action > 1)
    return DARWIN_ART_SURFACE_INPUT_INVALID;
  const auto ingress =
      static_cast<darwin_art::input::SurfaceInputContext*>(opaque)->key_ingress();
  if (ingress == nullptr) return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
  switch (ingress->Submit(*event)) {
    case darwin_art::DarwinArtInputEnqueueResult::kQueued:
      return DARWIN_ART_SURFACE_INPUT_QUEUED;
    case darwin_art::DarwinArtInputEnqueueResult::kBackpressured:
      return DARWIN_ART_SURFACE_INPUT_BACKPRESSURED;
    case darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel:
      return DARWIN_ART_SURFACE_INPUT_NO_TARGET;
  }
  return DARWIN_ART_SURFACE_INPUT_INVALID;
}

}  // namespace

extern "C" DarwinArtSurfaceResult darwin_art_android_input_sink_install(
    DarwinArtSurface* surface) {
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  // Surface installation precedes ActivityThread.main. Prepare the native
  // ALooper only on the real ART application owner; MessageQueue.nativeInit
  // later adopts this same TLS owner. No arbitrary callback-thread fallback.
  art::Thread* owner = darwin_art_process::owner_thread_for_callback();
  if (owner == nullptr || art::Thread::Current() != owner ||
      owner->GetState() != art::ThreadState::kNative)
    return DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
  void* owner_looper = darwin_art::looper::PrepareCurrent();
  if (owner_looper == nullptr) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  std::shared_ptr<darwin_art::window::DesktopRootEvents> root;
  const auto root_status =
      darwin_art::window::RetainSurfaceDesktopRoot(surface, &root);
  if (root_status != DARWIN_ART_SURFACE_OK) return root_status;
  darwin_art::input::RootKeyAuthorityAcquireStatus authority_status;
  auto authority =
      darwin_art::input::AcquireRootKeyAuthority(root, &authority_status);
  if (authority == nullptr) {
    using Status = darwin_art::input::RootKeyAuthorityAcquireStatus;
    switch (authority_status) {
      case Status::kClosed:
        return DARWIN_ART_SURFACE_WINDOW_CLOSED;
      case Status::kUnavailable:
        // A concurrent/reentrant factory construction is retryable, not OOM.
        return DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
      case Status::kInvalidRoot:
        return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
      case Status::kAllocationFailed:
      case Status::kAcquired:
        return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
    }
  }
  auto ingress = darwin_art::input::AcquireRootKeyIngress(
      authority, owner_looper, &darwin_art::input::SubmitFrameworkInputAdmission);
  if (ingress == nullptr) return DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
  auto* context = darwin_art::input::SurfaceInputContext::Create(
      std::move(root), std::move(authority), std::move(ingress));
  if (context == nullptr) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  const DarwinArtSurfaceInputSink sink{
      .version = 1,
      .size = static_cast<uint32_t>(sizeof(DarwinArtSurfaceInputSink)),
      .context = context,
      .retain_context = &darwin_art::input::SurfaceInputContext::Retain,
      .release_context = &darwin_art::input::SurfaceInputContext::Release,
      .pointer = &AndroidPointerSink,
      .key = &AndroidKeySink,
  };
  const auto result = darwin_art_surface_set_input_sink(surface, &sink);
  darwin_art::input::SurfaceInputContext::Release(context);
  return result;
}

}  // namespace darwin_art_graphics::input
