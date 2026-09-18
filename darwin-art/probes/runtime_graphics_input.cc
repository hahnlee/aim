#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <pthread.h>
#include <string>
#include <time.h>
#include <vector>

#include "base/locks.h"
#include "darwin_android_time.h"
#include "darwin_android_platform.h"
#include "darwin_framework_natives.h"
#include "darwin_art/darwin_art.h"
#include "darwin_surface_bridge.h"
#include "runtime_hwui_probe.h"
#include "runtime_graphics_gpu.h"
#include "runtime_graphics_probe.h"
#include "runtime_graphics_probe_internal.h"
#include "graphics_fixture_state.h"
#include "fixture_motion_event_recycler.h"
#include "../runtime/art/process_state.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

#if defined(DARWIN_ART_REAL_GRAPHICS)
#define private public
#include "AnimationContext.h"
#include "renderthread/TimeLord.h"
#undef private
#endif

namespace darwin_art_graphics {

namespace {

bool MotionEventBridgeEnabled() {
  const char* mode = std::getenv("DARWIN_ART_INPUT_MODE");
  if (mode != nullptr) return std::string(mode) == "motion_event";
  return std::getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr;
}

bool CompatRippleOverlayEnabled() {
  // This overlay is a probe bridge for validating Metal frame cadence while
  // HWUI CanvasProperty replay is incomplete. Product APKs must use the
  // framework RippleDrawable attached to the actual hit View: the overlay has
  // no View bounds/state and therefore cannot preserve Android hotspot,
  // clipping, or WindowManager coordinate semantics.
  return std::getenv("DARWIN_ART_HWUI_COMPAT_RIPPLE_OVERLAY") != nullptr;
}

bool g_motion_event_archive_probe_done = false;
size_t g_debug_main_queue_budget = 0;

int64_t MonotonicNanos() {
  return darwin_art::AndroidUptimeNanos();
}

uint64_t CurrentThreadId() {
  uint64_t thread_id = 0;
  return pthread_threadid_np(nullptr, &thread_id) == 0 ? thread_id : 0;
}

// Process only messages that Android's main Looper could dispatch without
// blocking. The standalone host owns the ART/UI thread, so calling
// Looper.loop() would take over that thread permanently. Peeking at the queue
// first preserves MessageQueue.next() semantics (including sync barriers) and
// lets the host interleave due Handler work with native events and Metal
// frames just like Android's Looper, which keeps dispatching already-due
// messages instead of throttling the UI queue to the display refresh rate.
// The finite budget is only a host safety bound; native registrations are
// polled between Java messages, preserving Looper's message/native/message
// ordering while draining startup bursts promptly.
bool DispatchDueMainMessages(GraphicsState* state, JNIEnv* env,
                             size_t limit = 64) {
  if (state == nullptr || env == nullptr) return false;
  const bool trace_main_queue =
      std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE_TRACE") != nullptr;
  if (trace_main_queue) {
    std::cerr << "ART Android Looper trace: dispatch-due enter tid="
              << CurrentThreadId() << " limit=" << limit << "\n";
  }
  static bool logged_sync_barrier = false;
  static bool logged_native_poll_error = false;
  static bool initialized_debug_budget = false;
  if (!initialized_debug_budget &&
      std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
    initialized_debug_budget = true;
    g_debug_main_queue_budget = 96;
  }
  const auto initial_native_started = std::chrono::steady_clock::now();
  const int initial_native_dispatch =
      darwin_art_android_platform_poll_current_looper();
  if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
    const auto native_us =
        std::chrono::duration_cast<std::chrono::microseconds>(
            std::chrono::steady_clock::now() - initial_native_started)
            .count();
    if (native_us >= 100'000) {
      std::cerr << "DARWIN_ART slow-native-poll phase=initial result="
                << initial_native_dispatch << " tid=" << CurrentThreadId()
                << " elapsed_us=" << native_us << "\n";
    }
  }
  if (initial_native_dispatch < 0) {
    // Android's MessageQueue does not terminate the Java loop when the native
    // Looper reports POLL_ERROR (for example, an interrupted or concurrently
    // closed native registration). Keep Handler/Choreographer work alive and
    // let the next non-blocking poll recover.
    if (!logged_native_poll_error) {
      logged_native_poll_error = true;
      std::cerr << "ART Android Looper: native poll failed; continuing main "
                   "queue\n";
    }
  }
  auto& cache = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->main_looper;
  if (cache.looper_class == nullptr) {
    jclass looper = env->FindClass("android/os/Looper");
    jclass queue = env->FindClass("android/os/MessageQueue");
    jclass message = env->FindClass("android/os/Message");
    jclass handler = env->FindClass("android/os/Handler");
    jclass clock = env->FindClass("android/os/SystemClock");
    if (env->ExceptionCheck() || looper == nullptr || queue == nullptr ||
        message == nullptr || handler == nullptr || clock == nullptr) {
      if (env->ExceptionCheck()) env->ExceptionClear();
      if (looper != nullptr) env->DeleteLocalRef(looper);
      if (queue != nullptr) env->DeleteLocalRef(queue);
      if (message != nullptr) env->DeleteLocalRef(message);
      if (handler != nullptr) env->DeleteLocalRef(handler);
      if (clock != nullptr) env->DeleteLocalRef(clock);
      return false;
    }
    cache.looper_class = static_cast<jclass>(env->NewGlobalRef(looper));
    cache.queue_class = static_cast<jclass>(env->NewGlobalRef(queue));
    cache.message_class = static_cast<jclass>(env->NewGlobalRef(message));
    cache.handler_class = static_cast<jclass>(env->NewGlobalRef(handler));
    cache.clock_class = static_cast<jclass>(env->NewGlobalRef(clock));
    env->DeleteLocalRef(looper);
    env->DeleteLocalRef(queue);
    env->DeleteLocalRef(message);
    env->DeleteLocalRef(handler);
    env->DeleteLocalRef(clock);
    if (cache.looper_class == nullptr || cache.queue_class == nullptr ||
        cache.message_class == nullptr || cache.handler_class == nullptr ||
        cache.clock_class == nullptr || env->ExceptionCheck()) {
      if (env->ExceptionCheck()) env->ExceptionClear();
      return false;
    }
    cache.my_queue = env->GetStaticMethodID(
        cache.looper_class, "myQueue", "()Landroid/os/MessageQueue;");
    cache.queue_next = env->GetMethodID(cache.queue_class, "next",
                                        "()Landroid/os/Message;");
    cache.queue_messages = env->GetFieldID(
        cache.queue_class, "mMessages", "Landroid/os/Message;");
    cache.message_when = env->GetFieldID(cache.message_class, "when", "J");
    cache.message_what = env->GetFieldID(cache.message_class, "what", "I");
    cache.message_next = env->GetFieldID(
        cache.message_class, "next", "Landroid/os/Message;");
    cache.message_target = env->GetFieldID(
        cache.message_class, "target", "Landroid/os/Handler;");
    cache.message_callback = env->GetFieldID(
        cache.message_class, "callback", "Ljava/lang/Runnable;");
    cache.is_asynchronous = env->GetMethodID(cache.message_class,
                                             "isAsynchronous", "()Z");
    cache.recycle = env->GetMethodID(cache.message_class, "recycleUnchecked",
                                     "()V");
    cache.dispatch = env->GetMethodID(cache.handler_class, "dispatchMessage",
                                      "(Landroid/os/Message;)V");
    cache.uptime_millis = env->GetStaticMethodID(cache.clock_class,
                                                 "uptimeMillis", "()J");
    if (env->ExceptionCheck()) {
      env->ExceptionClear();
      return false;
    }
  }
  jclass looper_class = cache.looper_class;
  jclass queue_class = cache.queue_class;
  jclass message_class = cache.message_class;
  jclass handler_class = cache.handler_class;
  jclass clock_class = cache.clock_class;
  jmethodID my_queue = cache.my_queue;
  jmethodID queue_next = cache.queue_next;
  jfieldID queue_messages = cache.queue_messages;
  jfieldID message_when = cache.message_when;
  jfieldID message_what = cache.message_what;
  jfieldID message_next = cache.message_next;
  jfieldID message_target = cache.message_target;
  jfieldID message_callback = cache.message_callback;
  jmethodID is_asynchronous = cache.is_asynchronous;
  jmethodID recycle = cache.recycle;
  jmethodID dispatch = cache.dispatch;
  jmethodID uptime_millis = cache.uptime_millis;
  const bool resolved = my_queue != nullptr && queue_next != nullptr &&
                        queue_messages != nullptr && message_when != nullptr &&
                        message_what != nullptr && message_callback != nullptr &&
                        message_next != nullptr && message_target != nullptr &&
                        is_asynchronous != nullptr && recycle != nullptr &&
                        dispatch != nullptr && uptime_millis != nullptr &&
                        !env->ExceptionCheck();
  jobject queue = resolved
                      ? env->CallStaticObjectMethod(looper_class, my_queue)
                      : nullptr;
  bool ok = resolved && queue != nullptr && !env->ExceptionCheck();
  // Bound one host turn to work that was already ready when it began. This
  // keeps the temporary owner-thread pump responsive until the UI Looper is
  // moved to its Android-style continuously running thread.
  const jlong turn_cutoff =
      ok ? env->CallStaticLongMethod(clock_class, uptime_millis) : 0;
  ok = ok && !env->ExceptionCheck();
  for (size_t dispatched = 0; ok && dispatched < limit; ++dispatched) {
    jobject head = env->GetObjectField(queue, queue_messages);
    if (head == nullptr || env->ExceptionCheck()) {
      if (g_debug_main_queue_budget > 0 &&
          std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
        --g_debug_main_queue_budget;
        std::cerr << "ART Android Looper: queue empty\n";
      }
      if (head != nullptr) env->DeleteLocalRef(head);
      break;
    }
    jobject target = env->GetObjectField(head, message_target);
    if (g_debug_main_queue_budget > 0 &&
        std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
      --g_debug_main_queue_budget;
      std::cerr << "ART Android Looper: head what="
                << env->GetIntField(head, message_what)
                << " when=" << env->GetLongField(head, message_when)
                << " cutoff=" << turn_cutoff << " target=" << target;
      jobject callback = env->GetObjectField(head, message_callback);
      if (callback != nullptr && !env->ExceptionCheck()) {
        jclass callback_class = env->GetObjectClass(callback);
        jclass class_class = env->FindClass("java/lang/Class");
        jmethodID get_name =
            class_class == nullptr
                ? nullptr
                : env->GetMethodID(class_class, "getName", "()Ljava/lang/String;");
        jstring callback_name =
            callback_class == nullptr || get_name == nullptr
                ? nullptr
                : static_cast<jstring>(
                      env->CallObjectMethod(callback_class, get_name));
        if (callback_name != nullptr && !env->ExceptionCheck()) {
          const char* utf = env->GetStringUTFChars(callback_name, nullptr);
          if (utf != nullptr) {
            std::cerr << " callback=" << utf;
            env->ReleaseStringUTFChars(callback_name, utf);
          }
        }
        if (callback_name != nullptr) env->DeleteLocalRef(callback_name);
        if (class_class != nullptr) env->DeleteLocalRef(class_class);
        if (callback_class != nullptr) env->DeleteLocalRef(callback_class);
      }
      if (callback != nullptr) env->DeleteLocalRef(callback);
      std::cerr << "\n";
    }
    bool selectable = target != nullptr &&
                      env->GetLongField(head, message_when) <= turn_cutoff;
    if (target == nullptr && !logged_sync_barrier &&
        std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
      logged_sync_barrier = true;
      std::cerr << "ART Android Looper: pending synchronization barrier when="
                << env->GetLongField(head, message_when)
                << " cutoff=" << turn_cutoff
                << "\n";
    }
    if (target != nullptr) env->DeleteLocalRef(target);
    if (!selectable) {
      // A null target is a synchronization barrier. MessageQueue.next() may
      // pass it only when a due asynchronous message exists behind it.
      jobject candidate = env->GetObjectField(head, message_next);
      while (candidate != nullptr && !env->ExceptionCheck()) {
        jobject candidate_target = env->GetObjectField(candidate, message_target);
        const bool due_async = candidate_target != nullptr &&
                               env->CallBooleanMethod(candidate, is_asynchronous) ==
                                   JNI_TRUE &&
                               env->GetLongField(candidate, message_when) <=
                                   turn_cutoff;
        if (candidate_target != nullptr) env->DeleteLocalRef(candidate_target);
        if (due_async) {
          selectable = true;
          env->DeleteLocalRef(candidate);
          break;
        }
        jobject next = env->GetObjectField(candidate, message_next);
        env->DeleteLocalRef(candidate);
        candidate = next;
      }
    }
    env->DeleteLocalRef(head);
    if (!selectable || env->ExceptionCheck()) break;

    if (trace_main_queue) {
      std::cerr << "ART Android Looper trace: queue-next enter tid="
                << CurrentThreadId() << "\n";
    }
    jobject message = env->CallObjectMethod(queue, queue_next);
    if (trace_main_queue) {
      std::cerr << "ART Android Looper trace: queue-next exit tid="
                << CurrentThreadId() << " message=" << message << "\n";
    }
    if (message == nullptr || env->ExceptionCheck()) {
      if (message != nullptr) env->DeleteLocalRef(message);
      ok = false;
      break;
    }
    jobject dispatch_target = env->GetObjectField(message, message_target);
    if (dispatch_target == nullptr || env->ExceptionCheck()) {
      if (dispatch_target != nullptr) env->DeleteLocalRef(dispatch_target);
      env->DeleteLocalRef(message);
      ok = false;
      break;
    }
    const int32_t dispatched_what = env->GetIntField(message, message_what);
    const auto message_started = std::chrono::steady_clock::now();
    if (trace_main_queue) {
      std::cerr << "ART Android Looper trace: dispatch enter tid="
                << CurrentThreadId() << " what=" << dispatched_what << "\n";
    }
    env->CallVoidMethod(dispatch_target, dispatch, message);
    if (trace_main_queue) {
      std::cerr << "ART Android Looper trace: dispatch exit tid="
                << CurrentThreadId() << " what=" << dispatched_what
                << " exception=" << (env->ExceptionCheck() ? 1 : 0) << "\n";
    }
    if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
      const auto message_us =
          std::chrono::duration_cast<std::chrono::microseconds>(
              std::chrono::steady_clock::now() - message_started)
              .count();
      if (message_us >= 100'000) {
        std::cerr << "DARWIN_ART slow-looper-message what="
                  << dispatched_what << " elapsed_us=" << message_us
                  << "\n";
      }
    }
    if (!env->ExceptionCheck()) env->CallVoidMethod(message, recycle);
    env->DeleteLocalRef(dispatch_target);
    env->DeleteLocalRef(message);
    ok = !env->ExceptionCheck();
    const auto native_started = std::chrono::steady_clock::now();
    const int native_result = darwin_art_android_platform_poll_current_looper();
    if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
      const auto native_us =
          std::chrono::duration_cast<std::chrono::microseconds>(
              std::chrono::steady_clock::now() - native_started)
              .count();
      if (native_us >= 100'000) {
        std::cerr << "DARWIN_ART slow-native-poll phase=message result="
                  << native_result << " tid=" << CurrentThreadId()
                  << " elapsed_us=" << native_us << "\n";
      }
    }
    if (ok && native_result < 0 && !logged_native_poll_error) {
      logged_native_poll_error = true;
      std::cerr << "ART Android Looper: native poll failed; continuing main "
                   "queue\n";
    }
  }
  if (queue != nullptr) env->DeleteLocalRef(queue);
  if (ok && darwin_art_android_platform_poll_current_looper() < 0 &&
      !logged_native_poll_error) {
    logged_native_poll_error = true;
    std::cerr << "ART Android Looper: native poll failed; continuing main "
                 "queue\n";
  }
  if (trace_main_queue) {
    std::cerr << "ART Android Looper trace: dispatch-due exit tid="
              << CurrentThreadId() << " ok=" << (ok ? 1 : 0) << "\n";
  }
  return ok;
}

