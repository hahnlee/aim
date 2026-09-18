#include "vsync_source.h"

#include <cstdlib>
#include <iostream>

#include "darwin_art/darwin_art.h"
#include "darwin_framework_natives.h"
#include "darwin_android_time.h"
#include "../../art/process_state.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

namespace darwin_art_graphics::display {

int32_t pump_frame(GraphicsState* state, int64_t frame_time_nanos) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (frame_time_nanos <= 0) {
    frame_time_nanos = darwin_art::AndroidUptimeNanos();
    if (frame_time_nanos <= 0) return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  }
  art::Thread* thread = darwin_art_process::owner_thread_for_callback();
  if (thread == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  if (art::Thread::Current() != thread ||
      thread->GetState() != art::ThreadState::kNative) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  }
  art::ScopedObjectAccess soa(thread);
  JNIEnv* env = thread->GetJniEnv();
  const int delivered =
      darwin_art::DispatchFrameworkPendingVsyncs(env, frame_time_nanos);
  if (env->ExceptionCheck()) {
    if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
      env->ExceptionDescribe();
    }
    env->ExceptionClear();
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  }
  if (delivered < 0) return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;

  // SurfaceFlinger publishes completed-target notifications. The persistent
  // AppKit display actor scans them out independently of this Android owner.
  // No fixture surface or ViewRoot is needed at the vsync boundary.
  return 0;
}

}  // namespace darwin_art_graphics::display
