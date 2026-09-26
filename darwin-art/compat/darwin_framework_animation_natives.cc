#include "darwin_framework_natives.h"
#include "../runtime/framework/looper/message_queue_jni.h"

#include "darwin_android_platform.h"
#include "darwin_android_time.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_framework_system_natives.h"
#include "window/root_geometry.h"

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <ctime>
#include <iostream>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

namespace {

// The refresh interval of the display this process's root is on (the
// built-in ProMotion display paces at 120 Hz), re-read at each schedule so a
// root moved to another display follows it.
std::chrono::nanoseconds DisplayFrameInterval() {
  return std::chrono::nanoseconds(darwin_art::window::ProcessFrameIntervalNanos());
}

std::chrono::steady_clock::time_point NextDisplayDeadline() {
  const auto interval = DisplayFrameInterval();
  const auto now = std::chrono::steady_clock::now();
  const auto elapsed = now.time_since_epoch();
  const auto frame = elapsed / interval + 1;
  return std::chrono::steady_clock::time_point(frame * interval);
}

struct DisplayPulse {
  int64_t timestamp_nanos;
};


// DisplayEventReceiver is the clock edge behind Choreographer, ValueAnimator,
// and framework RippleDrawable.  Android normally gets this edge from
// SurfaceFlinger.  The Darwin host has no SurfaceFlinger, so provide the same
// Java callback contract from a monotonic scheduler at the display's rate.  Keeping the
// receiver in a lease map is important: NativeAllocationRegistry may finalize
// it while a scheduled callback is already waiting on the host thread.
class DarwinDisplayEventReceiver {
 public:
  DarwinDisplayEventReceiver(JavaVM* vm, jobject receiver_weak,
                             jobject vsync_data_weak, jclass receiver_class,
                             jclass vsync_data_class, jclass timeline_class,
                             jmethodID reference_get, jmethodID dispatch_vsync,
                             jfieldID frame_interval,
                             jfieldID preferred_timeline_index,
                             jfieldID frame_timelines_length,
                             jfieldID number_queued_buffers,
                             jfieldID frame_timelines, jfieldID timeline_vsync_id,
                             jfieldID expected_presentation_time,
                             jfieldID deadline)
      : vm_(vm),
        receiver_weak_(receiver_weak),
        vsync_data_weak_(vsync_data_weak),
        receiver_class_(receiver_class),
        vsync_data_class_(vsync_data_class),
        timeline_class_(timeline_class),
        reference_get_(reference_get),
        dispatch_vsync_(dispatch_vsync),
        frame_interval_(frame_interval),
        preferred_timeline_index_(preferred_timeline_index),
        frame_timelines_length_(frame_timelines_length),
        number_queued_buffers_(number_queued_buffers),
        frame_timelines_(frame_timelines),
        timeline_vsync_id_(timeline_vsync_id),
        expected_presentation_time_(expected_presentation_time),
        deadline_(deadline) {}

  ~DarwinDisplayEventReceiver() = default;