bool ProgressFixtureOwner(void* opaque, JNIEnv* env) {
  auto* state = static_cast<GraphicsState*>(opaque);
  if (!DispatchDueMainMessages(state, env)) return false;
  auto* fixture = darwin_art_graphics_fixture::GetGraphicsFixtureState(state);
  return fixture != nullptr && !fixture->retiring.load(std::memory_order_acquire);
}

// Native compositor callbacks do not have a Java caller frame, so FindClass
// only sees the boot class loader. Resolve APK/runtime helper classes through
// the process thread's context loader instead.
jclass LoadContextClass(JNIEnv* env, const char* binary_name) {
  if (env == nullptr || binary_name == nullptr) return nullptr;
  jclass thread_class = env->FindClass("java/lang/Thread");
  jmethodID current_thread = thread_class == nullptr
                                 ? nullptr
                                 : env->GetStaticMethodID(
                                       thread_class, "currentThread",
                                       "()Ljava/lang/Thread;");
  jobject thread = current_thread == nullptr
                       ? nullptr
                       : env->CallStaticObjectMethod(thread_class, current_thread);
  jmethodID get_loader = thread_class == nullptr
                             ? nullptr
                             : env->GetMethodID(thread_class,
                                                "getContextClassLoader",
                                                "()Ljava/lang/ClassLoader;");
  jobject loader = get_loader == nullptr
                       ? nullptr
                       : env->CallObjectMethod(thread, get_loader);
  jclass loader_class = loader == nullptr ? nullptr : env->GetObjectClass(loader);
  jmethodID load_class = loader_class == nullptr
                             ? nullptr
                             : env->GetMethodID(loader_class, "loadClass",
                                                "(Ljava/lang/String;)Ljava/lang/Class;");
  jstring name = env->NewStringUTF(binary_name);
  jclass loaded = load_class == nullptr
                      ? nullptr
                      : static_cast<jclass>(
                            env->CallObjectMethod(loader, load_class, name));
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    loaded = nullptr;
  }
  if (name != nullptr) env->DeleteLocalRef(name);
  if (loader_class != nullptr) env->DeleteLocalRef(loader_class);
  if (loader != nullptr) env->DeleteLocalRef(loader);
  if (thread != nullptr) env->DeleteLocalRef(thread);
  if (thread_class != nullptr) env->DeleteLocalRef(thread_class);
  return loaded;
}

