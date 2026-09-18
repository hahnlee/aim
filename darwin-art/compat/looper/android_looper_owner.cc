#include "android_looper_owner.h"

#include "darwin_android_platform.h"
#include "darwin_android_time.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_art_bionic_errno.h"
#include "reusable_task.h"

#include <android/looper.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <climits>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <memory>
#include <mutex>
#include <new>
#include <optional>
#include <pthread.h>
#include <string>
#include <time.h>
#include <unistd.h>
#include <vector>

struct ALooperRegistration {
  int fd;
  int ident;
  int events;
  ALooper_callbackFunc callback;
  void* data;
  uint64_t generation = 0;
  uint64_t last_host_turn = 0;
  std::shared_ptr<void> callback_owner;
};

struct TimedTask {
  int64_t deadline_nanos = 0;
  darwin_art::looper::TimedTaskCallback callback = nullptr;
  void* context = nullptr;
  darwin_art::looper::TimedTaskRelease release = nullptr;
  uint64_t token = 0;
};

struct ALooper {
  std::atomic<uint32_t> references{1};
  int options = 0;
  int wake_fd = -1;
  uint64_t next_generation = 1;
  uint64_t next_timed_task_token = 1;
  std::mutex mutex;
  std::vector<ALooperRegistration> registrations;
  std::vector<TimedTask> timed_tasks;
  darwin_art::looper::detail::ReusableTaskQueue* reusable_tasks = nullptr;

  ~ALooper() {
    for (const TimedTask& task : timed_tasks) {
      if (task.release != nullptr) task.release(task.context);
    }
    darwin_art::looper::detail::DestroyReusableTaskQueue(reusable_tasks);
  }
};

thread_local uint32_t g_looper_callback_depth = 0;
thread_local ALooper* g_thread_looper = nullptr;
thread_local bool g_host_looper_turn_active = false;
thread_local uint64_t g_host_looper_turn = 0;

uint64_t CurrentThreadId() {
  uint64_t thread_id = 0;
  return pthread_threadid_np(nullptr, &thread_id) == 0 ? thread_id : 0;
}

uint64_t ThreadCpuNanos() {
  timespec value{};
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &value) != 0) return 0;
  return static_cast<uint64_t>(value.tv_sec) * UINT64_C(1000000000) +
         static_cast<uint64_t>(value.tv_nsec);
}

std::string NativeAddressDescription(const void* address) {
  if (address == nullptr) return "<null>";
  Dl_info info{};
  if (dladdr(address, &info) == 0 || info.dli_fname == nullptr) return "<guest>";
  const auto base = reinterpret_cast<uintptr_t>(info.dli_saddr);
  const auto value = reinterpret_cast<uintptr_t>(address);
  const auto offset = value >= base ? value - base : 0;
  char buffer[32];
  std::snprintf(buffer, sizeof(buffer), "%llx",
                static_cast<unsigned long long>(offset));
  return std::string(info.dli_fname) + "+0x" + buffer;
}

namespace {
int DispatchDueTimedTasks(ALooper* looper) {
  if (looper == nullptr) return 0;
  std::vector<TimedTask> due;
  // The guard releases every detached payload on normal completion or
  // callback unwind; callback exceptions intentionally propagate to the
  // owner-thread caller.
  struct DueTaskRelease {
    std::vector<TimedTask>& tasks;
    ~DueTaskRelease() noexcept {
      for (TimedTask& task : tasks) {
        if (task.release != nullptr) {
          auto release = task.release;
          task.release = nullptr;
          release(task.context);
        }
      }
    }
  } release_due{due};
  const auto now = darwin_art::AndroidUptimeNanos();
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    // Reserve while observing the queue under its mutex, before erasing any
    // task. No release callback runs under this lock.
    due.reserve(looper->timed_tasks.size());
    auto task = looper->timed_tasks.begin();
    while (task != looper->timed_tasks.end()) {
      if (task->deadline_nanos > now) {
        ++task;
        continue;
      }
      due.push_back(*task);
      task = looper->timed_tasks.erase(task);
    }
  }
  for (TimedTask& task : due) {
    if (task.callback != nullptr) task.callback(task.context);
    if (task.release != nullptr) {
      auto release = task.release;
      task.release = nullptr;
      release(task.context);
    }
  }
  return static_cast<int>(due.size());
}