  bool InitializeLooperTransport(void* looper) {
    constexpr int kAndroidONonblock = 0x800;
    constexpr int kAndroidOCloexec = 0x80000;
    int32_t descriptors[2] = {-1, -1};
    if (darwin_art_bionic_socket_broker_pipe2(
            descriptors, kAndroidONonblock | kAndroidOCloexec) != 0) {
      return false;
    }
    auto* callback_owner =
        new (std::nothrow) std::weak_ptr<DarwinDisplayEventReceiver>();
    if (callback_owner != nullptr) {
      std::lock_guard lock(mutex_);
      *callback_owner = self_;
    }
    if (looper == nullptr || callback_owner == nullptr) {
      (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
      (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
      delete callback_owner;
      return false;
    }
    if (darwin_art_android_platform_add_fd_owned(
            looper, descriptors[0], 0, 0x0001, &LooperCallback,
            callback_owner, callback_owner, &ReleaseCallbackOwner) != 1) {
      (void)darwin_art_bionic_socket_broker_close(descriptors[0]);
      (void)darwin_art_bionic_socket_broker_close(descriptors[1]);
      return false;
    }
    {
      std::lock_guard lock(mutex_);
      looper_ = looper;
      read_fd_ = descriptors[0];
      write_fd_ = descriptors[1];
      transport_registered_ = true;
    }
    timer_thread_ = std::thread([this] { TimerLoop(); });
    return true;
  }

  void Dispose(JNIEnv* env) {
    void* looper = nullptr;
    int read_fd = -1;
    int write_fd = -1;
    {
      std::unique_lock lock(mutex_);
      disposed_ = true;
      timer_stop_ = true;
      condition_.notify_all();
      condition_.wait(lock, [this] { return callbacks_ == 0; });
      looper = looper_;
      read_fd = read_fd_;
      write_fd = write_fd_;
      looper_ = nullptr;
      read_fd_ = -1;
      write_fd_ = -1;
      transport_registered_ = false;
    }
    if (looper != nullptr && read_fd >= 0) {
      (void)darwin_art_android_platform_remove_fd(looper, read_fd);
    }
    if (timer_thread_.joinable()) timer_thread_.join();
    if (read_fd >= 0) {
      (void)darwin_art_bionic_socket_broker_close(read_fd);
    }
    if (write_fd >= 0) {
      (void)darwin_art_bionic_socket_broker_close(write_fd);
    }
    std::lock_guard lock(mutex_);
    if (receiver_weak_ != nullptr) {
      env->DeleteGlobalRef(receiver_weak_);
      receiver_weak_ = nullptr;
    }
    if (vsync_data_weak_ != nullptr) {
      env->DeleteGlobalRef(vsync_data_weak_);
      vsync_data_weak_ = nullptr;
    }
    if (receiver_class_ != nullptr) {
      env->DeleteGlobalRef(receiver_class_);
      receiver_class_ = nullptr;
    }
    if (vsync_data_class_ != nullptr) {
      env->DeleteGlobalRef(vsync_data_class_);
      vsync_data_class_ = nullptr;
    }
    if (timeline_class_ != nullptr) {
      env->DeleteGlobalRef(timeline_class_);
      timeline_class_ = nullptr;
    }
  }

  void FinalizeFromAnyThread() {
    JNIEnv* env = nullptr;
    bool attached = false;
    if (vm_->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) ==
        JNI_EDETACHED) {
      if (vm_->AttachCurrentThread(&env, nullptr) != JNI_OK) {
        return;
      }
      attached = true;
    }
    Dispose(env);
    if (attached) {
      vm_->DetachCurrentThread();
    }
  }

  void Schedule() {
    std::lock_guard lock(mutex_);
    if (disposed_) return;
    pending_vsync_ = true;
    if (transport_registered_ && !timer_armed_) {
      timer_armed_ = true;
      timer_deadline_ = NextDisplayDeadline();
      condition_.notify_all();
    }
  }

  bool DispatchPending(JNIEnv* env, jlong timestamp) {
    jobject weak = nullptr;
    {
      std::lock_guard lock(mutex_);
      if (disposed_ || !pending_vsync_ || receiver_weak_ == nullptr) {
        return false;
      }
      pending_vsync_ = false;
      ++callbacks_;
      weak = receiver_weak_;
    }

    const jlong vsync_id = next_vsync_id_++;
    jobject receiver = env->CallObjectMethod(weak, reference_get_);
    if (receiver != nullptr && !env->ExceptionCheck()) {
      jobject vsync_data = UpdateVsyncData(env, timestamp, vsync_id);
      env->DeleteLocalRef(vsync_data);
      env->CallVoidMethod(receiver, dispatch_vsync_, timestamp, 0, 0);
      env->DeleteLocalRef(receiver);
    }

    {
      std::lock_guard lock(mutex_);
      if (--callbacks_ == 0) {
        condition_.notify_all();
      }
    }
    return !env->ExceptionCheck();
  }

  jobject LatestVsyncData(JNIEnv* env) {
    return UpdateVsyncData(env, darwin_art::AndroidUptimeNanos(),
                           next_vsync_id_++);
  }

  void SetSelf(const std::shared_ptr<DarwinDisplayEventReceiver>& self) {
    std::lock_guard lock(mutex_);
    self_ = self;
  }

 private:
  static void ReleaseCallbackOwner(void* owner) {
    delete static_cast<std::weak_ptr<DarwinDisplayEventReceiver>*>(owner);
  }

  static int LooperCallback(int fd, int events, void* data) {
    const auto* receiver =
        static_cast<std::weak_ptr<DarwinDisplayEventReceiver>*>(data);
    auto retained = receiver == nullptr ? nullptr : receiver->lock();
    if (retained == nullptr) return 0;
    DisplayPulse pulse{};
    jlong source_timestamp = 0;
    intptr_t received = 0;
    while ((received = darwin_art_bionic_socket_broker_read(
                fd, &pulse, sizeof(pulse))) > 0) {
      if (received == static_cast<intptr_t>(sizeof(pulse))) {
        source_timestamp = pulse.timestamp_nanos;
      }
    }
    JNIEnv* env = nullptr;
    if (retained->vm_->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) !=
        JNI_OK) {
      return 1;
    }
    (void)events;
    if (source_timestamp != 0) {
      retained->DispatchPending(env, source_timestamp);
    }
    return 1;
  }