void DebugWindowManagerViews(JNIEnv* env) {
  static bool dumped = false;
  if (dumped || std::getenv("DARWIN_ART_DEBUG_WINDOW_LAYERS") == nullptr) return;
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
  jfieldID params_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mParams", "Ljava/util/ArrayList;");
  jobject views = global == nullptr || views_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, views_field);
  jobject params = global == nullptr || params_field == nullptr
                       ? nullptr
                       : env->GetObjectField(global, params_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID size = list_class == nullptr
                       ? nullptr
                       : env->GetMethodID(list_class, "size", "()I");
  jmethodID get = list_class == nullptr
                      ? nullptr
                      : env->GetMethodID(list_class, "get", "(I)Ljava/lang/Object;");
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_width = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getWidth", "()I");
  jmethodID get_height = view_class == nullptr
                             ? nullptr
                             : env->GetMethodID(view_class, "getHeight", "()I");
  jmethodID get_measured_width =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getMeasuredWidth", "()I");
  jmethodID get_measured_height =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getMeasuredHeight", "()I");
  jclass params_class = env->FindClass("android/view/WindowManager$LayoutParams");
  jfieldID x_field = params_class == nullptr
                         ? nullptr
                         : env->GetFieldID(params_class, "x", "I");
  jfieldID y_field = params_class == nullptr
                         ? nullptr
                         : env->GetFieldID(params_class, "y", "I");
  jfieldID type_field = params_class == nullptr
                            ? nullptr
                            : env->GetFieldID(params_class, "type", "I");
  jfieldID gravity_field = params_class == nullptr
                               ? nullptr
                               : env->GetFieldID(params_class, "gravity", "I");
  jfieldID width_field = params_class == nullptr
                             ? nullptr
                             : env->GetFieldID(params_class, "width", "I");
  jfieldID height_field = params_class == nullptr
                              ? nullptr
                              : env->GetFieldID(params_class, "height", "I");
  if (!env->ExceptionCheck() && views != nullptr && params != nullptr &&
      size != nullptr && get != nullptr) {
    const jint count = env->CallIntMethod(views, size);
    if (count > 0 && !env->ExceptionCheck()) {
      bool any_sized = false;
      std::cerr << "ART Android windows: count=" << count << "\n";
      for (jint index = 0; index < count; ++index) {
        jobject window_view = env->CallObjectMethod(views, get, index);
        jobject window_params = env->CallObjectMethod(params, get, index);
        if (window_view != nullptr && window_params != nullptr &&
            !env->ExceptionCheck()) {
          const jint window_width = env->CallIntMethod(window_view, get_width);
          const jint window_height = env->CallIntMethod(window_view, get_height);
          any_sized = any_sized || (window_width > 0 && window_height > 0);
          std::cerr << "ART Android window[" << index << "] view=" << window_view
                    << " size=" << window_width
                    << "x" << window_height
                    << " measured="
                    << env->CallIntMethod(window_view, get_measured_width) << "x"
                    << env->CallIntMethod(window_view, get_measured_height)
                    << " pos=" << env->GetIntField(window_params, x_field) << ","
                    << env->GetIntField(window_params, y_field)
                    << " layout=" << env->GetIntField(window_params, width_field)
                    << "x" << env->GetIntField(window_params, height_field)
                    << " gravity="
                    << env->GetIntField(window_params, gravity_field) << "\n";
        }
        if (window_params != nullptr) env->DeleteLocalRef(window_params);
        if (window_view != nullptr) env->DeleteLocalRef(window_view);
      }
      dumped = any_sized;
    }
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (params_class != nullptr) env->DeleteLocalRef(params_class);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  if (list_class != nullptr) env->DeleteLocalRef(list_class);
  if (params != nullptr) env->DeleteLocalRef(params);
  if (views != nullptr) env->DeleteLocalRef(views);
  if (global != nullptr) env->DeleteLocalRef(global);
  if (global_class != nullptr) env->DeleteLocalRef(global_class);
}

bool sync_interactive_surface_size(GraphicsState* state, JNIEnv* env) {
  if (state == nullptr || env == nullptr) {
    return true;
  }
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (fixture->gpu_surface == nullptr || fixture->interactive_root == nullptr) {
    return true;
  }
  uint32_t width = 0;
  uint32_t height = 0;
  if (!darwin_art_surface_get_size(fixture->gpu_surface, &width, &height) ||
      width == 0 || height == 0) {
    return false;
  }
  if (fixture->interactive_width == static_cast<jint>(width) &&
      fixture->interactive_height == static_cast<jint>(height)) {
    return true;
  }
  if (std::getenv("DARWIN_ART_DEBUG_RESIZE") != nullptr) {
    std::cerr << "ART Android resize: " << fixture->interactive_width << "x"
              << fixture->interactive_height << " -> " << width << "x"
              << height << "\n";
  }
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded = false;
  const jboolean rendered = present_content(
      state, env, nullptr, fixture->interactive_root, static_cast<jint>(width),
      static_cast<jint>(height));
  if (rendered == JNI_TRUE) {
    fixture->interactive_width = static_cast<jint>(width);
    fixture->interactive_height = static_cast<jint>(height);
    return true;
  }
  return false;
}

jclass LoadProbeAnimationHost(JNIEnv* env) {
  return LoadContextClass(env, "dev.darwinart.probe.ProbeAnimationHost");
}

void ClearPointerDispatchRoot(GraphicsState* state, JNIEnv* env) {
  if (state == nullptr || env == nullptr) return;
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (fixture->pointer_dispatch_root != nullptr) {
    env->DeleteGlobalRef(fixture->pointer_dispatch_root);
    fixture->pointer_dispatch_root = nullptr;
  }
  if (fixture->pointer_dispatch_view_root != nullptr) {
    env->DeleteGlobalRef(fixture->pointer_dispatch_view_root);
    fixture->pointer_dispatch_view_root = nullptr;
  }
  fixture->pointer_dispatch_offset_x = 0.0f;
  fixture->pointer_dispatch_offset_y = 0.0f;
  fixture->pointer_dispatch_is_window = false;
  fixture->pointer_dispatch_outside_only = false;
}

void DebugViewTextState(JNIEnv* env, jobject root) {
  if (env == nullptr || root == nullptr ||
      std::getenv("DARWIN_ART_DEBUG_VIEW_TEXT") == nullptr) {
    return;
  }
  static int remaining_snapshots = 160;
  static auto last_snapshot = std::chrono::steady_clock::time_point{};
  const auto now = std::chrono::steady_clock::now();
  if (remaining_snapshots <= 0 ||
      (last_snapshot.time_since_epoch().count() != 0 &&
       now - last_snapshot < std::chrono::milliseconds(250))) {
    return;
  }
  --remaining_snapshots;
  last_snapshot = now;
  std::cerr << "ART Android View text snapshot\n";
  jclass view_class = env->FindClass("android/view/View");
  jclass group_class = env->FindClass("android/view/ViewGroup");
  jclass text_class = env->FindClass("android/widget/TextView");
  jclass sequence_class = env->FindClass("java/lang/CharSequence");
  jmethodID get_id = view_class == nullptr
                         ? nullptr
                         : env->GetMethodID(view_class, "getId", "()I");
  jmethodID get_left = view_class == nullptr
                           ? nullptr
                           : env->GetMethodID(view_class, "getLeft", "()I");
  jmethodID get_top = view_class == nullptr
                          ? nullptr
                          : env->GetMethodID(view_class, "getTop", "()I");
  jmethodID get_right = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getRight", "()I");
  jmethodID get_bottom = view_class == nullptr
                             ? nullptr
                             : env->GetMethodID(view_class, "getBottom", "()I");
  jmethodID get_visibility =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getVisibility", "()I");
  jmethodID get_alpha = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getAlpha", "()F");
  jmethodID get_scale_x =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getScaleX", "()F");
  jmethodID get_scale_y =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getScaleY", "()F");
  jmethodID get_translation_x =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getTranslationX", "()F");
  jmethodID get_translation_y =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getTranslationY", "()F");
  jmethodID get_layout_direction =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getLayoutDirection", "()I");
  jmethodID get_text_alignment =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "getTextAlignment", "()I");
  jmethodID get_child_count =
      group_class == nullptr
          ? nullptr
          : env->GetMethodID(group_class, "getChildCount", "()I");
  jmethodID get_child_at =
      group_class == nullptr
          ? nullptr
          : env->GetMethodID(group_class, "getChildAt", "(I)Landroid/view/View;");
  jmethodID get_text = text_class == nullptr
                           ? nullptr
                           : env->GetMethodID(text_class, "getText",
                                              "()Ljava/lang/CharSequence;");
  jmethodID get_text_size =
      text_class == nullptr
          ? nullptr
          : env->GetMethodID(text_class, "getTextSize", "()F");
  jmethodID get_text_color =
      text_class == nullptr
          ? nullptr
          : env->GetMethodID(text_class, "getCurrentTextColor", "()I");
  jmethodID get_gravity = text_class == nullptr
                              ? nullptr
                              : env->GetMethodID(text_class, "getGravity", "()I");
  jmethodID to_string =
      sequence_class == nullptr
          ? nullptr
          : env->GetMethodID(sequence_class, "toString", "()Ljava/lang/String;");
  const bool ready = view_class != nullptr && group_class != nullptr &&
                     text_class != nullptr && sequence_class != nullptr &&
                     get_id != nullptr && get_left != nullptr &&
                     get_top != nullptr && get_right != nullptr &&
                     get_bottom != nullptr && get_visibility != nullptr &&
                     get_alpha != nullptr && get_scale_x != nullptr &&
                     get_scale_y != nullptr && get_translation_x != nullptr &&
                     get_translation_y != nullptr &&
                     get_layout_direction != nullptr &&
                     get_text_alignment != nullptr && get_child_count != nullptr &&
                     get_child_at != nullptr && get_text != nullptr &&
                     get_text_size != nullptr && get_text_color != nullptr &&
                     get_gravity != nullptr && to_string != nullptr &&
                     !env->ExceptionCheck();
  std::vector<jobject> pending;
  if (ready) pending.push_back(env->NewLocalRef(root));
  size_t visited = 0;
  while (!pending.empty() && visited++ < 256 && !env->ExceptionCheck()) {
    jobject view = pending.back();
    pending.pop_back();
    if (env->IsInstanceOf(view, text_class)) {
      jobject sequence = env->CallObjectMethod(view, get_text);
      jstring text = sequence == nullptr
                         ? nullptr
                         : static_cast<jstring>(
                               env->CallObjectMethod(sequence, to_string));
      const char* utf = text == nullptr
                            ? nullptr
                            : env->GetStringUTFChars(text, nullptr);
      std::cerr << "ART Android View text id=0x" << std::hex
                << env->CallIntMethod(view, get_id) << std::dec
                << " bounds=" << env->CallIntMethod(view, get_left) << ","
                << env->CallIntMethod(view, get_top) << "-"
                << env->CallIntMethod(view, get_right) << ","
                << env->CallIntMethod(view, get_bottom)
                << " visibility=" << env->CallIntMethod(view, get_visibility)
                << " alpha=" << env->CallFloatMethod(view, get_alpha)
                << " scale=" << env->CallFloatMethod(view, get_scale_x) << ","
                << env->CallFloatMethod(view, get_scale_y)
                << " translation="
                << env->CallFloatMethod(view, get_translation_x) << ","
                << env->CallFloatMethod(view, get_translation_y)
                << " layout_direction="
                << env->CallIntMethod(view, get_layout_direction)
                << " text_alignment="
                << env->CallIntMethod(view, get_text_alignment)
                << " gravity=0x" << std::hex
                << env->CallIntMethod(view, get_gravity) << std::dec
                << " text_size=" << env->CallFloatMethod(view, get_text_size)
                << " color=0x" << std::hex
                << env->CallIntMethod(view, get_text_color) << std::dec
                << " text="
                << (utf == nullptr ? "(null)" : utf) << "\n";
      if (utf != nullptr) env->ReleaseStringUTFChars(text, utf);
      if (text != nullptr) env->DeleteLocalRef(text);
      if (sequence != nullptr) env->DeleteLocalRef(sequence);
    }
    if (env->IsInstanceOf(view, group_class)) {
      const jint count = env->CallIntMethod(view, get_child_count);
      for (jint index = 0; index < count && !env->ExceptionCheck(); ++index) {
        jobject child = env->CallObjectMethod(view, get_child_at, index);
        if (child != nullptr) pending.push_back(child);
      }
    }
    env->DeleteLocalRef(view);
  }
  for (jobject view : pending) env->DeleteLocalRef(view);
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (sequence_class != nullptr) env->DeleteLocalRef(sequence_class);
  if (text_class != nullptr) env->DeleteLocalRef(text_class);
  if (group_class != nullptr) env->DeleteLocalRef(group_class);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
}

bool SelectPointerDispatchRoot(GraphicsState* state, JNIEnv* env,
                               jobject main_root, jfloat x, jfloat y) {
  ClearPointerDispatchRoot(state, env);
  jobject selected = nullptr;
  jobject selected_view_root = nullptr;
  jint selected_x = 0;
  jint selected_y = 0;
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
  jfieldID params_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mParams", "Ljava/util/ArrayList;");
  jfieldID roots_field =
      global_class == nullptr
          ? nullptr
          : env->GetFieldID(global_class, "mRoots", "Ljava/util/ArrayList;");
  jobject views = global == nullptr || views_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, views_field);
  jobject params = global == nullptr || params_field == nullptr
                       ? nullptr
                       : env->GetObjectField(global, params_field);
  jobject roots = global == nullptr || roots_field == nullptr
                      ? nullptr
                      : env->GetObjectField(global, roots_field);
  jclass list_class = env->FindClass("java/util/ArrayList");
  jmethodID size = list_class == nullptr
                       ? nullptr
                       : env->GetMethodID(list_class, "size", "()I");
  jmethodID get = list_class == nullptr
                      ? nullptr
                      : env->GetMethodID(list_class, "get", "(I)Ljava/lang/Object;");
  jclass view_class = env->FindClass("android/view/View");
  jmethodID get_width = view_class == nullptr
                            ? nullptr
                            : env->GetMethodID(view_class, "getWidth", "()I");
  jmethodID get_height = view_class == nullptr
                             ? nullptr
                             : env->GetMethodID(view_class, "getHeight", "()I");
  jclass params_class = env->FindClass("android/view/WindowManager$LayoutParams");
  jfieldID x_field = params_class == nullptr
                         ? nullptr
                         : env->GetFieldID(params_class, "x", "I");
  jfieldID y_field = params_class == nullptr
                         ? nullptr
                         : env->GetFieldID(params_class, "y", "I");
  jfieldID type_field = params_class == nullptr
                            ? nullptr
                            : env->GetFieldID(params_class, "type", "I");
  jfieldID flags_field = params_class == nullptr
                             ? nullptr
                             : env->GetFieldID(params_class, "flags", "I");
  jfieldID token_field =
      params_class == nullptr
          ? nullptr
          : env->GetFieldID(params_class, "token", "Landroid/os/IBinder;");
  jclass view_root_class = env->FindClass("android/view/ViewRootImpl");
  jfieldID window_frame_field =
      view_root_class == nullptr
          ? nullptr
          : env->GetFieldID(view_root_class, "mWinFrame", "Landroid/graphics/Rect;");
  jclass rect_class = env->FindClass("android/graphics/Rect");
  jfieldID rect_left_field =
      rect_class == nullptr ? nullptr : env->GetFieldID(rect_class, "left", "I");
  jfieldID rect_top_field =
      rect_class == nullptr
          ? nullptr
          : env->GetFieldID(rect_class, "top", "I");
  jobject current_activity_token = nullptr;
  if (!env->ExceptionCheck() && views != nullptr && params != nullptr &&
      roots != nullptr &&
      size != nullptr && get != nullptr && get_width != nullptr &&
      get_height != nullptr && x_field != nullptr && y_field != nullptr &&
      type_field != nullptr && flags_field != nullptr && token_field != nullptr) {
    const jint count = env->CallIntMethod(views, size);
    for (jint index = 0;
         index < count && current_activity_token == nullptr; ++index) {
      jobject layer = env->CallObjectMethod(views, get, index);
      jobject layout_params = env->CallObjectMethod(params, get, index);
      if (layer != nullptr && layout_params != nullptr &&
          env->IsSameObject(layer, main_root) == JNI_TRUE) {
        current_activity_token =
            env->GetObjectField(layout_params, token_field);
      }
      if (layout_params != nullptr) env->DeleteLocalRef(layout_params);
      if (layer != nullptr) env->DeleteLocalRef(layer);
    }
    for (jint index = count - 1; index >= 0 && selected == nullptr; --index) {
      jobject layer = env->CallObjectMethod(views, get, index);
      jobject layout_params = env->CallObjectMethod(params, get, index);
      jobject layer_root = env->CallObjectMethod(roots, get, index);
      if (layer != nullptr && layout_params != nullptr && !env->ExceptionCheck()) {
        // LayoutParams.x/y are the client's requested gravity offsets. WMS
        // may fit a popup elsewhere on screen; ViewRootImpl retains that
        // resolved frame in mWinFrame and InputDispatcher uses
        // the resolved input window, not the original request.
        jobject window_frame =
            layer_root != nullptr && window_frame_field != nullptr
                ? env->GetObjectField(layer_root, window_frame_field)
                : nullptr;
        const jint left = window_frame != nullptr && rect_left_field != nullptr
                              ? env->GetIntField(window_frame, rect_left_field)
                              : env->GetIntField(layout_params, x_field);
        const jint top = window_frame != nullptr && rect_top_field != nullptr
                             ? env->GetIntField(window_frame, rect_top_field)
                             : env->GetIntField(layout_params, y_field);
        const jint type = env->GetIntField(layout_params, type_field);
        const jint flags = env->GetIntField(layout_params, flags_field);
        const jint width = env->CallIntMethod(layer, get_width);
        const jint height = env->CallIntMethod(layer, get_height);
        const bool current_activity =
            env->IsSameObject(layer, main_root) == JNI_TRUE;
        jobject layer_token = env->GetObjectField(layout_params, token_field);
        const bool current_activity_token_owner =
            current_activity_token != nullptr && layer_token != nullptr &&
            env->IsSameObject(layer_token, current_activity_token) == JNI_TRUE;
        // WindowManagerGlobal retains the outgoing Activity root until its
        // teardown traversal completes. Android InputDispatcher targets the
        // focused current application token, not whichever stale base
        // application entry happens to be last in mViews. Dialog windows may
        // also use TYPE_APPLICATION, so retain every layer owned by the
        // foreground Activity token in addition to ordinary sub-windows.
        const bool eligible = current_activity || current_activity_token_owner ||
                              type >= 1000;
        const bool inside = x >= left && y >= top && x < left + width &&
                            y < top + height;
        constexpr jint kFlagWatchOutsideTouch = 0x00040000;
        if (std::getenv("DARWIN_ART_DEBUG_POINTER") != nullptr) {
          std::cerr << "ART Android input window index=" << index
                    << " type=" << type << " current=" << current_activity
                    << " token_owner=" << current_activity_token_owner
                    << " bounds=" << left << "," << top << "-"
                    << (left + width) << "," << (top + height)
                    << " flags=0x" << std::hex << flags << std::dec
                    << " eligible=" << eligible << "\n";
        }
        if (eligible && inside) {
          selected = env->NewLocalRef(layer);
          selected_view_root = env->NewLocalRef(layer_root);
          selected_x = left;
          selected_y = top;
        } else if (eligible && type >= 1000 &&
                   (flags & kFlagWatchOutsideTouch) != 0) {
          // Android InputDispatcher sends ACTION_OUTSIDE to the topmost
          // watched sub-window before a modal popup is dismissed. Keep that
          // event on the popup's InputChannel instead of leaking the original
          // DOWN into the Activity underneath it.
          selected = env->NewLocalRef(layer);
          selected_view_root = env->NewLocalRef(layer_root);
          selected_x = left;
          selected_y = top;
          darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only = true;
        }
        if (layer_token != nullptr) env->DeleteLocalRef(layer_token);
        if (window_frame != nullptr) env->DeleteLocalRef(window_frame);
      }
      if (layer_root != nullptr) env->DeleteLocalRef(layer_root);
      if (layout_params != nullptr) env->DeleteLocalRef(layout_params);
      if (layer != nullptr) env->DeleteLocalRef(layer);
    }
  }
  if (env->ExceptionCheck()) env->ExceptionClear();
  if (current_activity_token != nullptr)
    env->DeleteLocalRef(current_activity_token);
  if (params_class != nullptr) env->DeleteLocalRef(params_class);
  if (view_root_class != nullptr) env->DeleteLocalRef(view_root_class);
  if (rect_class != nullptr) env->DeleteLocalRef(rect_class);
  if (view_class != nullptr) env->DeleteLocalRef(view_class);
  if (list_class != nullptr) env->DeleteLocalRef(list_class);
  if (params != nullptr) env->DeleteLocalRef(params);
  if (roots != nullptr) env->DeleteLocalRef(roots);
  if (views != nullptr) env->DeleteLocalRef(views);
  if (global != nullptr) env->DeleteLocalRef(global);
  if (global_class != nullptr) env->DeleteLocalRef(global_class);
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  jobject dispatch_root = selected == nullptr ? main_root : selected;
  fixture->pointer_dispatch_root = env->NewGlobalRef(dispatch_root);
  jobject dispatch_view_root = selected_view_root == nullptr
                                   ? fixture->interactive_view_root
                                   : selected_view_root;
  if (dispatch_view_root != nullptr) {
    fixture->pointer_dispatch_view_root = env->NewGlobalRef(dispatch_view_root);
  }
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_x = static_cast<jfloat>(selected_x);
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_y = static_cast<jfloat>(selected_y);
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_is_window = selected != nullptr;
  if (selected != nullptr) env->DeleteLocalRef(selected);
  if (selected_view_root != nullptr) env->DeleteLocalRef(selected_view_root);
  return darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_root != nullptr && !env->ExceptionCheck();
}

