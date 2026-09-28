#include "graphics_session.h"

#include <cstdint>

#include "darwin_android_platform.h"
#include "../framework/display/vsync_source.h"
#include "../framework/input/event_ingress.h"
#include "runtime.h"
#include "session_lifetime.h"
#include "thread-current-inl.h"

namespace aim_graphics {

namespace {

int32_t AdmitOwner(aim_graphics_session_t* session,
                   SessionLease* lease) {
  const int32_t preflight =
      preflight_session(session, SessionAdmissionKind::kOwner);
  if (preflight != 0) return preflight;
  art::Thread* current = art::Thread::Current();
  const bool native = current != nullptr &&
                      current->GetState() == art::ThreadState::kNative;
  return admit_session(session, SessionAdmissionKind::kOwner,
                       reinterpret_cast<void*>(current), native, lease);
}

}  // namespace

int32_t bind_session_for_process(void* context) {
  return bind_session_for_process_lifetime(context);
}

int32_t bind_session_art_thread(art::Thread* thread) {
  return bind_session_art_thread_identity(reinterpret_cast<void*>(thread));
}

int32_t close_session(aim_graphics_session_t* session) {
  const int32_t preflight = validate_session(session);
  if (preflight != 0) return preflight;
  art::Thread* current = art::Thread::Current();
  const bool native = current != nullptr &&
                      current->GetState() == art::ThreadState::kNative;
  return aim_graphics::close_session(
      session, reinterpret_cast<void*>(current), native);
}

int32_t finalize_bound_session(GraphicsState* state) {
  const int32_t preflight = validate_state(state);
  if (preflight != 0) return preflight;
  art::Thread* current = art::Thread::Current();
  return aim_graphics::finalize_bound_session(
      state, reinterpret_cast<void*>(current));
}

int32_t dispatch_pointer(aim_graphics_session_t* session,
                         uint32_t action, float x, float y) {
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  return input::dispatch_pointer(lease.state(), action, x, y);
}

int32_t dispatch_pointer_v2(aim_graphics_session_t* session,
                            const AimPointerEventV2* event) {
  if (session == nullptr || event == nullptr) {
    return AIM_STATUS_GRAPHICS_SESSION_INVALID;
  }
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  return input::dispatch_pointer_v2(lease.state(), event);
}

int32_t dispatch_key_v1(aim_graphics_session_t* session,
                        const AimKeyEventV1* event) {
  if (session == nullptr || event == nullptr) {
    return AIM_STATUS_GRAPHICS_SESSION_INVALID;
  }
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  return input::dispatch_key_v1(lease.state(), event);
}

int32_t pump_frame(aim_graphics_session_t* session,
                   int64_t frame_time_nanos) {
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  return display::pump_frame(lease.state(), frame_time_nanos);
}

int32_t pump_main_looper(aim_graphics_session_t* session) {
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  if (lease.looper() == nullptr) {
    void* looper = aim_android_platform_prepare_current_looper();
    set_session_looper(session, looper);
  }
  return input::pump_main_looper(lease.state());
}

int32_t wake_main_looper(aim_graphics_session_t* session) {
  SessionLease lease;
  const int32_t status = admit_session(session, SessionAdmissionKind::kWake,
                                       nullptr, false, &lease);
  if (status != 0) return status;
  // This is deliberately the only cross-thread operation: ALooper_wake is
  // safe to call without entering the owner thread's ART/JNI state.
  aim_android_platform_wake_looper(lease.looper());
  return 0;
}

int32_t wait_main_looper(aim_graphics_session_t* session,
                         int32_t timeout_ms) {
  SessionLease lease;
  const int32_t status = AdmitOwner(session, &lease);
  if (status != 0) return status;
  return aim_android_platform_wait_current_looper(timeout_ms);
}

}  // namespace aim_graphics

// Embedding handles enforce ART owner-thread affinity. DisplayEventReceiver
// remains Android-owned while ActivityThread runs Looper.loop(); the only
// cross-thread session operation wakes that looper without entering ART/JNI.
extern "C" AIM_EXPORT aim_graphics_session_t*
aim_graphics_session_create() {
  return aim_graphics::create_session();
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_close(
    aim_graphics_session_t* session) {
  return aim_graphics::close_session(session);
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_destroy(
    aim_graphics_session_t* session) {
  return aim_graphics::destroy_session(session);
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_dispatch_pointer(
    aim_graphics_session_t* session, uint32_t action, float x, float y) {
  return aim_graphics::dispatch_pointer(session, action, x, y);
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_dispatch_pointer_v2(
    aim_graphics_session_t* session,
    const AimPointerEventV2* event) {
  return aim_graphics::dispatch_pointer_v2(session, event);
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_dispatch_key_v1(
    aim_graphics_session_t* session,
    const AimKeyEventV1* event) {
  return aim_graphics::dispatch_key_v1(session, event);
}

extern "C" AIM_EXPORT int32_t aim_graphics_session_pump_frame(
    aim_graphics_session_t* session, int64_t frame_time_nanos) {
  return aim_graphics::pump_frame(session, frame_time_nanos);
}

extern "C" AIM_EXPORT int32_t
aim_graphics_session_pump_main_looper(
    aim_graphics_session_t* session) {
  return aim_graphics::pump_main_looper(session);
}

extern "C" AIM_EXPORT int32_t
aim_graphics_session_wait_main_looper(
    aim_graphics_session_t* session, int32_t timeout_ms) {
  return aim_graphics::wait_main_looper(session, timeout_ms);
}

extern "C" AIM_EXPORT int32_t
aim_graphics_session_wake_main_looper(
    aim_graphics_session_t* session) {
  return aim_graphics::wake_main_looper(session);
}

// Legacy process-global entry points cannot identify an owner session. Keep
// their ABI failure explicit; all current callers must use an opaque handle.
extern "C" AIM_EXPORT int32_t aim_dispatch_pointer(
    uint32_t, float, float) {
  return AIM_STATUS_GRAPHICS_SESSION_INVALID;
}

extern "C" AIM_EXPORT int32_t aim_dispatch_pointer_v2(
    const AimPointerEventV2*) {
  return AIM_STATUS_GRAPHICS_SESSION_INVALID;
}

extern "C" AIM_EXPORT int32_t aim_dispatch_key_v1(
    const AimKeyEventV1*) {
  return AIM_STATUS_GRAPHICS_SESSION_INVALID;
}

extern "C" AIM_EXPORT int32_t aim_pump_framework_frame(int64_t) {
  return AIM_STATUS_GRAPHICS_SESSION_INVALID;
}