int NextTimedTaskDelayMillis(ALooper* looper) {
  if (looper == nullptr) return -1;
  std::lock_guard<std::mutex> lock(looper->mutex);
  if (looper->timed_tasks.empty()) return -1;
  const auto next = std::min_element(
      looper->timed_tasks.begin(), looper->timed_tasks.end(),
      [](const auto& left, const auto& right) {
        return left.deadline_nanos < right.deadline_nanos;
      });
  const int64_t delay_nanos = next->deadline_nanos -
                              darwin_art::AndroidUptimeNanos();
  if (delay_nanos <= 0) return 0;
  // Preserve the old floor-plus-one behavior: this is intentionally not a
  // ceiling conversion, because the prior steady_clock implementation added
  // one millisecond after truncating fractional milliseconds.
  const int64_t milliseconds = delay_nanos / 1'000'000 + 1;
  return static_cast<int>(std::min<int64_t>(milliseconds, INT_MAX));
}

}

namespace darwin_art::looper::detail {
ReusableTaskQueue* ReusableTaskQueueForLooper(void* looper) {
  auto* owner = static_cast<ALooper*>(looper);
  return owner == nullptr ? nullptr : owner->reusable_tasks;
}
}  // namespace darwin_art::looper::detail

extern "C" ALooper* ALooper_forThread() { return g_thread_looper; }
extern "C" ALooper* ALooper_prepare(int options) {
  if (g_thread_looper == nullptr) {
    g_thread_looper = new (std::nothrow) ALooper();
    if (g_thread_looper != nullptr) {
      g_thread_looper->options = options;
      g_thread_looper->reusable_tasks =
          darwin_art::looper::detail::CreateReusableTaskQueue();
      if (g_thread_looper->reusable_tasks == nullptr) {
        delete g_thread_looper;
        g_thread_looper = nullptr;
        return nullptr;
      }
      // EFD_NONBLOCK | EFD_CLOEXEC in Android's ABI. The broker implements
      // eventfd over a datagram socketpair while preserving guest fd identity.
      g_thread_looper->wake_fd =
          darwin_art_bionic_socket_broker_eventfd(0, 0x80800);
      if (g_thread_looper->wake_fd < 0) {
        delete g_thread_looper;
        g_thread_looper = nullptr;
      }
    }
  }
  return g_thread_looper;
}
extern "C" void ALooper_acquire(ALooper* looper) {
  if (looper != nullptr) looper->references.fetch_add(1, std::memory_order_relaxed);
}
extern "C" void ALooper_release(ALooper* looper) {
  // The thread association is a process-lifetime safety reference. This
  // mirrors ART's other opaque compatibility tokens and prevents a borrowed
  // ALooper_forThread pointer from becoming dangling.
  if (looper != nullptr && looper->references.load(std::memory_order_acquire) > 1)
    looper->references.fetch_sub(1, std::memory_order_acq_rel);
}
int AddLooperFd(ALooper* looper, int fd, int ident, int events,
                ALooper_callbackFunc callback, void* data,
                std::shared_ptr<void> callback_owner) {
  if (looper == nullptr || fd < 0 ||
      (callback == nullptr &&
       (looper->options & ALOOPER_PREPARE_ALLOW_NON_CALLBACKS) == 0))
    return -1;
  std::optional<ALooperRegistration> replaced_registration;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
      const int status_flags =
          darwin_art_bionic_socket_broker_fcntl(fd, /*F_GETFL*/ 3, 0);
      std::fprintf(stderr,
                   "DARWIN_ART looper-add-fd tid=%llu looper=%p fd=%d ident=%d events=0x%x "
                   "callback=%p (%s) caller=%p (%s) data=%p status_flags=0x%x\n",
                   static_cast<unsigned long long>(CurrentThreadId()), looper,
                   fd, ident, events, reinterpret_cast<void*>(callback),
                   NativeAddressDescription(reinterpret_cast<void*>(callback)).c_str(),
                   __builtin_return_address(0),
                   NativeAddressDescription(__builtin_return_address(0)).c_str(),
                   data,
                   status_flags);
    }
    auto found = std::find_if(looper->registrations.begin(),
                              looper->registrations.end(),
                              [fd](const auto& value) { return value.fd == fd; });
    ALooperRegistration registration{
        fd, ident, events, callback, data, looper->next_generation++, 0,
        std::move(callback_owner)};
    if (found == looper->registrations.end())
      looper->registrations.push_back(registration);
    else {
      // Do not destroy the replaced owner while holding the looper mutex:
      // release callbacks are allowed to re-enter owner APIs.
      replaced_registration.emplace(std::move(*found));
      *found = std::move(registration);
      // `replaced_registration` is intentionally destroyed after this
      // critical section.
    }
  }
  // The registration can be published by a Binder or render worker while the
  // owner thread is blocked in an earlier poll snapshot. Android's Looper
  // wakes that poll so the next iteration observes the new request set.
  ALooper_wake(looper);
  return 1;
}
extern "C" int ALooper_addFd(ALooper* looper, int fd, int ident, int events,
                              ALooper_callbackFunc callback, void* data) {
  return AddLooperFd(looper, fd, ident, events, callback, data, {});
}