bool RefreshFocusedWindowRoot(GraphicsState* state, JNIEnv* env) {
  if (state == nullptr || env == nullptr) {
    return false;
  }
  auto* fixture = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state);
  if (fixture->interactive_view_root == nullptr) return false;
  jobject candidate = env->NewLocalRef(fixture->interactive_view_root);
  if (candidate == nullptr) return false;
  const bool unchanged = fixture->focused_view_root != nullptr &&
                         env->IsSameObject(fixture->focused_view_root,
                                           candidate) == JNI_TRUE;
  if (unchanged) {
    env->DeleteLocalRef(candidate);
    return true;
  }
  jobject candidate_global = env->NewGlobalRef(candidate);
  if (candidate_global == nullptr || env->ExceptionCheck()) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->DeleteLocalRef(candidate);
    return fixture->focused_view_root != nullptr;
  }
  // Probe the new receiver before retiring the old one. A popup ViewRoot can
  // be visible in WindowManagerGlobal for one owner turn before its
  // WindowInputEventReceiver has finished native initialization. Publishing
  // the candidate only succeeds with a ready receiver. SetFocusedInputChannel
  // cancels the previous channel; clearing its Java focus afterward cannot
  // clear the newly published channel.
  const bool focused =
      darwin_art_graphics_fixture::set_view_root_focus(env, candidate, true);
  if (!focused || env->ExceptionCheck()) {
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->DeleteGlobalRef(candidate_global);
    env->DeleteLocalRef(candidate);
    return fixture->focused_view_root != nullptr;
  }
  jobject previous_root = fixture->focused_view_root;
  fixture->focused_view_root = candidate_global;
  if (previous_root != nullptr) {
    darwin_art_graphics_fixture::set_view_root_focus(env, previous_root, false);
    if (env->ExceptionCheck()) env->ExceptionClear();
    env->DeleteGlobalRef(previous_root);
  }
  // InputDispatcher publishes focus before it releases the next key/pointer
  // packet. onFocusEvent posts ViewRootImpl's focus message, so drain that
  // owner-Looper work here before dispatching the event selected below.
  if (fixture->focused_view_root != nullptr && !env->ExceptionCheck() &&
      !DispatchDueMainMessages(state, env)) {
    // A posted focus message may fail during owner progress. Keep that
    // failure visible to the caller; submitted focus is not completed work.
    env->DeleteLocalRef(candidate);
    return false;
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android focused window changed="
              << (fixture->focused_view_root != nullptr ? 1 : 0)
              << " root=" << candidate << "\n";
  }
  env->DeleteLocalRef(candidate);
  return fixture->focused_view_root != nullptr && !env->ExceptionCheck() &&
         !fixture->retiring.load(std::memory_order_acquire);
}

