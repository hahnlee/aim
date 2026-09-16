#include "runtime_graphics_session.h"

#include <pthread.h>

#include <cstdlib>
#include <cstdint>
#include <iostream>
#include <mutex>
#include <new>
#include <unordered_set>

#include "runtime_graphics_probe.h"
#include "runtime_graphics_state.h"
#include "darwin_android_platform.h"
#include "runtime_process_state.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

struct darwin_art_graphics_session_t {
  darwin_art_graphics::GraphicsState state;
  pthread_t owner_thread{};
  art::Thread* owner_art_thread = nullptr;
  // Opaque ALooper owned by owner_thread.  The FrameClock may signal this
  // token from its helper thread, but never enters ART or JNI there.
  void* owner_looper = nullptr;
  bool bound_to_process = false;
  bool closed = false;
  bool finalized = false;
};

namespace darwin_art_graphics {
namespace {

std::mutex g_session_mutex;
// Session ownership is carried by the opaque handle. There is deliberately
// no process-global active graphics state; the registry only protects handle
// validity during the short validation phase.
std::unordered_set<darwin_art_graphics_session_t*> g_sessions;

int32_t check_owner(const darwin_art_graphics_session_t* session) {
  if (session == nullptr) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  if (pthread_equal(session->owner_thread, pthread_self()) == 0 ||
      art::Thread::Current() != session->owner_art_thread ||
      session->owner_art_thread == nullptr) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  }
  return 0;
}

int32_t check_active_locked(const darwin_art_graphics_session_t* session) {
  if (session == nullptr || g_sessions.find(const_cast<darwin_art_graphics_session_t*>(session)) ==
                                g_sessions.end()) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  if (session->closed) return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  return 0;
}

}  // namespace

darwin_art_graphics_session_t* create_session() {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = new (std::nothrow) darwin_art_graphics_session_t;
  if (session == nullptr) return nullptr;
  session->owner_thread = pthread_self();
  g_sessions.insert(session);
  return session;
}

int32_t bind_session_for_process(void* context) {
  if (context == nullptr) return 0;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = static_cast<darwin_art_graphics_session_t*>(context);
  if (g_sessions.find(session) == g_sessions.end() ||
      pthread_equal(session->owner_thread, pthread_self()) == 0 ||
      session->closed || session->bound_to_process) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  session->bound_to_process = true;
  return 0;
}

int32_t bind_session_art_thread(art::Thread* thread) {
  if (thread == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  if (g_sessions.empty()) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  // The ART process binds exactly one handle. Find the one marked for this
  // owner thread without exposing it as a singleton active session.
  darwin_art_graphics_session_t* bound = nullptr;
  for (auto* candidate : g_sessions) {
    if (candidate->bound_to_process && !candidate->closed &&
        pthread_equal(candidate->owner_thread, pthread_self()) != 0) {
      bound = candidate;
      break;
    }
  }
  if (bound == nullptr ||
      pthread_equal(bound->owner_thread, pthread_self()) == 0) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  bound->owner_art_thread = thread;
  return 0;
}

int32_t close_session(darwin_art_graphics_session_t* session) {
  art::Thread* owner = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_session_mutex);
    if (session == nullptr || g_sessions.find(session) == g_sessions.end()) {
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
    }
    if (session->closed) return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
    if (pthread_equal(session->owner_thread, pthread_self()) == 0 ||
        (session->owner_art_thread != nullptr &&
         art::Thread::Current() != session->owner_art_thread)) {
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
    }
    owner = session->owner_art_thread;
    if (owner != nullptr && owner->GetState() != art::ThreadState::kNative) {
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
    }
    session->closed = true;
  }
  // Native graphics teardown is performed by run_shutdown while the ART
  // runtime is still in its canonical shutdown transaction. Closing the
  // opaque handle here only stops new calls; doing JNI/HWUI destruction in a
  // separate pre-shutdown transaction leaves Runtime::DestroyJavaVM with an
  // attached-thread state it cannot safely detach.
  (void)owner;
  return 0;
}