bool RemoveLooperFdGeneration(ALooper* looper, int fd, uint64_t generation) {
  if (looper == nullptr) return false;
  std::optional<ALooperRegistration> removed_registration;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    const auto found = std::find_if(
        looper->registrations.begin(), looper->registrations.end(),
        [fd, generation](const auto& value) {
          return value.fd == fd && value.generation == generation;
        });
    if (found != looper->registrations.end()) {
      removed_registration.emplace(std::move(*found));
      looper->registrations.erase(found);
    }
  }
  if (!removed_registration.has_value()) return false;
  ALooper_wake(looper);
  return true;
}
extern "C" int ALooper_removeFd(ALooper* looper, int fd) {
  if (looper == nullptr) return -1;
  std::vector<ALooperRegistration> removed_registrations;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    for (auto registration = looper->registrations.begin();
         registration != looper->registrations.end();) {
      if (registration->fd != fd) {
        ++registration;
        continue;
      }
      removed_registrations.push_back(std::move(*registration));
      registration = looper->registrations.erase(registration);
    }
  }
  if (removed_registrations.empty()) return 0;
  ALooper_wake(looper);
  return 1;
}
namespace darwin_art::looper {
int SignalWake(void* opaque_looper) {
  auto* looper = static_cast<ALooper*>(opaque_looper);
  if (looper == nullptr || looper->wake_fd < 0) return -EINVAL;
  const uint64_t value = 1;
  for (int attempt = 0; attempt != 2; ++attempt) {
    const intptr_t written = darwin_art_bionic_socket_broker_write(
        looper->wake_fd, &value, sizeof(value));
    if (written == static_cast<intptr_t>(sizeof(value))) return 0;
    const int loaded_error = darwin_art_bionic_errno_load();
    const int error = loaded_error > 0 ? loaded_error : EIO;
    if (error != EINTR) return -error;
  }
  return -EINTR;
}
}  // namespace darwin_art::looper
extern "C" void ALooper_wake(ALooper* looper) {
  darwin_art::looper::Wake(looper);
}
extern "C" int ALooper_pollOnce(int timeout_ms, int* out_fd,
                                 int* out_events, void** out_data) {
  if (out_fd != nullptr) *out_fd = 0;
  if (out_events != nullptr) *out_events = 0;
  if (out_data != nullptr) *out_data = nullptr;
  ALooper* looper = g_thread_looper;
  if (looper == nullptr) return ALOOPER_POLL_ERROR;
  if (g_looper_callback_depth != 0 &&
      std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN_ART nested-looper-poll depth=%u timeout_ms=%d\n",
                 g_looper_callback_depth, timeout_ms);
  }
  const int reusable_tasks =
      darwin_art::looper::detail::DispatchReusableTasks(
          looper->reusable_tasks);
  if (reusable_tasks > 0) timeout_ms = 0;
  const int due_timed_tasks = DispatchDueTimedTasks(looper);
  if (due_timed_tasks > 0) timeout_ms = 0;
  const int frame_delay = NextTimedTaskDelayMillis(looper);
  if (frame_delay >= 0 && (timeout_ms < 0 || frame_delay < timeout_ms))
    timeout_ms = frame_delay;
  std::vector<ALooperRegistration> registrations;
  {
    std::lock_guard<std::mutex> lock(looper->mutex);
    registrations = looper->registrations;
  }
  std::vector<DarwinArtBionicPollFd> descriptors;
  descriptors.reserve(registrations.size() + 1);
  descriptors.push_back({looper->wake_fd, 0x0001, 0});
  for (const auto& registration : registrations) {
    int16_t poll_events = 0;
    if ((registration.events & ALOOPER_EVENT_INPUT) != 0) poll_events |= 0x0001;
    if ((registration.events & ALOOPER_EVENT_OUTPUT) != 0) poll_events |= 0x0004;
    descriptors.push_back({registration.fd, poll_events, 0});
  }
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    static thread_local size_t logged_registration_count = 0;
    if (logged_registration_count != registrations.size()) {
      logged_registration_count = registrations.size();
      std::fprintf(stderr,
                   "ART Android Looper poll-set tid=%llu looper=%p wake_fd=%d registrations=%zu",
                   static_cast<unsigned long long>(CurrentThreadId()), looper,
                   looper->wake_fd, registrations.size());
      for (const auto& registration : registrations) {
        std::fprintf(stderr, " fd=%d", registration.fd);
      }
      std::fputc('\n', stderr);
    }
  }
  int ready = -1;
  do {
    ready = darwin_art_bionic_socket_broker_poll(
        descriptors.data(), descriptors.size(), timeout_ms);
  } while (ready < 0 && errno == EINTR);
  if (ready > 0 &&
      std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr, "ART Android Looper poll-ready tid=%llu ready=%d",
                 static_cast<unsigned long long>(CurrentThreadId()), ready);
    for (const auto& descriptor : descriptors) {
      if (descriptor.revents != 0) {
        std::fprintf(stderr, " fd=%d revents=0x%x", descriptor.fd,
                     descriptor.revents);
      }
    }
    std::fputc('\n', stderr);
  }
  if (ready < 0) return ALOOPER_POLL_ERROR;
  if (ready == 0) {
    const int late_timed_tasks = DispatchDueTimedTasks(looper);
    return (reusable_tasks > 0 || due_timed_tasks > 0 || late_timed_tasks > 0)
               ? ALOOPER_POLL_CALLBACK
               : ALOOPER_POLL_TIMEOUT;
  }
  const bool received_wake = (descriptors[0].revents & 0x0001) != 0;
  if (received_wake) {
    uint64_t value = 0;
    (void)darwin_art_bionic_socket_broker_read(looper->wake_fd, &value,
                                                sizeof(value));
  }
  bool invoked_callback = false;
  bool skipped_host_callback = false;
  for (size_t index = 1; index < descriptors.size(); ++index) {
    const int16_t poll_events = descriptors[index].revents;
    if (poll_events == 0) continue;
    int events = 0;
    if ((poll_events & 0x0001) != 0) events |= ALOOPER_EVENT_INPUT;
    if ((poll_events & 0x0004) != 0) events |= ALOOPER_EVENT_OUTPUT;
    if ((poll_events & 0x0008) != 0) events |= ALOOPER_EVENT_ERROR;
    if ((poll_events & 0x0010) != 0) events |= ALOOPER_EVENT_HANGUP;
    if ((poll_events & 0x0020) != 0) events |= ALOOPER_EVENT_INVALID;
    const auto& registration = registrations[index - 1];
    if (registration.callback != nullptr) {
      bool registration_current = false;
      bool already_dispatched = false;
      {
        std::lock_guard<std::mutex> lock(looper->mutex);
        const auto found = std::find_if(
            looper->registrations.begin(), looper->registrations.end(),
            [&registration](const auto& current) {
              return current.fd == registration.fd &&
                     current.generation == registration.generation;
            });
        if (found != looper->registrations.end()) {
          registration_current = true;
          if (g_host_looper_turn_active) {
            already_dispatched = found->last_host_turn == g_host_looper_turn;
            if (!already_dispatched)
              found->last_host_turn = g_host_looper_turn;
          }
        }
      }
      if (!registration_current) continue;
      if (already_dispatched) {
        skipped_host_callback = true;
        continue;
      }
      invoked_callback = true;
      if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr &&
          std::getenv("DARWIN_ART_DEBUG_CALLBACK_VTABLE") != nullptr) {
        // Chromium's NativeChildProcessService callback is a small guest
        // thunk that dispatches through the service object's vtable. Read the
        // same slot used by that thunk so a slow callback can be mapped back
        // to its concrete method without changing callback affinity.
        uintptr_t object_vtable = 0;
        if (registration.data != nullptr) {
          std::memcpy(&object_vtable, registration.data,
                      sizeof(object_vtable));
        }
        int32_t vtable_offset = 0;
        if (object_vtable != 0) {
          std::memcpy(&vtable_offset,
                      reinterpret_cast<const void*>(object_vtable + 0x2c),
                      sizeof(vtable_offset));
        }
        const uintptr_t callback_target =
            object_vtable == 0
                ? 0
                : object_vtable + static_cast<int64_t>(vtable_offset);
        std::fprintf(stderr,
                     "DARWIN_ART looper-callback-target pid=%d process=%s "
                     "tid=%llu fd=%d callback=%p data=%p vtable=%p "
                     "slot_offset=%d target=%p\n",
                     getpid(),
                     std::getenv("DARWIN_ART_APK_PROCESS_NAME") == nullptr
                         ? "<main>"
                         : std::getenv("DARWIN_ART_APK_PROCESS_NAME"),
                     static_cast<unsigned long long>(CurrentThreadId()),
                     registration.fd,
                     reinterpret_cast<void*>(registration.callback),
                     registration.data,
                     reinterpret_cast<void*>(object_vtable), vtable_offset,
                     reinterpret_cast<void*>(callback_target));
      }
      const auto callback_started = std::chrono::steady_clock::now();
      const uint64_t callback_cpu_started = ThreadCpuNanos();
      ++g_looper_callback_depth;
      const int callback_result =
          registration.callback(registration.fd, events, registration.data);
      --g_looper_callback_depth;
      if (std::getenv("DARWIN_ART_DEBUG_SLOW_FRAME") != nullptr) {
        const auto callback_us =
            std::chrono::duration_cast<std::chrono::microseconds>(
                std::chrono::steady_clock::now() - callback_started)
                .count();
        if (callback_us >= 100'000) {
          const uint64_t callback_cpu_finished = ThreadCpuNanos();
          const uint64_t callback_cpu_us =
              callback_cpu_finished >= callback_cpu_started
                  ? (callback_cpu_finished - callback_cpu_started) / 1000
                  : 0;
          std::fprintf(stderr,
                       "DARWIN_ART slow-native-callback pid=%d process=%s "
                       "tid=%llu fd=%d events=0x%x "
                       "callback=%p data=%p result=%d elapsed_us=%lld "
                       "cpu_us=%llu callback_desc=%s\n",
                       getpid(),
                       std::getenv("DARWIN_ART_APK_PROCESS_NAME") == nullptr
                           ? "<main>"
                           : std::getenv("DARWIN_ART_APK_PROCESS_NAME"),
                       static_cast<unsigned long long>(CurrentThreadId()),
                       registration.fd, events,
                       reinterpret_cast<void*>(registration.callback),
                       registration.data, callback_result,
                       static_cast<long long>(callback_us),
                       static_cast<unsigned long long>(callback_cpu_us),
                       NativeAddressDescription(reinterpret_cast<void*>(
                           registration.callback)).c_str());
        }
      }
      if (callback_result == 0 ||
          (events & ALOOPER_EVENT_INVALID) != 0)
        (void)RemoveLooperFdGeneration(looper, registration.fd,
                                       registration.generation);
      // AOSP Looper::pollInner walks every callback response captured by the
      // same poll before returning POLL_CALLBACK. Discarding the remainder
      // lets an always-ready earlier descriptor starve InputChannel forever.
      // The outer host drain still bounds independent poll iterations.
      continue;
    } else {
      if (out_fd != nullptr) *out_fd = registration.fd;
      if (out_events != nullptr) *out_events = events;
      if (out_data != nullptr) *out_data = registration.data;
      return registration.ident;
    }
  }
  if (invoked_callback || reusable_tasks > 0 || due_timed_tasks > 0)
    return ALOOPER_POLL_CALLBACK;
  if (skipped_host_callback) return ALOOPER_POLL_TIMEOUT;
  return received_wake ? ALOOPER_POLL_WAKE : ALOOPER_POLL_TIMEOUT;
}