int32_t dispatch_motion_event(GraphicsState* state, JNIEnv* env, jobject root,
                              uint32_t action, float x, float y,
                              uint32_t pointer_flags,
                              uint64_t event_time_hint,
                              uint64_t down_time_hint) {
  if (state == nullptr || env == nullptr || root == nullptr) return 72;
  if (action > 3u || !std::isfinite(x) || !std::isfinite(y)) return 71;
  const bool down = action == 0u;
  const bool terminal = action == 1u || action == 3u;
  if (!down && !state->pointer_stream_active) return 78;
  if (!down && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only) {
    // ACTION_OUTSIDE is a one-shot Android window event. AppKit still closes
    // its physical pointer stream with UP/CANCEL, so consume that tail rather
    // than synthesizing an invalid gesture for the popup or Activity.
    if (terminal) {
      state->pointer_stream_active = false;
      state->pointer_down_time_nanos = 0;
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate = false;
      ClearPointerDispatchRoot(state, env);
    }
    return 0;
  }
  const int64_t event_time_nanos = event_time_hint > 0
                                       ? static_cast<int64_t>(event_time_hint)
                                       : MonotonicNanos();
  if (event_time_nanos <= 0) return 79;
  if (down) {
    state->pointer_down_time_nanos = down_time_hint > 0
                                         ? static_cast<int64_t>(down_time_hint)
                                         : event_time_nanos;
  }
  const int64_t down_time_nanos = state->pointer_down_time_nanos;
  if (down_time_nanos <= 0 || down_time_nanos > event_time_nanos) return 79;

  if (down && !SelectPointerDispatchRoot(state, env, root, x, y)) {
    env->ExceptionClear();
    return 82;
  }
  if (down && !RefreshFocusedWindowRoot(state, env)) {
    env->ExceptionClear();
    ClearPointerDispatchRoot(state, env);
    return 84;
  }

  jclass motion_event_class = env->FindClass("android/view/MotionEvent");
  jclass pointer_properties_class =
      env->FindClass("android/view/MotionEvent$PointerProperties");
  jclass pointer_coords_class =
      env->FindClass("android/view/MotionEvent$PointerCoords");
  jmethodID properties_constructor =
      pointer_properties_class == nullptr
          ? nullptr
          : env->GetMethodID(pointer_properties_class, "<init>", "()V");
  jmethodID coords_constructor =
      pointer_coords_class == nullptr
          ? nullptr
          : env->GetMethodID(pointer_coords_class, "<init>", "()V");
  jfieldID pointer_id_field =
      pointer_properties_class == nullptr
          ? nullptr
          : env->GetFieldID(pointer_properties_class, "id", "I");
  jfieldID tool_type_field =
      pointer_properties_class == nullptr
          ? nullptr
          : env->GetFieldID(pointer_properties_class, "toolType", "I");
  jfieldID coords_x_field = pointer_coords_class == nullptr
                                ? nullptr
                                : env->GetFieldID(pointer_coords_class, "x", "F");
  jfieldID coords_y_field = pointer_coords_class == nullptr
                                ? nullptr
                                : env->GetFieldID(pointer_coords_class, "y", "F");
  jfieldID pressure_field =
      pointer_coords_class == nullptr
          ? nullptr
          : env->GetFieldID(pointer_coords_class, "pressure", "F");
  jfieldID size_field = pointer_coords_class == nullptr
                            ? nullptr
                            : env->GetFieldID(pointer_coords_class, "size", "F");
  jmethodID obtain =
      motion_event_class == nullptr
          ? nullptr
          : env->GetStaticMethodID(
                motion_event_class, "obtain",
                "(JJII[Landroid/view/MotionEvent$PointerProperties;"
                "[Landroid/view/MotionEvent$PointerCoords;IIFFIIIII)"
                "Landroid/view/MotionEvent;");
  jmethodID recycle = motion_event_class == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->GetMethodID(motion_event_class, "recycle", "()V");
  if (obtain == nullptr || properties_constructor == nullptr ||
      coords_constructor == nullptr || pointer_id_field == nullptr ||
      tool_type_field == nullptr || coords_x_field == nullptr ||
      coords_y_field == nullptr || pressure_field == nullptr ||
      size_field == nullptr || recycle == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    if (pointer_coords_class != nullptr) env->DeleteLocalRef(pointer_coords_class);
    if (pointer_properties_class != nullptr)
      env->DeleteLocalRef(pointer_properties_class);
    env->DeleteLocalRef(motion_event_class);
    return 80;
  }
  constexpr uint32_t kPointerFlagMouse = 1u << 0;
  const bool mouse_source = (pointer_flags & kPointerFlagMouse) != 0;
  const jint android_source = mouse_source ? 0x2002 : 0x1002;
  const jint android_action = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only ? 4 : action;
  const jfloat pressure = terminal || darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only
                              ? 0.0f
                              : 1.0f;
  jobject properties =
      env->NewObject(pointer_properties_class, properties_constructor);
  jobject coords = env->NewObject(pointer_coords_class, coords_constructor);
  jobjectArray properties_array =
      env->NewObjectArray(1, pointer_properties_class, nullptr);
  jobjectArray coords_array =
      env->NewObjectArray(1, pointer_coords_class, nullptr);
  if (properties != nullptr && coords != nullptr && properties_array != nullptr &&
      coords_array != nullptr && !env->ExceptionCheck()) {
    env->SetIntField(properties, pointer_id_field, 0);
    // MotionEvent.TOOL_TYPE_MOUSE=3, TOOL_TYPE_FINGER=1.
    env->SetIntField(properties, tool_type_field, mouse_source ? 3 : 1);
    env->SetFloatField(coords, coords_x_field, x);
    env->SetFloatField(coords, coords_y_field, y);
    env->SetFloatField(coords, pressure_field, pressure);
    env->SetFloatField(coords, size_field, 1.0f);
    env->SetObjectArrayElement(properties_array, 0, properties);
    env->SetObjectArrayElement(coords_array, 0, coords);
  }
  if (properties == nullptr || coords == nullptr || properties_array == nullptr ||
      coords_array == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    if (coords_array != nullptr) env->DeleteLocalRef(coords_array);
    if (properties_array != nullptr) env->DeleteLocalRef(properties_array);
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    env->DeleteLocalRef(pointer_coords_class);
    env->DeleteLocalRef(pointer_properties_class);
    env->DeleteLocalRef(motion_event_class);
    return 81;
  }
  const jint button_state = mouse_source && !terminal ? 1 : 0;
  jobject event = env->CallStaticObjectMethod(
      motion_event_class, obtain, static_cast<jlong>(down_time_nanos / 1000000),
      static_cast<jlong>(event_time_nanos / 1000000), android_action,
      static_cast<jint>(1), properties_array, coords_array,
      static_cast<jint>(0), button_state, static_cast<jfloat>(1.0f),
      static_cast<jfloat>(1.0f), static_cast<jint>(mouse_source ? 1 : 0),
      static_cast<jint>(0), android_source, static_cast<jint>(0),
      static_cast<jint>(0));
  darwin_art_graphics_fixture::FixtureMotionEventRecycler original_event(
      env, event, recycle);
  if (coords_array != nullptr) env->DeleteLocalRef(coords_array);
  if (properties_array != nullptr) env->DeleteLocalRef(properties_array);
  if (coords != nullptr) env->DeleteLocalRef(coords);
  if (properties != nullptr) env->DeleteLocalRef(properties);
  env->DeleteLocalRef(pointer_coords_class);
  env->DeleteLocalRef(pointer_properties_class);
  if (event == nullptr || !original_event.valid() || env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 81;
  }
  // Preserve the host device class at Android's InputEvent boundary. AppKit
  // mouse/trackpad clicks are external mouse input; synthetic acceptance
  // packets retain the zero-flag touchscreen default.
  jmethodID set_source = env->GetMethodID(motion_event_class, "setSource", "(I)V");
  if (set_source != nullptr && !env->ExceptionCheck()) {
    env->CallVoidMethod(event, set_source, android_source);
  }
  if (env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 81;
  }
  jobject dispatch_root = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_root == nullptr
                              ? root
                              : darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_root;
  const jfloat local_x = x - darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_x;
  const jfloat local_y = y - darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_y;
  if (down && std::getenv("DARWIN_ART_DEBUG_POINTER") != nullptr) {
    jclass state_view_class = env->FindClass("android/view/View");
    jmethodID has_window_focus =
        state_view_class == nullptr
            ? nullptr
            : env->GetMethodID(state_view_class, "hasWindowFocus", "()Z");
    jmethodID is_in_touch_mode =
        state_view_class == nullptr
            ? nullptr
            : env->GetMethodID(state_view_class, "isInTouchMode", "()Z");
    const bool window_focused =
        has_window_focus != nullptr &&
        env->CallBooleanMethod(dispatch_root, has_window_focus) == JNI_TRUE;
    const bool touch_mode =
        is_in_touch_mode != nullptr &&
        env->CallBooleanMethod(dispatch_root, is_in_touch_mode) == JNI_TRUE;
    jobject debug_hit =
        find_clickable_view_at(env, dispatch_root, local_x, local_y);
    std::cerr << "ART Android MotionEvent target x=" << x << " y=" << y
              << " local_x=" << local_x << " local_y=" << local_y
              << " offset_x=" << darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_x
              << " offset_y=" << darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_y
              << " focused=" << (window_focused ? 1 : 0)
              << " touch_mode=" << (touch_mode ? 1 : 0)
              << " outside=" << (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only ? 1 : 0)
              << " hit=" << debug_hit;
    if (debug_hit != nullptr) {
      jmethodID is_focusable =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "isFocusable", "()Z");
      jmethodID is_focusable_in_touch_mode =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "isFocusableInTouchMode",
                                 "()Z");
      jmethodID is_focused =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "isFocused", "()Z");
      jmethodID has_click_listener =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "hasOnClickListeners", "()Z");
      jmethodID get_location_in_window =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "getLocationInWindow", "([I)V");
      jmethodID get_width =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "getWidth", "()I");
      jmethodID get_height =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "getHeight", "()I");
      if (is_focusable != nullptr && is_focusable_in_touch_mode != nullptr &&
          is_focused != nullptr && has_click_listener != nullptr &&
          !env->ExceptionCheck()) {
        std::cerr << " focusable="
                  << (env->CallBooleanMethod(debug_hit, is_focusable) == JNI_TRUE)
                  << " focusable_touch="
                  << (env->CallBooleanMethod(debug_hit,
                                             is_focusable_in_touch_mode) == JNI_TRUE)
                  << " view_focused="
                  << (env->CallBooleanMethod(debug_hit, is_focused) == JNI_TRUE)
                  << " click_listener="
                  << (env->CallBooleanMethod(debug_hit,
                                             has_click_listener) == JNI_TRUE);
      }
      if (get_location_in_window != nullptr && get_width != nullptr &&
          get_height != nullptr && !env->ExceptionCheck()) {
        jintArray location = env->NewIntArray(2);
        if (location != nullptr && !env->ExceptionCheck()) {
          env->CallVoidMethod(debug_hit, get_location_in_window, location);
          jint bounds[2] = {0, 0};
          env->GetIntArrayRegion(location, 0, 2, bounds);
          if (!env->ExceptionCheck()) {
            std::cerr << " bounds=" << bounds[0] << ',' << bounds[1] << '-'
                      << bounds[0] + env->CallIntMethod(debug_hit, get_width)
                      << ','
                      << bounds[1] + env->CallIntMethod(debug_hit, get_height);
          }
        }
        if (location != nullptr) env->DeleteLocalRef(location);
      }
      jclass object_class = env->FindClass("java/lang/Object");
      jmethodID to_string =
          object_class == nullptr
              ? nullptr
              : env->GetMethodID(object_class, "toString", "()Ljava/lang/String;");
      jstring description =
          to_string == nullptr
              ? nullptr
              : static_cast<jstring>(env->CallObjectMethod(debug_hit, to_string));
      if (description != nullptr && !env->ExceptionCheck()) {
        const char* utf = env->GetStringUTFChars(description, nullptr);
        if (utf != nullptr) {
          std::cerr << " view=" << utf;
          env->ReleaseStringUTFChars(description, utf);
        }
      }
      if (description != nullptr) env->DeleteLocalRef(description);
      if (object_class != nullptr) env->DeleteLocalRef(object_class);
    }
    std::cerr << "\n";
    if (debug_hit != nullptr) env->DeleteLocalRef(debug_hit);
    if (state_view_class != nullptr) env->DeleteLocalRef(state_view_class);
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  if (down) {
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_down_x = x;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_down_y = y;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate = !darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_outside_only;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_touch_slop = 8;
    jclass view_class = env->FindClass("android/view/View");
    jmethodID get_context =
        view_class == nullptr
            ? nullptr
            : env->GetMethodID(view_class, "getContext",
                               "()Landroid/content/Context;");
    jobject context = get_context == nullptr
                          ? nullptr
                          : env->CallObjectMethod(dispatch_root, get_context);
    jclass config_class = env->FindClass("android/view/ViewConfiguration");
    jmethodID get_config =
        config_class == nullptr
            ? nullptr
            : env->GetStaticMethodID(
                  config_class, "get",
                  "(Landroid/content/Context;)Landroid/view/ViewConfiguration;");
    jobject config = context == nullptr || get_config == nullptr
                         ? nullptr
                         : env->CallStaticObjectMethod(config_class, get_config, context);
    jmethodID get_touch_slop =
        config_class == nullptr
            ? nullptr
            : env->GetMethodID(config_class, "getScaledTouchSlop", "()I");
    if (config != nullptr && get_touch_slop != nullptr &&
        !env->ExceptionCheck()) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_touch_slop =
          std::max<jint>(1, env->CallIntMethod(config, get_touch_slop));
    }
    if (env->ExceptionCheck()) env->ExceptionClear();
    if (config != nullptr) env->DeleteLocalRef(config);
    if (config_class != nullptr) env->DeleteLocalRef(config_class);
    if (context != nullptr) env->DeleteLocalRef(context);
    if (view_class != nullptr) env->DeleteLocalRef(view_class);
  } else if (action == 2u && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate) {
    const float dx = x - darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_down_x;
    const float dy = y - darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_down_y;
    const float slop = static_cast<float>(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_touch_slop);
    if (dx * dx + dy * dy > slop * slop) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate = false;
    }
  }
  jmethodID offset_location =
      env->GetMethodID(motion_event_class, "offsetLocation", "(FF)V");
  if (offset_location == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    ClearPointerDispatchRoot(state, env);
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 82;
  }
  if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_x != 0.0f ||
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_y != 0.0f) {
    env->CallVoidMethod(event, offset_location,
                        -darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_x,
                        -darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_offset_y);
  }
  jclass root_class = env->GetObjectClass(dispatch_root);
  if (recycle == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    ClearPointerDispatchRoot(state, env);
    env->DeleteLocalRef(root_class);
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 82;
  }
  const bool has_view_root = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_view_root != nullptr;
  // Legacy probes can still run without a WindowManager attachment. Keep
  // their bounded click shim isolated from the APK path; a real ViewRoot owns
  // click cancellation, touch slop, and PerformClick scheduling itself.
  if (!has_view_root && down) {
    if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view != nullptr) {
      env->DeleteGlobalRef(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view);
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view = nullptr;
    }
    jobject hit = find_clickable_view_at(env, dispatch_root, local_x, local_y);
    if (hit != nullptr && !env->ExceptionCheck()) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view = env->NewGlobalRef(hit);
    }
    if (hit != nullptr) env->DeleteLocalRef(hit);
  }
  const int64_t dispatch_start = MonotonicNanos();
  jboolean consumed = JNI_FALSE;
  bool enqueued = false;
  jclass input_host = LoadProbeAnimationHost(env);
  if (input_host != nullptr && !g_motion_event_archive_probe_done &&
      std::getenv("DARWIN_ART_DEBUG_MOTION_EVENT_ARCHIVE") != nullptr) {
    jmethodID archive_probe =
        env->GetStaticMethodID(input_host, "motionEventArchiveProbe", "()I");
    if (archive_probe != nullptr && !env->ExceptionCheck()) {
      const jint probe_status =
          env->CallStaticIntMethod(input_host, archive_probe);
      if (!env->ExceptionCheck()) {
        std::cerr << "ART Android MotionEvent archive probe status="
                  << probe_status << " history=1 copy=1\n";
        g_motion_event_archive_probe_done = true;
      }
    }
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  jclass view_root_class =
      has_view_root ? env->GetObjectClass(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_view_root)
                    : nullptr;
  // TEST ONLY: submit the shared wire ABI to a genuine imported channel.
  // Its real receiver constructs/owns its own Java event; this local event
  // remains fixture-owned for diagnostics and the detached DecorView path.
  if (has_view_root) {
    darwin_art::DarwinArtInputPacket packet;
    packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
    packet.pointer.version = 2;
    packet.pointer.size = sizeof(packet.pointer);
    packet.pointer.action = android_action;
    packet.pointer.flags = pointer_flags;
    packet.pointer.event_time_nanos = event_time_nanos;
    packet.pointer.down_time_nanos = down_time_nanos;
    packet.pointer.pointer_count = 1;
    packet.pointer.x = local_x;
    packet.pointer.y = local_y;
    packet.pointer.raw_x = x;
    packet.pointer.raw_y = y;
    packet.pointer.pressure = pressure;
    packet.pointer.size_value = 1.0f;
    const auto framework_result =
        darwin_art_graphics_fixture::DispatchFixtureInputPacket(
            darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state), env,
            darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)
                ->pointer_dispatch_view_root,
            packet, ProgressFixtureOwner, state);
    enqueued = framework_result.delivered && !framework_result.failed;
    consumed = framework_result.handled ? JNI_TRUE : JNI_FALSE;
    // Opt-in lifecycle diagnostics for real APKs. ViewRoot owns the gesture
    // target in this path, so inspect the hit view after each terminal edge
    // without changing dispatch semantics or forcing performClick().
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_STATE") != nullptr &&
        (action == 0u || action == 1u)) {
      jobject state_hit = find_clickable_view_at(env, dispatch_root, local_x, local_y);
      jclass state_view_class = env->FindClass("android/view/View");
      jmethodID is_pressed =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "isPressed", "()Z");
      jmethodID is_focused =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "isFocused", "()Z");
      jmethodID has_click_listener =
          state_view_class == nullptr
              ? nullptr
              : env->GetMethodID(state_view_class, "hasOnClickListeners", "()Z");
      jfieldID perform_click_runnable =
          state_view_class == nullptr
              ? nullptr
              : env->GetFieldID(state_view_class, "mPerformClick",
                                "Landroid/view/View$PerformClick;");
      jobject pending_click =
          state_hit != nullptr && perform_click_runnable != nullptr &&
                  !env->ExceptionCheck()
              ? env->GetObjectField(state_hit, perform_click_runnable)
              : nullptr;
      if (env->ExceptionCheck()) env->ExceptionClear();
      std::cerr << "ART Android input state action=" << action
                << " hit=" << state_hit
                << " pressed="
                << (state_hit != nullptr && is_pressed != nullptr &&
                            !env->ExceptionCheck() &&
                            env->CallBooleanMethod(state_hit, is_pressed) == JNI_TRUE
                        ? 1
                        : 0)
                << " focused="
                << (state_hit != nullptr && is_focused != nullptr &&
                            !env->ExceptionCheck() &&
                            env->CallBooleanMethod(state_hit, is_focused) == JNI_TRUE
                        ? 1
                        : 0)
                << " click_listener="
                << (state_hit != nullptr && has_click_listener != nullptr &&
                            !env->ExceptionCheck() &&
                            env->CallBooleanMethod(state_hit, has_click_listener) == JNI_TRUE
                        ? 1
                        : 0)
                << " pending_click=" << (pending_click != nullptr ? 1 : 0)
                << " consumed=" << (consumed == JNI_TRUE ? 1 : 0) << "\n";
      if (env->ExceptionCheck()) env->ExceptionClear();
      if (pending_click != nullptr) env->DeleteLocalRef(pending_click);
      if (state_hit != nullptr) env->DeleteLocalRef(state_hit);
      if (state_view_class != nullptr) env->DeleteLocalRef(state_view_class);
    }
  }
  if (env->ExceptionCheck()) {
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      std::cerr << "ART Android MotionEvent: InputChannel delivery failed\n";
      jthrowable error = env->ExceptionOccurred();
      env->ExceptionClear();
      jclass throwable_class = env->FindClass("java/lang/Throwable");
      jmethodID to_string =
          throwable_class == nullptr
              ? nullptr
              : env->GetMethodID(throwable_class, "toString",
                                 "()Ljava/lang/String;");
      jstring description =
          error == nullptr || to_string == nullptr
              ? nullptr
              : static_cast<jstring>(env->CallObjectMethod(error, to_string));
      if (description != nullptr && !env->ExceptionCheck()) {
        const char* utf = env->GetStringUTFChars(description, nullptr);
        if (utf != nullptr) {
          std::cerr << "ART Android MotionEvent exception=" << utf << "\n";
          env->ReleaseStringUTFChars(description, utf);
        }
      }
      if (env->ExceptionCheck()) env->ExceptionClear();
      if (description != nullptr) env->DeleteLocalRef(description);
      if (throwable_class != nullptr) env->DeleteLocalRef(throwable_class);
      if (error != nullptr) env->DeleteLocalRef(error);
    }
    env->ExceptionClear();
    enqueued = false;
  }
  if (has_view_root && !enqueued) {
    ClearPointerDispatchRoot(state, env);
    env->DeleteLocalRef(view_root_class);
    env->DeleteLocalRef(input_host);
    env->DeleteLocalRef(root_class);
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 84;
  }
  if (!has_view_root) {
    jmethodID dispatch_touch = root_class == nullptr
                                   ? nullptr
                                   : env->GetMethodID(
                                         root_class, "dispatchTouchEvent",
                                         "(Landroid/view/MotionEvent;)Z");
    if (dispatch_touch == nullptr || env->ExceptionCheck()) {
      env->ExceptionClear();
      ClearPointerDispatchRoot(state, env);
      env->DeleteLocalRef(view_root_class);
      env->DeleteLocalRef(input_host);
      env->DeleteLocalRef(root_class);
      env->DeleteLocalRef(event);
      env->DeleteLocalRef(motion_event_class);
      return 82;
    }
    consumed = env->CallBooleanMethod(dispatch_root, dispatch_touch, event);
  }
  if (!enqueued && terminal && darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view != nullptr) {
    jobject hit = action == 1u && !env->ExceptionCheck()
                      ? find_clickable_view_at(env, dispatch_root, local_x, local_y)
                      : nullptr;
    const bool same_target =
        hit != nullptr && !env->ExceptionCheck() &&
        env->IsSameObject(hit, darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view) == JNI_TRUE;
    if (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate && same_target &&
        !env->ExceptionCheck()) {
      jclass view_class = env->FindClass("android/view/View");
      jfieldID perform_click_runnable =
          view_class == nullptr
              ? nullptr
              : env->GetFieldID(view_class, "mPerformClick",
                                "Landroid/view/View$PerformClick;");
      jmethodID remove_callbacks =
          view_class == nullptr
              ? nullptr
              : env->GetMethodID(view_class, "removeCallbacks",
                                 "(Ljava/lang/Runnable;)Z");
      jmethodID perform_click =
          view_class == nullptr
              ? nullptr
              : env->GetMethodID(view_class, "performClick", "()Z");
      jobject runnable =
          perform_click_runnable == nullptr
              ? nullptr
              : env->GetObjectField(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view,
                                    perform_click_runnable);
      if (runnable != nullptr && remove_callbacks != nullptr &&
          !env->ExceptionCheck()) {
        env->CallBooleanMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view, remove_callbacks, runnable);
      }
      if (perform_click != nullptr && !env->ExceptionCheck()) {
        env->CallBooleanMethod(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view, perform_click);
      }
      if (runnable != nullptr) env->DeleteLocalRef(runnable);
      if (view_class != nullptr) env->DeleteLocalRef(view_class);
    }
    if (hit != nullptr) env->DeleteLocalRef(hit);
    env->DeleteGlobalRef(darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view);
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view = nullptr;
  }
  const int64_t dispatch_end = MonotonicNanos();
  const bool dispatch_ok = !env->ExceptionCheck();
  if (!dispatch_ok && std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android MotionEvent dispatch exception\n";
    env->ExceptionDescribe();
  }
  // The imported receiver owns a separate event constructed from the packet.
  // This local diagnostic/DecorView event always remains fixture-owned.
  bool recycle_ok = true;
  {
    recycle_ok = original_event.Recycle();
    if (!recycle_ok && std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      std::cerr << "ART Android MotionEvent recycle exception\n";
      env->ExceptionDescribe();
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::cerr << "ART Android MotionEvent ABI2 action=" << action
              << " consumed=" << (consumed == JNI_TRUE ? 1 : 0)
              << " path=" << (enqueued ? "input-channel" : "decor")
              << " window=" << (darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_dispatch_is_window ? 1 : 0)
              << " dispatch_us="
              << (dispatch_end > dispatch_start
                      ? (dispatch_end - dispatch_start) / 1000
                      : 0)
              << " owner=" << pthread_self() << "\n";
  }
  if (!dispatch_ok || !recycle_ok) {
    env->ExceptionClear();
    ClearPointerDispatchRoot(state, env);
    env->DeleteLocalRef(view_root_class);
    env->DeleteLocalRef(input_host);
    env->DeleteLocalRef(root_class);
    env->DeleteLocalRef(event);
    env->DeleteLocalRef(motion_event_class);
    return 83;
  }
  if (down) state->pointer_stream_active = true;
  if (terminal) {
    if (std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE") != nullptr) {
      g_debug_main_queue_budget = 32;
    }
    state->pointer_stream_active = false;
    state->pointer_down_time_nanos = 0;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pointer_click_candidate = false;
    DebugViewTextState(env, dispatch_root);
    ClearPointerDispatchRoot(state, env);
  }
  const bool compat_ripple = CompatRippleOverlayEnabled();
  darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active = compat_ripple && action != 3u;
  if (compat_ripple && down) {
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_x = x;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_y = y;
    darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_started = std::chrono::steady_clock::now();
  }
  if (action != 2u) darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node_recorded = false;
  env->DeleteLocalRef(view_root_class);
  env->DeleteLocalRef(root_class);
  env->DeleteLocalRef(input_host);
  env->DeleteLocalRef(event);
  env->DeleteLocalRef(motion_event_class);
  // Input mutates Android state only. The owner-thread vsync loop performs
  // the single RenderNode/Metal submission for the next frame.
  return 0;
}

}  // namespace