int32_t finalize_bound_session(GraphicsState* state) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  std::lock_guard<std::mutex> lock(g_session_mutex);
  for (auto* session : g_sessions) {
    if (&session->state != state) continue;
    if (!session->closed || !session->bound_to_process ||
        pthread_equal(session->owner_thread, pthread_self()) == 0 ||
        session->owner_art_thread == nullptr ||
        art::Thread::Current() != session->owner_art_thread) {
      return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
    }
    session->finalized = true;
    return 0;
  }
  return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
}

int32_t destroy_session(darwin_art_graphics_session_t* session) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
    if (session == nullptr || g_sessions.find(session) == g_sessions.end()) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  // The native shutdown transaction marks the session finalized before
  // DestroyJavaVM.  Rust Drop may run after ART is gone; in that path only
  // erase the opaque allocation and never call art::Thread::Current().
  if (session->finalized) {
    g_sessions.erase(session);
    delete session;
    return 0;
  }
  if (pthread_equal(session->owner_thread, pthread_self()) == 0 ||
      (session->owner_art_thread != nullptr &&
       art::Thread::Current() != session->owner_art_thread)) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD;
  }
  if (!session->closed) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_CLOSED;
  }
  g_sessions.erase(session);
  delete session;
  return 0;
}

GraphicsState* state_for_context(void* context) {
  std::lock_guard<std::mutex> lock(g_session_mutex);
  auto* session = static_cast<darwin_art_graphics_session_t*>(context);
  return session != nullptr && g_sessions.find(session) != g_sessions.end()
             ? &session->state
             : nullptr;
}

int32_t dispatch_pointer(darwin_art_graphics_session_t* session,
                         uint32_t action, float x, float y) {
  {
    std::lock_guard<std::mutex> lock(g_session_mutex);
    const int32_t active_status = check_active_locked(session);
    if (active_status != 0) return active_status;
    const int32_t owner_status = check_owner(session);
    if (owner_status != 0) return owner_status;
  }
  return dispatch_pointer(&session->state, action, x, y);
}

int32_t dispatch_pointer_v2(darwin_art_graphics_session_t* session,
                            const DarwinArtPointerEventV2* event) {
  if (session == nullptr || event == nullptr) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  std::lock_guard<std::mutex> lock(g_session_mutex);
  int32_t status = check_active_locked(session);
  if (status != 0) return status;
  status = check_owner(session);
  if (status != 0) return status;
  if (!session->bound_to_process || session->closed || session->finalized) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  }
  return dispatch_pointer_v2(&session->state, event);
}

int32_t dispatch_key_v1(darwin_art_graphics_session_t* session,
                        const DarwinArtKeyEventV1* event) {
  if (session == nullptr || event == nullptr) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  }
  std::lock_guard<std::mutex> lock(g_session_mutex);
  int32_t status = check_active_locked(session);
  if (status != 0) return status;
  status = check_owner(session);
  if (status != 0) return status;
  if (!session->bound_to_process || session->closed || session->finalized) {
    return DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED;
  }
  return dispatch_key_v1(&session->state, event);
}

int32_t pump_frame(darwin_art_graphics_session_t* session,
                   int64_t frame_time_nanos) {
  {
    std::lock_guard<std::mutex> lock(g_session_mutex);
    const int32_t active_status = check_active_locked(session);
    if (active_status != 0) return active_status;
    const int32_t owner_status = check_owner(session);
    if (owner_status != 0) return owner_status;
  }
  return pump_frame(&session->state, frame_time_nanos);
}

int32_t pump_main_looper(darwin_art_graphics_session_t* session) {
  {
    std::lock_guard<std::mutex> lock(g_session_mutex);
    const int32_t active_status = check_active_locked(session);
    if (active_status != 0) return active_status;
    const int32_t owner_status = check_owner(session);
    if (owner_status != 0) return owner_status;
    if (session->owner_looper == nullptr) {
      session->owner_looper = darwin_art_android_platform_prepare_current_looper();
    }
  }
  return pump_main_looper(&session->state);
}

int32_t wake_main_looper(darwin_art_graphics_session_t* session) {
  void* looper = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_session_mutex);
    const int32_t active_status = check_active_locked(session);
    if (active_status != 0) return active_status;
    looper = session->owner_looper;
  }
  if (looper == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY;
  // This is deliberately the only cross-thread operation: ALooper_wake is
  // safe to call without entering the owner thread's ART/JNI state.
  darwin_art_android_platform_wake_looper(looper);
  return 0;
}

