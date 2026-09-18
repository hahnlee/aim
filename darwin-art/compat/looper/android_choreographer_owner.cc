#include "android_looper_owner.h"

#include "darwin_android_time.h"

#include <android/choreographer.h>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <new>
#include <pthread.h>
#include <utility>
#include <unistd.h>
#include <vector>

enum class ChoreographerCallbackKind {
  kFrame,
  kFrame64,
  kVsync,
  kRefreshRate,
};

struct AChoreographer {
  void* looper = nullptr;
  std::mutex mutex;
  std::vector<std::pair<AChoreographer_refreshRateCallback, void*>>
      refresh_callbacks;
};

struct AChoreographerFrameCallbackData {
  int64_t frame_time_nanos = 0;
  AVsyncId vsync_id = 0;
  int64_t expected_presentation_time_nanos = 0;
  int64_t deadline_nanos = 0;
};

namespace {

struct CallbackTask {
  ChoreographerCallbackKind kind;
  void* callback;
  void* data;
  void* looper;
};

int64_t MonotonicNanos() { return darwin_art::AndroidUptimeNanos(); }

uint64_t CurrentThreadId() {
  uint64_t thread_id = 0;
  return pthread_threadid_np(nullptr, &thread_id) == 0 ? thread_id : 0;
}

void DispatchCallbackTask(void* opaque) {
  auto* task = static_cast<CallbackTask*>(opaque);
  if (task == nullptr || task->callback == nullptr) return;
  static std::atomic<AVsyncId> next_vsync_id{1};
  static std::atomic<uint32_t> dispatched_count{0};
  const int64_t frame_time = MonotonicNanos();
  const uint32_t sequence =
      dispatched_count.fetch_add(1, std::memory_order_relaxed) + 1;
  if (std::getenv("DARWIN_ART_DEBUG_CHOREOGRAPHER") != nullptr &&
      sequence <= 64) {
    std::fprintf(stderr,
                 "ART Android Choreographer: dispatch sequence=%u kind=%u "
                 "frame_ns=%lld tid=%llu looper=%p callback=%p data=%p "
                 "pid=%d\n",
                 sequence, static_cast<unsigned>(task->kind),
                 static_cast<long long>(frame_time),
                 static_cast<unsigned long long>(CurrentThreadId()),
                 task->looper, task->callback, task->data, getpid());
  }
  switch (task->kind) {
    case ChoreographerCallbackKind::kFrame:
      reinterpret_cast<AChoreographer_frameCallback>(task->callback)(
          static_cast<long>(frame_time), task->data);
      break;
    case ChoreographerCallbackKind::kFrame64:
      reinterpret_cast<AChoreographer_frameCallback64>(task->callback)(
          frame_time, task->data);
      break;
    case ChoreographerCallbackKind::kVsync: {
      AChoreographerFrameCallbackData data{
          frame_time, next_vsync_id.fetch_add(1, std::memory_order_relaxed),
          frame_time + 16'666'667, frame_time + 15'000'000};
      reinterpret_cast<AChoreographer_vsyncCallback>(task->callback)(
          &data, task->data);
      break;
    }
    case ChoreographerCallbackKind::kRefreshRate:
      reinterpret_cast<AChoreographer_refreshRateCallback>(task->callback)(
          16'666'667, task->data);
      break;
  }
}

void Schedule(AChoreographer* choreographer, ChoreographerCallbackKind kind,
              void* callback, void* data, uint32_t delay_millis) {
  static std::atomic<uint32_t> scheduled_count{0};
  const uint32_t sequence =
      scheduled_count.fetch_add(1, std::memory_order_relaxed) + 1;
  const bool debug =
      std::getenv("DARWIN_ART_DEBUG_CHOREOGRAPHER") != nullptr;
  if (choreographer == nullptr || choreographer->looper == nullptr ||
      callback == nullptr) {
    if (debug && sequence <= 64) {
      const char* reason = choreographer == nullptr
                               ? "choreographer-null"
                               : choreographer->looper == nullptr
                                   ? "looper-null"
                                   : "callback-null";
      std::fprintf(stderr,
                   "ART Android Choreographer: schedule-skipped "
                   "sequence=%u kind=%u reason=%s tid=%llu looper=%p "
                   "callback=%p data=%p pid=%d\n",
                   sequence, static_cast<unsigned>(kind), reason,
                   static_cast<unsigned long long>(CurrentThreadId()),
                   choreographer == nullptr ? nullptr : choreographer->looper,
                   callback, data, getpid());
    }
    return;
  }
  if (delay_millis == 0) delay_millis = 16;
  auto* task =
      new (std::nothrow) CallbackTask{kind, callback, data, choreographer->looper};
  if (task == nullptr) {
    if (debug && sequence <= 64) {
      std::fprintf(stderr,
                   "ART Android Choreographer: schedule-skipped "
                   "sequence=%u kind=%u reason=allocation-failed tid=%llu "
                   "looper=%p callback=%p data=%p pid=%d\n",
                   sequence, static_cast<unsigned>(kind),
                   static_cast<unsigned long long>(CurrentThreadId()),
                   choreographer->looper, callback, data, getpid());
    }
    return;
  }
  const int64_t deadline = MonotonicNanos() +
                           static_cast<int64_t>(delay_millis) * 1'000'000;
  // The owner invokes this release after dispatch, and also on enqueue
  // failure, so payload ownership has one unambiguous path.
  const int accepted = darwin_art::looper::ScheduleTimedTaskAt(
      choreographer->looper, deadline, DispatchCallbackTask, task,
      [](void* value) { delete static_cast<CallbackTask*>(value); });
  if (debug && sequence <= 64) {
    std::fprintf(stderr,
                 "ART Android Choreographer: schedule sequence=%u kind=%u "
                 "delay_ms=%u deadline_ns=%lld accepted=%d tid=%llu "
                 "looper=%p callback=%p data=%p pid=%d\n",
                 sequence, static_cast<unsigned>(kind), delay_millis,
                 static_cast<long long>(deadline), accepted,
                 static_cast<unsigned long long>(CurrentThreadId()),
                 choreographer->looper, callback, data, getpid());
  }
}

}  // namespace