namespace darwin_art::looper {
void* Current() { return g_thread_looper; }
void* PrepareCurrent() { return ALooper_prepare(0); }
void Wake(void* looper) { (void)SignalWake(looper); }
int PollCurrent(int timeout_ms) {
  return ALooper_pollOnce(timeout_ms, nullptr, nullptr, nullptr);
}
int DrainCurrent() {
  if (g_thread_looper == nullptr) return 0;
  int dispatched = 0;
  ++g_host_looper_turn;
  g_host_looper_turn_active = true;
  constexpr int kHostCallbackBudget = 8;
  int status = 0;
  for (int iteration = 0; iteration < kHostCallbackBudget; ++iteration) {
    const int result = ALooper_pollOnce(0, nullptr, nullptr, nullptr);
    if (result == ALOOPER_POLL_CALLBACK || result == ALOOPER_POLL_WAKE) {
      ++dispatched;
      continue;
    }
    if (result == ALOOPER_POLL_TIMEOUT) {
      status = dispatched;
      break;
    }
    status = result == ALOOPER_POLL_ERROR ? -1 : dispatched;
    break;
  }
  g_host_looper_turn_active = false;
  return status == 0 ? dispatched : status;
}
int WaitCurrent(int timeout_ms) {
  if (g_thread_looper == nullptr) return 0;
  timeout_ms = std::clamp(timeout_ms, 0, 16);
  ++g_host_looper_turn;
  g_host_looper_turn_active = true;
  const int result = ALooper_pollOnce(timeout_ms, nullptr, nullptr, nullptr);
  g_host_looper_turn_active = false;
  return result == ALOOPER_POLL_ERROR ? -1 : 0;
}
int AddFd(void* looper, int fd, int ident, int events, FdCallback callback, void* data) {
  if (looper == nullptr || callback == nullptr) return 0;
  return ALooper_addFd(static_cast<ALooper*>(looper), fd, ident, events,
                       callback, data);
}
int AddFdOwned(void* looper, int fd, int ident, int events, FdCallback callback,
               void* data, void* owner, OwnerRelease release) {
  if (looper == nullptr || callback == nullptr || owner == nullptr ||
      release == nullptr) {
    if (owner != nullptr && release != nullptr) release(owner);
    return 0;
  }
  std::shared_ptr<void> callback_owner(owner, [release](void* value) {
    if (value != nullptr) release(value);
  });
  return AddLooperFd(static_cast<ALooper*>(looper), fd, ident, events,
                     callback, data, std::move(callback_owner));
}
int RemoveFd(void* looper, int fd) {
  return looper == nullptr ? 0 : ALooper_removeFd(static_cast<ALooper*>(looper), fd);
}
int RemoveFdIfOwned(void* looper, int fd, FdCallback expected_callback,
                    void* expected_data) {
  if (looper == nullptr || fd < 0 || expected_callback == nullptr) return -1;
  auto* owner = static_cast<ALooper*>(looper);
  std::optional<ALooperRegistration> removed_registration;
  {
    std::lock_guard<std::mutex> lock(owner->mutex);
    const auto found = std::find_if(
        owner->registrations.begin(), owner->registrations.end(),
        [fd, expected_callback, expected_data](const auto& value) {
          return value.fd == fd && value.callback == expected_callback &&
                 value.data == expected_data;
        });
    if (found == owner->registrations.end()) return 0;
    removed_registration.emplace(std::move(*found));
    owner->registrations.erase(found);
  }
  ALooper_wake(owner);
  return 1;
}
int ScheduleTimedTaskAt(void* looper, int64_t deadline_nanos,
                        TimedTaskCallback callback, void* context,
                        TimedTaskRelease release) {
  auto* owner = static_cast<ALooper*>(looper);
  if (owner == nullptr || callback == nullptr || release == nullptr) {
    if (release != nullptr) release(context);
    return 0;
  }
  const TimedTask task{deadline_nanos, callback, context, release};
  {
    try {
      std::lock_guard<std::mutex> lock(owner->mutex);
      owner->timed_tasks.push_back(task);
    } catch (...) {
      release(context);
      return 0;
    }
  }
  ALooper_wake(owner);
  return 1;
}

int ScheduleTimedTaskAtOwned(void* looper, int64_t deadline_nanos,
                             TimedTaskCallback callback, void* context,
                             TimedTaskRelease release, uint64_t* token_out) {
  if (token_out != nullptr) *token_out = 0;
  auto* owner = static_cast<ALooper*>(looper);
  if (owner == nullptr || callback == nullptr || release == nullptr ||
      token_out == nullptr) {
    if (release != nullptr) release(context);
    return 0;
  }

  uint64_t token = 0;
  bool queued = false;
  bool exhausted = false;
  {
    std::lock_guard<std::mutex> lock(owner->mutex);
    token = owner->next_timed_task_token;
    if (token == 0) {
      exhausted = true;
    } else {
      try {
        owner->timed_tasks.push_back(
            TimedTask{deadline_nanos, callback, context, release, token});
        // The output is published under the same mutex immediately before
        // the queue entry becomes visible after unlock.
        *token_out = token;
        owner->next_timed_task_token =
            token == UINT64_MAX ? 0 : token + 1;
        queued = true;
      } catch (...) {
        // The release tail below runs after this mutex is dropped.
      }
    }
  }
  if (!queued) {
    if (exhausted) errno = EOVERFLOW;
    release(context);
    return 0;
  }
  ALooper_wake(owner);
  return 1;
}

int CancelTimedTaskIfOwned(void* looper, uint64_t token) {
  auto* owner = static_cast<ALooper*>(looper);
  if (owner == nullptr || token == 0) return 0;
  std::optional<TimedTask> removed;
  {
    std::lock_guard<std::mutex> lock(owner->mutex);
    const auto found = std::find_if(
        owner->timed_tasks.begin(), owner->timed_tasks.end(),
        [token](const TimedTask& task) { return task.token == token; });
    if (found == owner->timed_tasks.end()) return 0;
    removed.emplace(std::move(*found));
    owner->timed_tasks.erase(found);
  }
  if (removed->release != nullptr) {
    auto release = removed->release;
    removed->release = nullptr;
    release(removed->context);
  }
  ALooper_wake(owner);
  return 1;
}

}  // namespace darwin_art::looper