  void TimerLoop() {
    std::unique_lock lock(mutex_);
    while (!timer_stop_) {
      condition_.wait(lock, [this] { return timer_stop_ || timer_armed_; });
      if (timer_stop_) break;
      const auto deadline = timer_deadline_;
      if (condition_.wait_until(lock, deadline, [this, deadline] {
            return timer_stop_ || !timer_armed_ || timer_deadline_ != deadline;
          })) {
        continue;
      }
      timer_armed_ = false;
      const int write_fd = write_fd_;
      lock.unlock();
      if (write_fd >= 0) {
        const DisplayPulse pulse{darwin_art::AndroidUptimeNanos()};
        (void)darwin_art_bionic_socket_broker_write(write_fd, &pulse,
                                                    sizeof(pulse));
      }
      lock.lock();
    }
  }

  jobject UpdateVsyncData(JNIEnv* env, jlong timestamp, jlong vsync_id) {
    const jlong kFrameIntervalNanos = DisplayFrameInterval().count();
    constexpr jlong kGpuBudgetNanos = 2'000'000;
    if (vsync_data_weak_ == nullptr) return nullptr;
    jobject data = env->CallObjectMethod(vsync_data_weak_, reference_get_);
    if (data == nullptr || env->ExceptionCheck()) return data;

    env->SetLongField(data, frame_interval_, kFrameIntervalNanos);
    env->SetIntField(data, preferred_timeline_index_, 0);
    env->SetIntField(data, frame_timelines_length_, 1);
    env->SetIntField(data, number_queued_buffers_, 0);
    auto timelines = static_cast<jobjectArray>(
        env->GetObjectField(data, frame_timelines_));
    jobject timeline = timelines == nullptr
                           ? nullptr
                           : env->GetObjectArrayElement(timelines, 0);
    if (timeline != nullptr && !env->ExceptionCheck()) {
      const jlong expected = timestamp + kFrameIntervalNanos;
      env->SetLongField(timeline, timeline_vsync_id_, vsync_id);
      env->SetLongField(timeline, expected_presentation_time_, expected);
      env->SetLongField(timeline, deadline_, expected - kGpuBudgetNanos);
    }
    env->DeleteLocalRef(timeline);
    env->DeleteLocalRef(timelines);
    if (env->ExceptionCheck()) {
      env->DeleteLocalRef(data);
      return nullptr;
    }
    return data;
  }

  std::shared_ptr<DarwinDisplayEventReceiver> SharedFromThis() {
    std::lock_guard lock(mutex_);
    if (disposed_) {
      return nullptr;
    }
    return self_.lock();
  }