int32_t wait_main_looper(darwin_art_graphics_session_t* session,
                         int32_t timeout_ms) {
  const int32_t owner_status = check_owner(session);
  if (owner_status != 0) return owner_status;
  return darwin_art_android_platform_wait_current_looper(timeout_ms);
}

}  // namespace darwin_art_graphics

// DisplayEventReceiver is Android-owned and runs while ActivityThread.main()
// holds the owner thread inside Looper.loop(). Keep this optional diagnostic
// hook outside the product ABI: it only resolves the bound session on that
// same owner thread and never lets the display helper enter ART/JNI.
extern "C" void darwin_art_graphics_debug_vsync(JNIEnv* env) {
  if (env == nullptr ||
      std::getenv("DARWIN_ART_DEBUG_VIEW_TEXT") == nullptr) {
    return;
  }
  jclass global_class = env->FindClass("android/view/WindowManagerGlobal");
  jmethodID get_instance =
      global_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(global_class, "getInstance",
                                   "()Landroid/view/WindowManagerGlobal;");
  jobject global = get_instance == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(global_class, get_instance);
  jfieldID views_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mViews", "Ljava/util/ArrayList;");
  jobject views = views_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, views_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID size = list_class == nullptr
                       ? nullptr
                       : env->GetMethodID(list_class, "size", "()I");
  jmethodID get = list_class == nullptr
                      ? nullptr
                      : env->GetMethodID(list_class, "get",
                                         "(I)Ljava/lang/Object;");
  jobject root = nullptr;
  if (views != nullptr && size != nullptr && get != nullptr &&
      !env->ExceptionCheck()) {
    const jint count = env->CallIntMethod(views, size);
    if (count > 0 && !env->ExceptionCheck()) {
      root = env->CallObjectMethod(views, get, count - 1);
    }
  }
  darwin_art_graphics::debug_view_text_state(env, root);
  env->DeleteLocalRef(root);
  env->DeleteLocalRef(list_class);
  env->DeleteLocalRef(views);
  env->DeleteLocalRef(global);
  env->DeleteLocalRef(global_class);
  if (env->ExceptionCheck()) env->ExceptionClear();
}

extern "C" DARWIN_ART_EXPORT darwin_art_graphics_session_t*
darwin_art_graphics_session_create() {
  return darwin_art_graphics::create_session();
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_close(
    darwin_art_graphics_session_t* session) {
  return darwin_art_graphics::close_session(session);
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_destroy(
    darwin_art_graphics_session_t* session) {
  return darwin_art_graphics::destroy_session(session);
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_dispatch_pointer(
    darwin_art_graphics_session_t* session, uint32_t action, float x, float y) {
  return darwin_art_graphics::dispatch_pointer(session, action, x, y);
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_dispatch_pointer_v2(
    darwin_art_graphics_session_t* session,
    const DarwinArtPointerEventV2* event) {
  return darwin_art_graphics::dispatch_pointer_v2(session, event);
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_dispatch_key_v1(
    darwin_art_graphics_session_t* session,
    const DarwinArtKeyEventV1* event) {
  return darwin_art_graphics::dispatch_key_v1(session, event);
}

extern "C" DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_pump_frame(
    darwin_art_graphics_session_t* session, int64_t frame_time_nanos) {
  return darwin_art_graphics::pump_frame(session, frame_time_nanos);
}

extern "C" DARWIN_ART_EXPORT int32_t
darwin_art_graphics_session_pump_main_looper(
    darwin_art_graphics_session_t* session) {
  return darwin_art_graphics::pump_main_looper(session);
}

extern "C" DARWIN_ART_EXPORT int32_t
darwin_art_graphics_session_wait_main_looper(
    darwin_art_graphics_session_t* session, int32_t timeout_ms) {
  return darwin_art_graphics::wait_main_looper(session, timeout_ms);
}

extern "C" DARWIN_ART_EXPORT int32_t
darwin_art_graphics_session_wake_main_looper(
    darwin_art_graphics_session_t* session) {
  return darwin_art_graphics::wake_main_looper(session);
}
