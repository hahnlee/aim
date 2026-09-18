#pragma once

#include <cstdint>
#include <memory>
#include <utility>

// Narrow owner-thread contract shared by the Android ALooper ABI and the
// NDK Choreographer owner. The callback payload remains opaque to this owner;
// Choreographer owns all typed callback interpretation.
namespace darwin_art::looper {
using TimedTaskCallback = void (*)(void*);
using TimedTaskRelease = void (*)(void*);
using FdCallback = int (*)(int, int, void*);
using OwnerRelease = void (*)(void*);

void* PrepareCurrent();
void* Current();
void Wake(void* looper);
// Signals the owner poll set. Returns 0 on success, or -errno when the
// provider could not publish the wake. ALooper_wake retains its ABI's void
// signature and delegates here.
int SignalWake(void* looper);
int PollCurrent(int timeout_ms);
int DrainCurrent();
int WaitCurrent(int timeout_ms);
int AddFd(void* looper, int fd, int ident, int events, FdCallback callback,
          void* data);
int AddFdOwned(void* looper, int fd, int ident, int events,
               FdCallback callback, void* data, void* owner,
               OwnerRelease release);
int RemoveFd(void* looper, int fd);
// Removes only the registration that still owns this exact fd/callback/data
// tuple. Returns one when removed, zero when it no longer matches, and -1 for
// invalid arguments. Any owner release runs after the looper mutex is dropped.
int RemoveFdIfOwned(void* looper, int fd, FdCallback expected_callback,
                    void* expected_data);
// The owner keeps the opaque payload alive through callback/reentrant work,
// then calls release. Failed enqueue also calls release immediately.
int ScheduleTimedTaskAt(void* looper, int64_t deadline_nanos,
                        TimedTaskCallback callback, void* context,
                        TimedTaskRelease release);
// Schedules a task with an opaque, nonzero token owned by this looper. The
// token is written while the queue mutex is held immediately before the task
// becomes visible to the owner thread. On failure, token_out is cleared and
// release(context) runs outside the queue mutex. Tokens are never reused;
// exhaustion rejects the task visibly rather than wrapping into zero.
int ScheduleTimedTaskAtOwned(void* looper, int64_t deadline_nanos,
                             TimedTaskCallback callback, void* context,
                             TimedTaskRelease release, uint64_t* token_out);
// Removes only a still-queued task carrying token. A zero return also covers
// a task already detached for callback/release work, and is not quiescence.
// On removal, release(context) runs before returning, outside the queue
// mutex, so reentrant owner calls are safe.
int CancelTimedTaskIfOwned(void* looper, uint64_t token);

// A reusable owner-thread task. Prepare is the only allocating operation: it
// builds the stable callback/context state before the task can be published.
// Request coalesces concurrent requests and is allocation-free after Prepare;
// the callback is always dispatched by the original ALooper owner thread.
struct ReusableLooperTaskCallbacks {
  TimedTaskCallback callback = nullptr;
  void* context = nullptr;
  std::shared_ptr<void> context_owner;
  // Once-only metadata completion, outside queue locks, after cancellation
  // and all admitted work has ended. May run on the cancelling thread for an
  // idle task; it must not do FD, Claim or JNI work. The context owner is pinned.
  void (*on_quiescent)(void*) noexcept = nullptr;
};

class ReusableLooperTask {
 public:
  struct State;

  static std::shared_ptr<ReusableLooperTask> Prepare(
      void* looper, const ReusableLooperTaskCallbacks& callbacks);
  static std::shared_ptr<ReusableLooperTask> Prepare(
      void* looper, TimedTaskCallback callback, void* context,
      std::shared_ptr<void> context_owner = {});

  ~ReusableLooperTask();
  ReusableLooperTask(const ReusableLooperTask&) = delete;
  ReusableLooperTask& operator=(const ReusableLooperTask&) = delete;

  // Returns false when cancelled/unavailable or when the owner wake failed. A
  // wake failure leaves the accepted request queued for a later retry. A true
  // result means the request was accepted or coalesced; it does not mean that
  // the callback has run.
  bool Request();

  // Irreversibly closes and unlinks the task without waiting for a callback.
  // In-flight work observes the close at completion and is never requeued.
  bool Cancel();

  // Reports actual quiescence, not merely cancellation acceptance.
  bool IsQuiescent() const;

 private:
  explicit ReusableLooperTask(std::shared_ptr<State> state);
  std::shared_ptr<State> state_;
};

using ReusableLooperTaskHandle = std::shared_ptr<ReusableLooperTask>;

inline ReusableLooperTaskHandle PrepareReusableTask(
    void* looper, const ReusableLooperTaskCallbacks& callbacks) {
  return ReusableLooperTask::Prepare(looper, callbacks);
}
inline ReusableLooperTaskHandle PrepareReusableTask(
    void* looper, TimedTaskCallback callback, void* context,
    std::shared_ptr<void> context_owner = {}) {
  return ReusableLooperTask::Prepare(looper, callback, context,
                                     std::move(context_owner));
}
}  // namespace darwin_art::looper