  JavaVM* vm_;
  jobject receiver_weak_;
  jobject vsync_data_weak_;
  jclass receiver_class_;
  jclass vsync_data_class_;
  jclass timeline_class_;
  jmethodID reference_get_;
  jmethodID dispatch_vsync_;
  jfieldID frame_interval_;
  jfieldID preferred_timeline_index_;
  jfieldID frame_timelines_length_;
  jfieldID number_queued_buffers_;
  jfieldID frame_timelines_;
  jfieldID timeline_vsync_id_;
  jfieldID expected_presentation_time_;
  jfieldID deadline_;
  std::mutex mutex_;
  std::condition_variable condition_;
  std::weak_ptr<DarwinDisplayEventReceiver> self_;
  std::thread timer_thread_;
  std::chrono::steady_clock::time_point timer_deadline_{};
  void* looper_ = nullptr;
  int read_fd_ = -1;
  int write_fd_ = -1;
  size_t callbacks_ = 0;
  jlong next_vsync_id_ = 1;
  bool pending_vsync_ = false;
  bool transport_registered_ = false;
  bool timer_armed_ = false;
  bool timer_stop_ = false;
  bool disposed_ = false;
};

std::mutex g_display_receiver_mutex;
std::unordered_map<jlong, std::shared_ptr<DarwinDisplayEventReceiver>>
    g_display_receivers;

std::shared_ptr<DarwinDisplayEventReceiver> FindDisplayReceiver(jlong handle) {
  std::lock_guard lock(g_display_receiver_mutex);
  const auto found = g_display_receivers.find(handle);
  return found == g_display_receivers.end() ? nullptr : found->second;
}

void DisplayEventReceiverRelease(void* raw) {
  const jlong handle = static_cast<jlong>(reinterpret_cast<std::uintptr_t>(raw));
  std::shared_ptr<DarwinDisplayEventReceiver> receiver;
  {
    std::lock_guard lock(g_display_receiver_mutex);
    const auto found = g_display_receivers.find(handle);
    if (found == g_display_receivers.end()) {
      return;
    }
    receiver = std::move(found->second);
    g_display_receivers.erase(found);
  }
  // The finalizer normally runs on a Java thread, but this also handles an
  // ART shutdown callback from a detached host thread.
  if (receiver != nullptr) {
    receiver->FinalizeFromAnyThread();
  }
}

jlong DisplayEventReceiverNativeInit(JNIEnv* env, jclass, jobject receiver_weak,
                                     jobject vsync_data_weak, jobject message_queue, jint,
                                     jint, jlong) {
  if (receiver_weak == nullptr || vsync_data_weak == nullptr) {
    return 0;
  }
  JavaVM* vm = nullptr;
  if (env->GetJavaVM(&vm) != JNI_OK) {
    return 0;
  }
  jclass reference_class = env->FindClass("java/lang/ref/Reference");
  jmethodID reference_get = reference_class == nullptr
                                ? nullptr
                                : env->GetMethodID(
                                      reference_class, "get",
                                      "()Ljava/lang/Object;");
  jobject receiver = reference_get == nullptr
                         ? nullptr
                         : env->CallObjectMethod(receiver_weak, reference_get);
  jobject vsync_data = reference_get == nullptr
                           ? nullptr
                           : env->CallObjectMethod(vsync_data_weak, reference_get);
  if (receiver == nullptr || vsync_data == nullptr || env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(reference_class);
    env->DeleteLocalRef(receiver);
    env->DeleteLocalRef(vsync_data);
    return 0;
  }
  jclass receiver_class_local = env->GetObjectClass(receiver);
  jclass vsync_data_class_local = env->GetObjectClass(vsync_data);
  jclass timeline_class_local = env->FindClass(
      "android/view/DisplayEventReceiver$VsyncEventData$FrameTimeline");
  jmethodID dispatch_vsync =
      receiver_class_local == nullptr
          ? nullptr
          : env->GetMethodID(receiver_class_local, "dispatchVsync", "(JJI)V");
  jfieldID frame_interval =
      vsync_data_class_local == nullptr
          ? nullptr
          : env->GetFieldID(vsync_data_class_local, "frameInterval", "J");
  jfieldID preferred_timeline_index =
      vsync_data_class_local == nullptr
          ? nullptr
          : env->GetFieldID(vsync_data_class_local,
                            "preferredFrameTimelineIndex", "I");
  jfieldID frame_timelines_length =
      vsync_data_class_local == nullptr
          ? nullptr
          : env->GetFieldID(vsync_data_class_local, "frameTimelinesLength", "I");
  jfieldID number_queued_buffers =
      vsync_data_class_local == nullptr
          ? nullptr
          : env->GetFieldID(vsync_data_class_local, "numberQueuedBuffers", "I");
  jfieldID frame_timelines =
      vsync_data_class_local == nullptr
          ? nullptr
          : env->GetFieldID(
                vsync_data_class_local, "frameTimelines",
                "[Landroid/view/DisplayEventReceiver$VsyncEventData$FrameTimeline;");
  jfieldID timeline_vsync_id =
      timeline_class_local == nullptr
          ? nullptr
          : env->GetFieldID(timeline_class_local, "vsyncId", "J");
  jfieldID expected_presentation_time =
      timeline_class_local == nullptr
          ? nullptr
          : env->GetFieldID(timeline_class_local, "expectedPresentationTime", "J");
  jfieldID deadline = timeline_class_local == nullptr
                          ? nullptr
                          : env->GetFieldID(timeline_class_local, "deadline", "J");
  if (dispatch_vsync == nullptr || frame_interval == nullptr ||
      preferred_timeline_index == nullptr ||
      frame_timelines_length == nullptr || number_queued_buffers == nullptr ||
      frame_timelines == nullptr || timeline_vsync_id == nullptr ||
      expected_presentation_time == nullptr || deadline == nullptr ||
      env->ExceptionCheck()) {
    env->ExceptionClear();
    env->DeleteLocalRef(timeline_class_local);
    env->DeleteLocalRef(vsync_data_class_local);
    env->DeleteLocalRef(receiver_class_local);
    env->DeleteLocalRef(receiver);
    env->DeleteLocalRef(vsync_data);
    env->DeleteLocalRef(reference_class);
    return 0;
  }
  jobject weak_global = env->NewGlobalRef(receiver_weak);
  jobject vsync_data_weak_global = env->NewGlobalRef(vsync_data_weak);
  jclass receiver_class_global =
      static_cast<jclass>(env->NewGlobalRef(receiver_class_local));
  jclass vsync_data_class_global =
      static_cast<jclass>(env->NewGlobalRef(vsync_data_class_local));
  jclass timeline_class_global =
      static_cast<jclass>(env->NewGlobalRef(timeline_class_local));
  env->DeleteLocalRef(timeline_class_local);
  env->DeleteLocalRef(vsync_data_class_local);
  env->DeleteLocalRef(receiver_class_local);
  env->DeleteLocalRef(receiver);
  env->DeleteLocalRef(vsync_data);
  env->DeleteLocalRef(reference_class);
  if (weak_global == nullptr || vsync_data_weak_global == nullptr ||
      receiver_class_global == nullptr || vsync_data_class_global == nullptr ||
      timeline_class_global == nullptr) {
    env->DeleteGlobalRef(weak_global);
    env->DeleteGlobalRef(vsync_data_weak_global);
    env->DeleteGlobalRef(receiver_class_global);
    env->DeleteGlobalRef(vsync_data_class_global);
    env->DeleteGlobalRef(timeline_class_global);
    return 0;
  }
  auto receiver_state = std::make_shared<DarwinDisplayEventReceiver>(
      vm, weak_global, vsync_data_weak_global, receiver_class_global,
      vsync_data_class_global, timeline_class_global, reference_get,
      dispatch_vsync, frame_interval, preferred_timeline_index,
      frame_timelines_length, number_queued_buffers, frame_timelines,
      timeline_vsync_id, expected_presentation_time, deadline);
  const jlong handle = static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      receiver_state.get()));
  receiver_state->SetSelf(receiver_state);
  void* looper =
      darwin_art::framework_system::message_queue_looper(env, message_queue);
  if (looper == nullptr ||
      !receiver_state->InitializeLooperTransport(looper)) {
    receiver_state->Dispose(env);
    return 0;
  }
  {
    std::lock_guard lock(g_display_receiver_mutex);
    g_display_receivers.emplace(handle, std::move(receiver_state));
  }
  return handle;
}