extern "C" AChoreographer* AChoreographer_getInstance() {
  static thread_local AChoreographer* choreographer = nullptr;
  static std::atomic<uint32_t> get_instance_count{0};
  const uint32_t sequence =
      get_instance_count.fetch_add(1, std::memory_order_relaxed) + 1;
  const bool debug =
      std::getenv("DARWIN_ART_DEBUG_CHOREOGRAPHER") != nullptr;
  void* looper = darwin_art::looper::Current();
  if (looper == nullptr) {
    if (debug && sequence <= 64) {
      std::fprintf(stderr,
                   "ART Android Choreographer: get-instance sequence=%u "
                   "tid=%llu looper=null result=null pid=%d\n",
                   sequence, static_cast<unsigned long long>(CurrentThreadId()),
                   getpid());
    }
    return nullptr;
  }
  if (choreographer == nullptr) {
    choreographer = new (std::nothrow) AChoreographer();
    if (choreographer != nullptr) choreographer->looper = looper;
  }
  if (debug && sequence <= 64) {
    std::fprintf(stderr,
                 "ART Android Choreographer: get-instance sequence=%u "
                 "tid=%llu looper=%p result=%p pid=%d\n",
                 sequence, static_cast<unsigned long long>(CurrentThreadId()),
                 looper, static_cast<void*>(choreographer), getpid());
  }
  return choreographer;
}

extern "C" void AChoreographer_postFrameCallback(
    AChoreographer* choreographer, AChoreographer_frameCallback callback,
    void* data) {
  Schedule(choreographer, ChoreographerCallbackKind::kFrame,
           reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_postFrameCallbackDelayed(
    AChoreographer* choreographer, AChoreographer_frameCallback callback,
    void* data, long delay_millis) {
  Schedule(choreographer, ChoreographerCallbackKind::kFrame,
           reinterpret_cast<void*>(callback), data,
           delay_millis <= 0 ? 0 : static_cast<uint32_t>(delay_millis));
}

extern "C" void AChoreographer_postFrameCallback64(
    AChoreographer* choreographer, AChoreographer_frameCallback64 callback,
    void* data) {
  Schedule(choreographer, ChoreographerCallbackKind::kFrame64,
           reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_postFrameCallbackDelayed64(
    AChoreographer* choreographer, AChoreographer_frameCallback64 callback,
    void* data, uint32_t delay_millis) {
  Schedule(choreographer, ChoreographerCallbackKind::kFrame64,
           reinterpret_cast<void*>(callback), data, delay_millis);
}

extern "C" void AChoreographer_postVsyncCallback(
    AChoreographer* choreographer, AChoreographer_vsyncCallback callback,
    void* data) {
  Schedule(choreographer, ChoreographerCallbackKind::kVsync,
           reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_registerRefreshRateCallback(
    AChoreographer* choreographer,
    AChoreographer_refreshRateCallback callback, void* data) {
  if (choreographer == nullptr || callback == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(choreographer->mutex);
    choreographer->refresh_callbacks.emplace_back(callback, data);
  }
  Schedule(choreographer, ChoreographerCallbackKind::kRefreshRate,
           reinterpret_cast<void*>(callback), data, 0);
}

extern "C" void AChoreographer_unregisterRefreshRateCallback(
    AChoreographer* choreographer,
    AChoreographer_refreshRateCallback callback, void* data) {
  if (choreographer == nullptr) return;
  std::lock_guard<std::mutex> lock(choreographer->mutex);
  std::erase(choreographer->refresh_callbacks, std::make_pair(callback, data));
}

extern "C" int64_t AChoreographerFrameCallbackData_getFrameTimeNanos(
    const AChoreographerFrameCallbackData* data) {
  return data == nullptr ? 0 : data->frame_time_nanos;
}
extern "C" size_t AChoreographerFrameCallbackData_getFrameTimelinesLength(
    const AChoreographerFrameCallbackData*) {
  return 1;
}
extern "C" size_t AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex(
    const AChoreographerFrameCallbackData*) {
  return 0;
}
extern "C" AVsyncId AChoreographerFrameCallbackData_getFrameTimelineVsyncId(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? -1 : data->vsync_id;
}
extern "C" int64_t
AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? 0
                                      : data->expected_presentation_time_nanos;
}
extern "C" int64_t
AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos(
    const AChoreographerFrameCallbackData* data, size_t index) {
  return data == nullptr || index != 0 ? 0 : data->deadline_nanos;
}