jobject find_clickable_view_at(JNIEnv* env, jobject view, jfloat x, jfloat y) {
  if (view == nullptr || x < 0.0f || y < 0.0f || env->ExceptionCheck()) {
    return nullptr;
  }
  jclass view_class = env->FindClass("android/view/View");
  jclass group_class = env->FindClass("android/view/ViewGroup");
  if (view_class == nullptr || group_class == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(group_class);
    env->DeleteLocalRef(view_class);
    return nullptr;
  }
  jmethodID get_width = env->GetMethodID(view_class, "getWidth", "()I");
  jmethodID get_height = env->GetMethodID(view_class, "getHeight", "()I");
  jmethodID get_visibility =
      env->GetMethodID(view_class, "getVisibility", "()I");
  jmethodID is_enabled = env->GetMethodID(view_class, "isEnabled", "()Z");
  jmethodID is_clickable = env->GetMethodID(view_class, "isClickable", "()Z");
  if (get_width == nullptr || get_height == nullptr ||
      get_visibility == nullptr || is_enabled == nullptr ||
      is_clickable == nullptr || env->ExceptionCheck()) {
    env->DeleteLocalRef(group_class);
    env->DeleteLocalRef(view_class);
    return nullptr;
  }
  const jint width = env->CallIntMethod(view, get_width);
  const jint height = env->CallIntMethod(view, get_height);
  const jint visibility = env->CallIntMethod(view, get_visibility);
  const jboolean enabled = env->CallBooleanMethod(view, is_enabled);
  if (env->ExceptionCheck() || visibility != 0 || enabled != JNI_TRUE ||
      x >= static_cast<jfloat>(width) || y >= static_cast<jfloat>(height)) {
    env->DeleteLocalRef(group_class);
    env->DeleteLocalRef(view_class);
    return nullptr;
  }

  if (env->IsInstanceOf(view, group_class)) {
    jmethodID get_child_count =
        env->GetMethodID(group_class, "getChildCount", "()I");
    jmethodID get_child_at = env->GetMethodID(
        group_class, "getChildAt", "(I)Landroid/view/View;");
    jmethodID get_left = env->GetMethodID(view_class, "getLeft", "()I");
    jmethodID get_top = env->GetMethodID(view_class, "getTop", "()I");
    if (get_child_count != nullptr && get_child_at != nullptr &&
        get_left != nullptr && get_top != nullptr && !env->ExceptionCheck()) {
      const jint child_count = env->CallIntMethod(view, get_child_count);
      for (jint index = child_count - 1; index >= 0 && !env->ExceptionCheck();
           --index) {
        jobject child = env->CallObjectMethod(view, get_child_at, index);
        if (child == nullptr || env->ExceptionCheck()) {
          env->DeleteLocalRef(child);
          continue;
        }
        const jint left = env->CallIntMethod(child, get_left);
        const jint top = env->CallIntMethod(child, get_top);
        jobject result = find_clickable_view_at(env, child, x - left, y - top);
        env->DeleteLocalRef(child);
        if (result != nullptr) {
          env->DeleteLocalRef(group_class);
          env->DeleteLocalRef(view_class);
          return result;
        }
      }
    }
  }
  jobject result =
      env->CallBooleanMethod(view, is_clickable) == JNI_TRUE &&
              !env->ExceptionCheck()
          ? env->NewLocalRef(view)
          : nullptr;
  env->DeleteLocalRef(group_class);
  env->DeleteLocalRef(view_class);
  return result;
}