void DisplayEventReceiverNativeScheduleVsync(JNIEnv*, jclass, jlong handle) {
  if (auto receiver = FindDisplayReceiver(handle); receiver != nullptr) {
    receiver->Schedule();
    if (std::getenv("DARWIN_ART_DEBUG_VSYNC") != nullptr) {
      std::cerr << "ART Choreographer vsync scheduled receiver=" << handle
                << "\n";
    }
  }
}

int DispatchPendingVsyncs(JNIEnv* env, jlong frame_time_nanos) {
  std::vector<std::shared_ptr<DarwinDisplayEventReceiver>> receivers;
  {
    std::lock_guard lock(g_display_receiver_mutex);
    receivers.reserve(g_display_receivers.size());
    for (const auto& [handle, receiver] : g_display_receivers) {
      (void)handle;
      receivers.push_back(receiver);
    }
  }
  int delivered = 0;
  for (const auto& receiver : receivers) {
    if (receiver != nullptr && receiver->DispatchPending(env, frame_time_nanos)) {
      ++delivered;
    }
    if (env->ExceptionCheck()) return -1;
  }
  if (std::getenv("DARWIN_ART_DEBUG_VSYNC") != nullptr &&
      (!receivers.empty() || delivered != 0)) {
    std::cerr << "ART Choreographer vsync dispatch receivers="
              << receivers.size() << " delivered=" << delivered << "\n";
  }
  return delivered;
}

jobject DisplayEventReceiverNativeGetLatestVsyncEventData(JNIEnv* env, jclass,
                                                            jlong handle) {
  if (auto receiver = FindDisplayReceiver(handle); receiver != nullptr) {
    return receiver->LatestVsyncData(env);
  }
  return nullptr;
}

jlong PerfettoCategoryNativeGetExtraPtr(jlong) { return 0; }

jlong DisplayEventReceiverNativeFinalizer(JNIEnv*, jclass) {
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      &DisplayEventReceiverRelease));
}

// PropertyValuesHolder's JNI helpers are used by ValueAnimator and
// RippleDrawable. A jmethodID is already an opaque stable JNI handle, so the
// host can preserve Android's lookup/call ABI without exposing ART internals.
jlong PropertyGetMethod(JNIEnv* env, jclass, jclass target, jstring name,
                        const char* signature) {
  if (target == nullptr || name == nullptr) return 0;
  const char* utf_name = env->GetStringUTFChars(name, nullptr);
  if (utf_name == nullptr) return 0;
  jmethodID method = env->GetMethodID(target, utf_name, signature);
  env->ReleaseStringUTFChars(name, utf_name);
  return reinterpret_cast<jlong>(method);
}