extern "C" int darwin_art_android_platform_poll_current_looper() {
  return darwin_art::looper::DrainCurrent();
}
extern "C" void* darwin_art_android_platform_prepare_current_looper() {
  return darwin_art::looper::PrepareCurrent();
}
extern "C" int darwin_art_android_platform_poll_current_looper_timeout(int timeout_ms) {
  return darwin_art::looper::PollCurrent(timeout_ms);
}
extern "C" int darwin_art_android_platform_wait_current_looper(int timeout_ms) {
  return darwin_art::looper::WaitCurrent(timeout_ms);
}
extern "C" void darwin_art_android_platform_wake_looper(void* looper) {
  darwin_art::looper::Wake(looper);
}
extern "C" int darwin_art_android_platform_add_fd(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data) {
  return darwin_art::looper::AddFd(looper, fd, ident, events, callback, data);
}
extern "C" int darwin_art_android_platform_add_fd_owned(
    void* looper, int fd, int ident, int events,
    DarwinArtLooperCallback callback, void* data, void* owner,
    DarwinArtLooperOwnerRelease release) {
  return darwin_art::looper::AddFdOwned(looper, fd, ident, events, callback,
                                        data, owner, release);
}
extern "C" int darwin_art_android_platform_remove_fd(void* looper, int fd) {
  return darwin_art::looper::RemoveFd(looper, fd);
}