int32_t dispatch_pointer_internal(GraphicsState* state, uint32_t action, float x,
                                  float y, uint32_t pointer_flags,
                                  uint64_t event_time_nanos,
                                  uint64_t down_time_nanos) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (action > 3u || !std::isfinite(x) || !std::isfinite(y)) {
    return 71;
  }
  art::Thread* art_thread = darwin_art_process::owner_thread_for_callback();
  if (art_thread == nullptr) return 72;
  if (art::Thread::Current() != art_thread ||
      art_thread->GetState() != art::ThreadState::kNative) {
    return 73;
  }

  art::ScopedObjectAccess soa(art_thread);
  JNIEnv* env = art_thread->GetJniEnv();
  darwin_art_graphics_fixture::GraphicsFixtureInvocation invocation(state, env);
  auto* fixture = invocation.get();
  if (fixture == nullptr) return 74;
  jobject root = fixture->interactive_root;
  if (root == nullptr) return 72;
  if (MotionEventBridgeEnabled()) {
    // The host keeps pulsing the retained RenderNode after ACTION_UP so
    // animations can finish. Those replay samples are not Android pointer
    // events: never synthesize a MOVE after the stream has been terminated.
    // Rendering remains on the same owner thread and is still presented once
    // for the current frame.
    if (action == 2u && !state->pointer_stream_active) {
      // Post-terminal animation replays are not Android pointer events. The
      // frame loop owns their presentation, so this path stays side-effect
      // free and never submits a frame per synthetic MOVE.
      return 0;
    }
    const int32_t status = dispatch_motion_event(
        state, env, root, action, x, y, pointer_flags, event_time_nanos,
        down_time_nanos);
    if (status == 0 || std::getenv("DARWIN_ART_INPUT_MODE") != nullptr ||
        std::getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr) {
      return status;
    }
    std::cerr << "ART Android input: MotionEvent bridge unavailable status=" << status
              << ", using explicit direct fallback\n";
  }
  jobject hit = find_clickable_view_at(env, root, x, y);
  if (std::getenv("DARWIN_ART_DEBUG_POINTER") != nullptr) {
    std::cerr << "ART Android input debug action=" << action << " x=" << x
              << " y=" << y << " hit=" << hit << "\n";
  }
  jclass view_class = env->FindClass("android/view/View");
  jmethodID set_pressed =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "setPressed", "(Z)V");
  jmethodID perform_click =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "performClick", "()Z");
  jmethodID drawable_hotspot_changed =
      view_class == nullptr
          ? nullptr
          : env->GetMethodID(view_class, "drawableHotspotChanged", "(FF)V");
  if (view_class == nullptr || set_pressed == nullptr ||
      perform_click == nullptr || drawable_hotspot_changed == nullptr ||
      env->ExceptionCheck()) {
    env->DeleteLocalRef(hit);
    env->DeleteLocalRef(view_class);
    return 74;
  }

  jobject& pressed_view = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pressed_view;
  uint32_t& pending_action = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_action;
  jfloat& pending_x = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_x;
  jfloat& pending_y = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->pending_pressed_y;
  if (action == 0u) {
    if (std::getenv("DARWIN_ART_APK_APP_EXPECT_WIDGETS") != nullptr &&
        hit == nullptr) {
      env->DeleteLocalRef(view_class);
      env->DeleteLocalRef(hit);
      std::cerr << "ART Android input: expected APK clickable target was not hit\n";
      return 76;
    }
    if (CompatRippleOverlayEnabled()) {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active = true;
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_x = x;
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_y = y;
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_started = std::chrono::steady_clock::now();
    } else {
      darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_ripple_overlay_active = false;
    }
    if (pressed_view != nullptr) {
      env->CallVoidMethod(pressed_view, set_pressed, JNI_FALSE);
      env->DeleteGlobalRef(pressed_view);
      pressed_view = nullptr;
    }
    if (hit != nullptr && !env->ExceptionCheck()) {
      pressed_view = env->NewGlobalRef(hit);
      pending_action = 1;
      pending_x = x;
      pending_y = y;
    }
  } else if (action == 2u) {
    if (pressed_view != nullptr && !env->ExceptionCheck()) {
      env->CallVoidMethod(pressed_view, drawable_hotspot_changed, x, y);
    }
  } else if (action == 1u) {
    if (pressed_view != nullptr) {
      pending_action = 2;
      pending_x = x;
      pending_y = y;
    }
  }
  const bool same_pressed_view =
      action == 1u && hit != nullptr && pressed_view != nullptr &&
      env->IsSameObject(hit, pressed_view) == JNI_TRUE;
  if (action == 1u &&
      std::getenv("DARWIN_ART_APK_APP_EXPECT_WIDGETS") != nullptr &&
      !same_pressed_view) {
    env->DeleteLocalRef(hit);
    env->DeleteLocalRef(view_class);
    std::cerr << "ART Android input: APK pointer release missed pressed target\n";
    return 77;
  }
  env->DeleteLocalRef(hit);
  env->DeleteLocalRef(view_class);
  const bool rendered =
      !env->ExceptionCheck() &&
      present_content(state, env, nullptr, root, fixture->interactive_width,
                      fixture->interactive_height) == JNI_TRUE;
  if (env->ExceptionCheck()) {
    std::cerr << "ART Android input: click dispatch threw\n"
              << art_thread->GetException()->Dump() << "\n";
    art_thread->ClearException();
  }
  if (action == 1u && pressed_view != nullptr) {
    if (same_pressed_view && perform_click != nullptr &&
        !env->ExceptionCheck()) {
      const jboolean clicked =
          env->CallBooleanMethod(pressed_view, perform_click);
      if (std::getenv("DARWIN_ART_DEBUG_POINTER") != nullptr) {
        std::cerr << "ART Android input debug performClick="
                  << (clicked == JNI_TRUE ? 1 : 0)
                  << " exception=" << (env->ExceptionCheck() ? 1 : 0)
                  << "\n";
      }
    }
    env->DeleteGlobalRef(pressed_view);
    pressed_view = nullptr;
  }
  return rendered ? 0 : 75;
}

int32_t dispatch_pointer(GraphicsState* state, uint32_t action, float x,
                         float y) {
  return dispatch_pointer_internal(state, action, x, y, 0, 0, 0);
}

int32_t dispatch_pointer_v2(GraphicsState* state,
                             const DarwinArtPointerEventV2* event) {
  if (event == nullptr || event->version != 2 ||
      event->size < sizeof(DarwinArtPointerEventV2) ||
      event->pointer_count == 0 || event->pointer_count > 16) {
    return 71;
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr &&
      event->event_time_nanos > 0) {
    const int64_t now_nanos = MonotonicNanos();
    const int64_t age_nanos =
        now_nanos >= static_cast<int64_t>(event->event_time_nanos)
            ? now_nanos - static_cast<int64_t>(event->event_time_nanos)
            : 0;
    std::cerr << "ART Android PointerEvent ingress action=" << event->action
              << " sequence=" << event->sequence
              << " ingress_latency_us=" << (age_nanos / 1000) << "\n";
  }
  return dispatch_pointer_internal(state, event->action, event->x, event->y,
                                   event->flags,
                                   event->event_time_nanos,
                                   event->down_time_nanos);
}

int32_t dispatch_key_v1(GraphicsState* state,
                        const DarwinArtKeyEventV1* event) {
  if (state == nullptr || event == nullptr || event->version != 1 ||
      event->size < sizeof(DarwinArtKeyEventV1) || event->action > 1) {
    return 85;
  }
  art::Thread* art_thread = darwin_art_process::owner_thread_for_callback();
  if (art_thread == nullptr || art::Thread::Current() != art_thread ||
      art_thread->GetState() != art::ThreadState::kNative) {
    return 73;
  }
  art::ScopedObjectAccess soa(art_thread);
  JNIEnv* env = art_thread->GetJniEnv();
  darwin_art_graphics_fixture::GraphicsFixtureInvocation invocation(state, env);
  auto* fixture = invocation.get();
  if (fixture == nullptr || fixture->interactive_view_root == nullptr) return 85;
  if (!RefreshFocusedWindowRoot(state, env)) {
    env->ExceptionClear();
    return 86;
  }
  jobject dispatch_view_root = fixture->focused_view_root;
  jclass key_event_class = env->FindClass("android/view/KeyEvent");
  jmethodID constructor =
      key_event_class == nullptr
          ? nullptr
          : env->GetMethodID(key_event_class, "<init>", "(JJIIIIIIII)V");
  const int64_t event_time_nanos = event->event_time_nanos > 0
                                       ? static_cast<int64_t>(event->event_time_nanos)
                                       : MonotonicNanos();
  if (event_time_nanos <= 0) {
    env->DeleteLocalRef(key_event_class);
    return 87;
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    const int64_t now_nanos = MonotonicNanos();
    const int64_t age_nanos =
        now_nanos >= event_time_nanos ? now_nanos - event_time_nanos : 0;
    std::cerr << "ART Android KeyEvent ingress action=" << event->action
              << " sequence=" << event->sequence
              << " ingress_latency_us=" << (age_nanos / 1000) << "\n";
  }
  int64_t down_time_nanos = event->down_time_nanos > 0
                                ? static_cast<int64_t>(event->down_time_nanos)
                                : event_time_nanos;
  const size_t key_index = static_cast<size_t>(event->key_code);
  if (key_index < state->key_down_time_nanos.size()) {
    if (event->action == 0) {
      state->key_down_time_nanos[key_index] = down_time_nanos;
    } else if (event->down_time_nanos == 0 &&
               state->key_down_time_nanos[key_index] > 0) {
      down_time_nanos = state->key_down_time_nanos[key_index];
    }
  }
  jobject key_event =
      constructor == nullptr
          ? nullptr
          : env->NewObject(
                key_event_class, constructor,
                static_cast<jlong>(down_time_nanos / 1000000LL),
                static_cast<jlong>(event_time_nanos / 1000000LL),
                static_cast<jint>(event->action),
                static_cast<jint>(event->key_code),
                static_cast<jint>(event->repeat_count),
                static_cast<jint>(event->meta_state),
                static_cast<jint>(event->device_id),
                static_cast<jint>(event->scan_code),
                static_cast<jint>(event->flags),
                static_cast<jint>(event->source));
  if (event->action == 1 && key_index < state->key_down_time_nanos.size()) {
    state->key_down_time_nanos[key_index] = 0;
  }
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  packet.key = *event;
  packet.key.event_time_nanos = event_time_nanos;
  packet.key.down_time_nanos = down_time_nanos;
  const auto framework_result =
      key_event != nullptr && !env->ExceptionCheck()
          ? darwin_art_graphics_fixture::DispatchFixtureInputPacket(
                fixture, env, dispatch_view_root, packet, ProgressFixtureOwner, state)
          : darwin_art_graphics_fixture::FixtureInputDispatchResult{};
  const bool delivered = framework_result.delivered;
  const bool handled = framework_result.handled;
  // The genuine receiver executes ViewRoot's full InputStage chain. Its
  // unhandled ACK is terminal; dispatching again to a focused View would be a
  // second delivery, not completion of the framework's original event.
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    const bool activity_window =
        env->IsSameObject(dispatch_view_root,
                          fixture->interactive_view_root) == JNI_TRUE;
    std::cerr << "ART Android KeyEvent action=" << event->action
              << " key=" << event->key_code
              << " device=" << event->device_id
              << " window=" << (activity_window ? "activity" : "subwindow")
              << " path=input-channel"
              << " delivered=" << (delivered ? 1 : 0)
              << " handled=" << (handled ? 1 : 0)
              << "\n";
  }
  env->DeleteLocalRef(key_event);
  env->DeleteLocalRef(key_event_class);
  const bool dispatch_failed = framework_result.failed || env->ExceptionCheck() ||
      fixture->retiring.load(std::memory_order_acquire);
  if (env->ExceptionCheck()) {
    if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
      env->ExceptionDescribe();
    }
    env->ExceptionClear();
  }
  return delivered && !dispatch_failed ? 0 : 86;
}