jlong PropertyGetFloatMethod(JNIEnv* env, jclass declaring, jclass target,
                             jstring name) {
  return PropertyGetMethod(env, declaring, target, name, "(F)V");
}
jlong PropertyGetIntMethod(JNIEnv* env, jclass declaring, jclass target,
                           jstring name) {
  return PropertyGetMethod(env, declaring, target, name, "(I)V");
}
jlong PropertyGetMultipleFloatMethod(JNIEnv* env, jclass declaring,
                                     jclass target, jstring name, jint count) {
  std::string signature("(");
  signature.append(static_cast<std::size_t>(count > 0 ? count : 0), 'F');
  signature.append(")V");
  return PropertyGetMethod(env, declaring, target, name, signature.c_str());
}
jlong PropertyGetMultipleIntMethod(JNIEnv* env, jclass declaring, jclass target,
                                   jstring name, jint count) {
  std::string signature("(");
  signature.append(static_cast<std::size_t>(count > 0 ? count : 0), 'I');
  signature.append(")V");
  return PropertyGetMethod(env, declaring, target, name, signature.c_str());
}

void PropertyCallFloatMethod(JNIEnv* env, jclass, jobject object, jlong method,
                             jfloat value) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), value);
}
void PropertyCallIntMethod(JNIEnv* env, jclass, jobject object, jlong method,
                           jint value) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), value);
}
void PropertyCallTwoFloatMethod(JNIEnv* env, jclass, jobject object, jlong method,
                                jfloat a, jfloat b) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), a, b);
}
void PropertyCallFourFloatMethod(JNIEnv* env, jclass, jobject object, jlong method,
                                 jfloat a, jfloat b, jfloat c, jfloat d) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), a, b, c,
                        d);
}
void PropertyCallMultipleFloatMethod(JNIEnv* env, jclass, jobject object,
                                     jlong method,
                                     jfloatArray values) {
  if (object == nullptr || method == 0 || values == nullptr) return;
  const jsize count = env->GetArrayLength(values);
  std::vector<jfloat> source(static_cast<std::size_t>(count));
  env->GetFloatArrayRegion(values, 0, count, source.data());
  std::vector<jvalue> arguments(static_cast<std::size_t>(count));
  for (jsize index = 0; index < count; ++index) {
    arguments[static_cast<std::size_t>(index)].f =
        source[static_cast<std::size_t>(index)];
  }
  env->CallVoidMethodA(object, reinterpret_cast<jmethodID>(method),
                       arguments.data());
}
void PropertyCallTwoIntMethod(JNIEnv* env, jclass, jobject object, jlong method,
                              jint a,
                              jint b) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), a, b);
}
void PropertyCallFourIntMethod(JNIEnv* env, jclass, jobject object, jlong method,
                               jint a,
                               jint b, jint c, jint d) {
  if (object != nullptr && method != 0)
    env->CallVoidMethod(object, reinterpret_cast<jmethodID>(method), a, b, c,
                        d);
}
void PropertyCallMultipleIntMethod(JNIEnv* env, jclass, jobject object,
                                   jlong method,
                                   jintArray values) {
  if (object == nullptr || method == 0 || values == nullptr) return;
  const jsize count = env->GetArrayLength(values);
  std::vector<jint> source(static_cast<std::size_t>(count));
  env->GetIntArrayRegion(values, 0, count, source.data());
  std::vector<jvalue> arguments(static_cast<std::size_t>(count));
  for (jsize index = 0; index < count; ++index) {
    arguments[static_cast<std::size_t>(index)].i =
        source[static_cast<std::size_t>(index)];
  }
  env->CallVoidMethodA(object, reinterpret_cast<jmethodID>(method),
                       arguments.data());
}

void PerfettoNoopFinalizer(void*) {}

jlong PerfettoCategoryNativeDelete() {
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      &PerfettoNoopFinalizer));
}

jlong PerfettoCategoryNativeInit(jstring, jstring, jstring) { return 1; }
void PerfettoCategoryNativeRegister(jlong) {}
void PerfettoCategoryNativeUnregister(jlong) {}
jboolean PerfettoCategoryNativeIsEnabled(jlong) { return JNI_FALSE; }

// ProtoLog initializes Perfetto while ViewRootImpl is being constructed. A
// Darwin host has no perfetto producer backend, but the framework still needs
// the exact initialization ABI so tracing remains an inert optional service
// instead of aborting window attachment with UnsatisfiedLinkError.
void PerfettoProducerNativeInit(JNIEnv*, jclass, jint, jint) {}

// ProtoLogDataSource is constructed by ViewRootImpl even when no tracing
// backend is present. Keep its Java-side object lifecycle valid while making
// all trace iteration inert on Darwin.
jlong PerfettoDataSourceNativeCreate(JNIEnv*, jclass, jobject, jstring) {
  return 1;
}
void PerfettoDataSourceNativeFlushAll(JNIEnv*, jclass, jlong) {}
jlong PerfettoDataSourceNativeGetFinalizer(JNIEnv*, jclass) { return 0; }
jint PerfettoDataSourceNativeGetInstanceIndex(JNIEnv*, jclass, jlong) {
  return 0;
}
jobject PerfettoDataSourceNativeGetInstance(JNIEnv*, jclass, jlong, jint) {
  return nullptr;
}
jboolean PerfettoDataSourceNativeIterateBegin(JNIEnv*, jclass, jlong) {
  return JNI_FALSE;
}
void PerfettoDataSourceNativeIterateBreak(JNIEnv*, jclass, jlong) {}
jboolean PerfettoDataSourceNativeIterateNext(JNIEnv*, jclass, jlong) {
  return JNI_FALSE;
}
void PerfettoDataSourceNativeRegister(JNIEnv*, jclass, jlong, jint, jboolean,
                                      jboolean) {}
void PerfettoDataSourceNativeReleaseInstance(JNIEnv*, jclass, jlong, jint) {}
void PerfettoDataSourceNativeWritePackets(JNIEnv*, jclass, jlong, jobjectArray) {}