int32_t pump_main_looper(GraphicsState* state) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  art::Thread* art_thread = darwin_art_process::owner_thread_for_callback();
  if (art_thread == nullptr) return 72;
  if (art::Thread::Current() != art_thread ||
      art_thread->GetState() != art::ThreadState::kNative) {
    return 73;
  }
  art::ScopedObjectAccess soa(art_thread);
  JNIEnv* env = art_thread->GetJniEnv();
  darwin_art_graphics_fixture::GraphicsFixtureInvocation invocation(state, env);
  auto* fixture = invocation.get();
  if (fixture == nullptr) return 74;
  const bool trace_main_queue =
      std::getenv("DARWIN_ART_DEBUG_MAIN_QUEUE_TRACE") != nullptr;
  if (trace_main_queue) {
    std::cerr << "ART Android Looper trace: pump enter tid="
              << CurrentThreadId() << "\n";
  }
  if (fixture->gpu_surface != nullptr && !fixture->owner_wake_bound) {
    void* looper = darwin_art_android_platform_prepare_current_looper();
    if (looper != nullptr &&
        darwin_art_surface_set_owner_wake(
            fixture->gpu_surface, darwin_art_android_platform_wake_looper,
            looper) == DARWIN_ART_SURFACE_OK) {
      fixture->owner_wake_bound = true;
    }
  }
  // Refresh on both sides of the owner-Looper drain. The pre-drain pass
  // retires a stale channel as soon as topology is visible; the post-drain
  // pass retries a receiver that was visible before its native peer finished
  // initialization. InputChannel-backed receivers consume directly on their
  // registered Looper, so waiting until dispatch_motion_event(DOWN) is too
  // late. AppKit may enqueue concurrently with either pass; the channel mutex
  // preserves packet order, and any packet racing the focus boundary remains
  // owned by the channel that was focused when it was published.
  if (!RefreshFocusedWindowRoot(state, env)) {
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  if (!DispatchDueMainMessages(state, env)) {
    if (env->ExceptionCheck() && art_thread->GetException() != nullptr) {
      std::cerr << "ART Android main Looper dispatch threw\n"
                << art_thread->GetException()->Dump() << "\n";
      art_thread->ClearException();
    }
    return 75;
  }
  // Raw AppKit input is fixture-owned only when this retained sink was
  // explicitly installed. Drain its mixed-kind queue on the Android owner
  // thread, preserving packet order and fixture-side event construction.
  for (;;) {
    darwin_art_graphics_fixture::FixtureQueuedInput packet;
    if (!darwin_art_graphics_fixture::take_surface_input(state, &packet)) {
      break;
    }
    const int32_t dispatch_status =
        packet.kind == darwin_art_graphics_fixture::FixtureQueuedInput::Kind::kPointer
            ? dispatch_pointer_v2(state, &packet.pointer)
            : dispatch_key_v1(state, &packet.key);
    if (dispatch_status != 0) {
      std::cerr << "ART Android fixture input sink dispatch failed status="
                << dispatch_status << "\n";
      return dispatch_status;
    }
  }
  (void)darwin_art_graphics_fixture::acknowledge_surface_input_if_empty(state);
  // Physical AppKit input is consumed by ViewRoot's real InputChannel callback,
  // not dispatch_pointer_internal(). Sample after the owner Looper drain so
  // opt-in diagnostics observe that Android-owned path without changing it.
  DebugViewTextState(env, fixture->interactive_root);
  if (!RefreshFocusedWindowRoot(state, env)) {
    if (env->ExceptionCheck()) env->ExceptionClear();
  }
  // SurfaceView lifecycle repair is display-paced in pump_frame(). Keeping it
  // out of this short owner-Looper poll avoids traversing the full
  // WindowManager hierarchy on every 1–2 ms host turn.
  DebugWindowManagerViews(env);
  if (trace_main_queue) {
    std::cerr << "ART Android Looper trace: pump exit tid="
              << CurrentThreadId() << " status=0\n";
  }
  return 0;
}

void debug_view_text_state(JNIEnv* env, jobject root) {
  DebugViewTextState(env, root);
}

int32_t pump_frame(GraphicsState* state, jlong frame_time_nanos) {
  if (state == nullptr) return DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID;
  if (frame_time_nanos <= 0) {
    frame_time_nanos = darwin_art::AndroidUptimeNanos();
    if (frame_time_nanos <= 0) return 74;
  }
  art::Thread* art_thread = darwin_art_process::owner_thread_for_callback();
  if (art_thread == nullptr) return 72;
  if (art::Thread::Current() != art_thread ||
      art_thread->GetState() != art::ThreadState::kNative) {
    return 73;
  }
  art::ScopedObjectAccess soa(art_thread);
  JNIEnv* env = art_thread->GetJniEnv();
  darwin_art_graphics_fixture::GraphicsFixtureInvocation invocation(state, env);
  auto* fixture = invocation.get();
  if (fixture == nullptr) return 74;
  const bool debug_slow_frame =
      std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr;
  const auto frame_started = std::chrono::steady_clock::now();
  auto log_slow_frame_stage = [&](const char* stage) {
    if (!debug_slow_frame) return;
    const auto elapsed = std::chrono::duration_cast<std::chrono::microseconds>(
                             std::chrono::steady_clock::now() - frame_started)
                             .count();
    if (elapsed >= 100'000) {
      std::cerr << "DARWIN_ART slow-frame stage=" << stage
                << " elapsed_us=" << elapsed << "\n";
    }
  };
  if (!sync_interactive_surface_size(state, env)) return 75;
  log_slow_frame_stage("sync_surface_size");
  // The host owner reactor drains MessageQueue/ALooper before publishing a
  // display edge. Do not drain it again from the frame publisher: doing so
  // couples one vsync to a second opaque native callback batch and lets a
  // Chromium watcher occupy what should be the bounded frame hand-off. This
  // function remains owner-thread-only because framework vsync delivery and
  // ViewRoot bookkeeping use JNI; HWUI RenderThread and final Metal scanout
  // remain asynchronous consumers below.
  DebugWindowManagerViews(env);
#if defined(DARWIN_ART_REAL_GRAPHICS)
  auto* animation_context = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_animation_context.get();
  auto* time_lord = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->hwui_time_lord.get();
  jobject render_node = darwin_art_graphics_fixture::EnsureGraphicsFixtureState(state)->gpu_render_node;
  if (fixture->interactive_view_root == nullptr && animation_context != nullptr &&
      time_lord != nullptr &&
      render_node != nullptr) {
    jclass render_node_class = env->FindClass("android/graphics/RenderNode");
    jfieldID native_render_node =
        render_node_class == nullptr
            ? nullptr
            : env->GetFieldID(render_node_class, "mNativeRenderNode", "J");
    auto* node = native_render_node == nullptr
                     ? nullptr
                     : reinterpret_cast<android::uirenderer::RenderNode*>(
                           static_cast<std::uintptr_t>(env->GetLongField(
                               render_node, native_render_node)));
    if (node != nullptr && !env->ExceptionCheck()) {
      time_lord->vsyncReceived(frame_time_nanos, frame_time_nanos, 0,
                               frame_time_nanos + 16666666, 16666666);
      animation_context->startFrame(android::uirenderer::TreeInfo::MODE_FULL);
      darwin_art_hwui::animate_node_with_context(node, *animation_context);
    }
    env->ExceptionClear();
    env->DeleteLocalRef(render_node_class);
  }
#endif
  // SurfaceFlinger normally signals DisplayEventReceiver on its registered
  // Looper. The Darwin surface loop is the display clock, so publish exactly
  // one pending edge here on the Android owner thread and let the framework's
  // normal async onVsync -> Handler -> Choreographer.doFrame path run below.
  const int delivered_vsyncs =
      darwin_art::DispatchFrameworkPendingVsyncs(env, frame_time_nanos);
  log_slow_frame_stage("dispatch_vsync");
  bool ok = delivered_vsyncs >= 0 && !env->ExceptionCheck();
  // The vsync callback posts Choreographer work onto the owner Looper. Do not
  // synchronously drain MessageQueue again inside this frame pulse: the host
  // turn before the next display edge will dispatch it, matching Android's
  // callback->Handler ordering and preventing MessageQueue.next() from
  // blocking the RenderThread-equivalent pulse behind unrelated UI work.
  log_slow_frame_stage("post_vsync_message_enqueue");
  if (ok && delivered_vsyncs > 0 && fixture->interactive_root != nullptr) {
    debug_product_view_root(env, fixture->interactive_root);
  }
  if (!ok) {
    if (env->ExceptionCheck() && art_thread->GetException() != nullptr) {
      std::cerr << "ART Android frame pulse threw\n"
                << art_thread->GetException()->Dump() << "\n";
      art_thread->ClearException();
    } else {
      std::cerr << "ART Android frame pulse failed without a pending Java exception\n";
    }
  }
  // Product windows are owned by ViewRootImpl/ThreadedRenderer. Their
  // Choreographer traversal records and submits the retained tree through the
  // ANativeWindow queue above; replaying that root here would submit every
  // frame twice and bypass HWUI's damage tracking. Only detached legacy probes
  // retain the bounded host presentation path.
  if (ok && fixture->interactive_root != nullptr &&
      fixture->interactive_view_root == nullptr) {
    ok = present_content(state, env, nullptr, fixture->interactive_root,
                         fixture->interactive_width,
                         fixture->interactive_height) == JNI_TRUE &&
         !env->ExceptionCheck();
    if (!ok && env->ExceptionCheck()) {
      std::cerr << "ART Android frame presentation threw\n"
                << art_thread->GetException()->Dump() << "\n";
      art_thread->ClearException();
    }
  }
  // ViewRootImpl/RenderThread and the central SurfaceFlinger own recording,
  // buffer submission, latching, and composition. The Darwin HWC backend must
  // still scan the completed display IOSurface out to CAMetalLayer. This is a
  // GPU texture blit only: it neither calls View.draw nor replays a RenderNode.
  if (ok && fixture->interactive_view_root != nullptr &&
      fixture->gpu_surface != nullptr) {
    // HWC scanout is an AppKit-actor command. Do not synchronously wait for
    // nextDrawable here: the ART owner must remain free to drain Android
    // Looper work while the main actor presents the persistent IOSurface.
    const DarwinArtSurfaceResult present =
        darwin_art_surface_present_async(fixture->gpu_surface);
    log_slow_frame_stage("enqueue_scanout");
    ok = present == DARWIN_ART_SURFACE_OK ||
         present == DARWIN_ART_SURFACE_DRAWABLE_UNAVAILABLE;
    if (!ok) {
      std::cerr << "ART Android HWC scanout failed status=" << present << "\n";
    }
  }
  return ok ? 0 : 75;
}

}  // namespace darwin_art_graphics