jlong PerfettoTrackEventExtraNativeDelete() {
  return static_cast<jlong>(reinterpret_cast<std::uintptr_t>(
      &PerfettoNoopFinalizer));
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

namespace darwin_art {

int DispatchFrameworkPendingVsyncs(JNIEnv* env, jlong frame_time_nanos) {
  return DispatchPendingVsyncs(env, frame_time_nanos);
}

bool RegisterFrameworkAnimationNatives(JNIEnv* env) {
  JNINativeMethod display_event_receiver_methods[] = {
      {const_cast<char*>("nativeInit"),
       const_cast<char*>(
           "(Ljava/lang/ref/WeakReference;Ljava/lang/ref/WeakReference;"
           "Landroid/os/MessageQueue;IIJ)J"),
       reinterpret_cast<void*>(&DisplayEventReceiverNativeInit)},
      {const_cast<char*>("nativeGetDisplayEventReceiverFinalizer"),
       const_cast<char*>("()J"),
       reinterpret_cast<void*>(&DisplayEventReceiverNativeFinalizer)},
      {const_cast<char*>("nativeScheduleVsync"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&DisplayEventReceiverNativeScheduleVsync)},
      {const_cast<char*>("nativeGetLatestVsyncEventData"),
       const_cast<char*>(
           "(J)Landroid/view/DisplayEventReceiver$VsyncEventData;"),
       reinterpret_cast<void*>(&DisplayEventReceiverNativeGetLatestVsyncEventData)},
  };
  if (!Register(env, "android/view/DisplayEventReceiver",
                display_event_receiver_methods,
                static_cast<jint>(std::size(display_event_receiver_methods)))) {
    return false;
  }

  JNINativeMethod property_values_holder_methods[] = {
      {const_cast<char*>("nCallFloatMethod"),
       const_cast<char*>("(Ljava/lang/Object;JF)V"),
       reinterpret_cast<void*>(&PropertyCallFloatMethod)},
      {const_cast<char*>("nCallFourFloatMethod"),
       const_cast<char*>("(Ljava/lang/Object;JFFFF)V"),
       reinterpret_cast<void*>(&PropertyCallFourFloatMethod)},
      {const_cast<char*>("nCallFourIntMethod"),
       const_cast<char*>("(Ljava/lang/Object;JIIII)V"),
       reinterpret_cast<void*>(&PropertyCallFourIntMethod)},
      {const_cast<char*>("nCallIntMethod"),
       const_cast<char*>("(Ljava/lang/Object;JI)V"),
       reinterpret_cast<void*>(&PropertyCallIntMethod)},
      {const_cast<char*>("nCallMultipleFloatMethod"),
       const_cast<char*>("(Ljava/lang/Object;J[F)V"),
       reinterpret_cast<void*>(&PropertyCallMultipleFloatMethod)},
      {const_cast<char*>("nCallMultipleIntMethod"),
       const_cast<char*>("(Ljava/lang/Object;J[I)V"),
       reinterpret_cast<void*>(&PropertyCallMultipleIntMethod)},
      {const_cast<char*>("nCallTwoFloatMethod"),
       const_cast<char*>("(Ljava/lang/Object;JFF)V"),
       reinterpret_cast<void*>(&PropertyCallTwoFloatMethod)},
      {const_cast<char*>("nCallTwoIntMethod"),
       const_cast<char*>("(Ljava/lang/Object;JII)V"),
       reinterpret_cast<void*>(&PropertyCallTwoIntMethod)},
      {const_cast<char*>("nGetFloatMethod"),
       const_cast<char*>("(Ljava/lang/Class;Ljava/lang/String;)J"),
       reinterpret_cast<void*>(&PropertyGetFloatMethod)},
      {const_cast<char*>("nGetIntMethod"),
       const_cast<char*>("(Ljava/lang/Class;Ljava/lang/String;)J"),
       reinterpret_cast<void*>(&PropertyGetIntMethod)},
      {const_cast<char*>("nGetMultipleFloatMethod"),
       const_cast<char*>("(Ljava/lang/Class;Ljava/lang/String;I)J"),
       reinterpret_cast<void*>(&PropertyGetMultipleFloatMethod)},
      {const_cast<char*>("nGetMultipleIntMethod"),
       const_cast<char*>("(Ljava/lang/Class;Ljava/lang/String;I)J"),
       reinterpret_cast<void*>(&PropertyGetMultipleIntMethod)},
  };
  if (!Register(env, "android/animation/PropertyValuesHolder",
                property_values_holder_methods,
                static_cast<jint>(std::size(property_values_holder_methods)))) {
    return false;
  }

  JNINativeMethod perfetto_category_methods[] = {
      {const_cast<char*>("native_init"),
       const_cast<char*>("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)J"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeInit)},
      {const_cast<char*>("native_delete"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeDelete)},
      {const_cast<char*>("native_get_extra_ptr"), const_cast<char*>("(J)J"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeGetExtraPtr)},
      {const_cast<char*>("native_register"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeRegister)},
      {const_cast<char*>("native_unregister"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeUnregister)},
      {const_cast<char*>("native_is_enabled"), const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&PerfettoCategoryNativeIsEnabled)},
  };
  if (!Register(env, "android/os/PerfettoTrace$Category",
                perfetto_category_methods,
                static_cast<jint>(std::size(perfetto_category_methods)))) {
    return false;
  }
  JNINativeMethod perfetto_extra_methods[] = {
      {const_cast<char*>("native_delete"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&PerfettoTrackEventExtraNativeDelete)},
  };
  if (!Register(env, "android/os/PerfettoTrackEventExtra", perfetto_extra_methods,
                static_cast<jint>(std::size(perfetto_extra_methods)))) {
    return false;
  }
  JNINativeMethod perfetto_producer_methods[] = {
      {const_cast<char*>("nativePerfettoProducerInit"),
       const_cast<char*>("(II)V"),
       reinterpret_cast<void*>(&PerfettoProducerNativeInit)},
  };
  if (!Register(env, "android/tracing/perfetto/Producer",
                perfetto_producer_methods,
                static_cast<jint>(std::size(perfetto_producer_methods)))) {
    return false;
  }
  JNINativeMethod perfetto_data_source_methods[] = {
      {const_cast<char*>("nativeCreate"),
       const_cast<char*>("(Landroid/tracing/perfetto/DataSource;Ljava/lang/String;)J"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeCreate)},
      {const_cast<char*>("nativeFlushAll"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeFlushAll)},
      {const_cast<char*>("nativeGetFinalizer"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeGetFinalizer)},
      {const_cast<char*>("nativeGetPerfettoDsInstanceIndex"),
       const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeGetInstanceIndex)},
      {const_cast<char*>("nativeGetPerfettoInstanceLocked"),
       const_cast<char*>("(JI)Landroid/tracing/perfetto/DataSourceInstance;"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeGetInstance)},
      {const_cast<char*>("nativePerfettoDsTraceIterateBegin"),
       const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeIterateBegin)},
      {const_cast<char*>("nativePerfettoDsTraceIterateBreak"),
       const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeIterateBreak)},
      {const_cast<char*>("nativePerfettoDsTraceIterateNext"),
       const_cast<char*>("(J)Z"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeIterateNext)},
      {const_cast<char*>("nativeRegisterDataSource"),
       const_cast<char*>("(JIZZ)V"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeRegister)},
      {const_cast<char*>("nativeReleasePerfettoInstanceLocked"),
       const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeReleaseInstance)},
      {const_cast<char*>("nativeWritePackets"), const_cast<char*>("(J[[B)V"),
       reinterpret_cast<void*>(&PerfettoDataSourceNativeWritePackets)},
  };
  return Register(env, "android/tracing/perfetto/DataSource",
                  perfetto_data_source_methods,
                  static_cast<jint>(std::size(perfetto_data_source_methods)));
}

}  // namespace darwin_art
