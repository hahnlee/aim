#include "darwin_art_bionic_socket_broker.h"
#include "looper/android_looper_owner.h"

#include <android/looper.h>

#include <atomic>
#include <cassert>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdlib>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <new>
#include <poll.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace {

std::atomic<bool> g_count_allocations{false};
std::atomic<unsigned> g_allocations{0};
std::atomic<int> g_fail_after{-1};

void* Allocate(std::size_t size) {
  if (g_fail_after.load(std::memory_order_relaxed) >= 0 &&
      g_fail_after.fetch_sub(1, std::memory_order_relaxed) == 0)
    throw std::bad_alloc();
  void* result = std::malloc(size == 0 ? 1 : size);
  if (result == nullptr) throw std::bad_alloc();
  if (g_count_allocations.load(std::memory_order_relaxed))
    g_allocations.fetch_add(1, std::memory_order_relaxed);
  return result;
}

std::mutex g_wake_mutex;
std::map<int, int> g_wake_writers;
std::atomic<bool> g_fail_wake{false};
std::atomic<int> g_bionic_errno{0};

struct CallbackState {
  darwin_art::looper::ReusableLooperTaskHandle task;
  std::thread::id owner;
  std::atomic<int> calls{0};
  std::atomic<bool> wrong_thread{false};
  std::atomic<bool> nested_poll{false};
};

void ReentrantCallback(void* opaque) {
  auto* state = static_cast<CallbackState*>(opaque);
  if (std::this_thread::get_id() != state->owner)
    state->wrong_thread.store(true, std::memory_order_release);
  const int call = state->calls.fetch_add(1, std::memory_order_acq_rel) + 1;
  if (call == 1) {
    assert(state->task->Request());
    const int nested = ALooper_pollOnce(0, nullptr, nullptr, nullptr);
    assert(nested != ALOOPER_POLL_CALLBACK);
    state->nested_poll.store(true, std::memory_order_release);
  }
}

struct BlockingState {
  std::mutex mutex;
  std::condition_variable condition;
  bool entered = false;
  bool release = false;
  int calls = 0;
  darwin_art::looper::ReusableLooperTaskHandle task;
  std::atomic<int> completions{0};
};

void BlockingCallback(void* opaque) {
  auto* state = static_cast<BlockingState*>(opaque);
  std::unique_lock<std::mutex> lock(state->mutex);
  ++state->calls;
  state->entered = true;
  state->condition.notify_all();
  state->condition.wait(lock, [state] { return state->release; });
}

void BlockingQuiescence(void* opaque) noexcept {
  auto* state = static_cast<BlockingState*>(opaque);
  assert(state->task->IsQuiescent());
  state->completions.fetch_add(1, std::memory_order_relaxed);
  assert(state->task->Cancel());
}

struct Counter {
  std::atomic<int> value{0};
};

void CountCallback(void* opaque) {
  static_cast<Counter*>(opaque)->value.fetch_add(1, std::memory_order_relaxed);
}

void CountMetadata(void* opaque) noexcept { CountCallback(opaque); }

struct QuiescenceState {
  darwin_art::looper::ReusableLooperTaskHandle task;
  int completions = 0;
  bool throw_in_callback = false;
};

void CloseInCallback(void* opaque) {
  auto* state = static_cast<QuiescenceState*>(opaque);
  assert(state->task->Cancel());
  assert(!state->task->IsQuiescent());
  assert(state->completions == 0);
  if (state->throw_in_callback) throw std::bad_alloc();
}

void OnQuiescent(void* opaque) noexcept {
  auto* state = static_cast<QuiescenceState*>(opaque);
  assert(state->task->IsQuiescent());
  ++state->completions;
  assert(state->task->Cancel());
  assert(!state->task->Request());
}

void CountRelease(void* opaque) {
  static_cast<Counter*>(opaque)->value.fetch_add(1, std::memory_order_relaxed);
}

int CountFd(int, int, void* opaque) {
  static_cast<Counter*>(opaque)->value.fetch_add(1, std::memory_order_relaxed);
  return 0;
}

int64_t MonotonicNanos() {
  timespec value{};
  assert(clock_gettime(CLOCK_MONOTONIC, &value) == 0);
  return static_cast<int64_t>(value.tv_sec) * 1'000'000'000 + value.tv_nsec;
}

}  // namespace

void* operator new(std::size_t size) { return Allocate(size); }
void* operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void* value) noexcept { std::free(value); }
void operator delete[](void* value) noexcept { std::free(value); }
void operator delete(void* value, std::size_t) noexcept { std::free(value); }
void operator delete[](void* value, std::size_t) noexcept { std::free(value); }

extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t, int) {
  int descriptors[2] = {-1, -1};
  assert(pipe(descriptors) == 0);
  std::lock_guard<std::mutex> lock(g_wake_mutex);
  g_wake_writers.emplace(descriptors[0], descriptors[1]);
  return descriptors[0];
}

extern "C" intptr_t darwin_art_bionic_socket_broker_read(int fd, void* bytes,
                                                            size_t count) {
  return read(fd, bytes, count);
}

extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t count) {
  if (g_fail_wake.load(std::memory_order_acquire)) {
    g_bionic_errno.store(5, std::memory_order_release);
    errno = EIO;
    return -1;
  }
  std::lock_guard<std::mutex> lock(g_wake_mutex);
  const auto found = g_wake_writers.find(fd);
  return found == g_wake_writers.end() ? -1 : write(found->second, bytes, count);
}

extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout_ms) {
  std::vector<pollfd> host_descriptors;
  host_descriptors.reserve(count);
  for (size_t i = 0; i < count; ++i)
    host_descriptors.push_back({descriptors[i].fd, descriptors[i].events, 0});
  const int result = ::poll(host_descriptors.data(), host_descriptors.size(),
                            timeout_ms);
  if (result < 0) return result;
  int ready = 0;
  for (size_t i = 0; i < count; ++i) {
    descriptors[i].revents = host_descriptors[i].revents;
    if (descriptors[i].revents != 0) ++ready;
  }
  return ready;
}

extern "C" int darwin_art_bionic_socket_broker_fcntl(int fd, int command,
                                                       intptr_t argument) {
  return fcntl(fd, command, argument);
}

extern "C" int darwin_art_bionic_errno_load() {
  return g_bionic_errno.load(std::memory_order_acquire);
}

int main() {
  alarm(15);
  void* looper = darwin_art::looper::PrepareCurrent();
  assert(looper != nullptr);

  // Failure at any preparation allocation must not expose a partial task or
  // announce cancellation of a task whose handle was never returned.
  for (int allocation = 0; allocation != 4; ++allocation) {
    Counter unpublished;
    g_fail_after.store(allocation, std::memory_order_relaxed);
    auto task = darwin_art::looper::PrepareReusableTask(
        looper, {.callback = CountCallback, .context = &unpublished,
                 .on_quiescent = CountMetadata});
    g_fail_after.store(-1, std::memory_order_relaxed);
    assert(unpublished.value.load() == 0);
    if (task != nullptr) {
      assert(task->Cancel());
      assert(unpublished.value.load() == 1);
    }
  }

  // Idle and admitted cancellation publish metadata only after real callback
  // completion. Reentrant cancellation never duplicates the completion.
  QuiescenceState idle;
  idle.task = darwin_art::looper::PrepareReusableTask(
      looper, {.callback = CloseInCallback, .context = &idle,
               .on_quiescent = OnQuiescent});
  assert(idle.task != nullptr && idle.task->Cancel());
  assert(idle.completions == 1 && idle.task->Cancel());
  assert(idle.completions == 1);
  for (bool throwing : {false, true}) {
    QuiescenceState admitted;
    admitted.throw_in_callback = throwing;
    admitted.task = darwin_art::looper::PrepareReusableTask(
        looper, {.callback = CloseInCallback, .context = &admitted,
                 .on_quiescent = OnQuiescent});
    assert(admitted.task != nullptr && admitted.task->Request());
    bool caught = false;
    try {
      assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) ==
             ALOOPER_POLL_CALLBACK);
    } catch (const std::bad_alloc&) {
      caught = true;
    }
    assert(caught == throwing);
    assert(admitted.task->IsQuiescent() && admitted.completions == 1);
    assert(admitted.task->Cancel() && admitted.completions == 1);
  }

  // Request coalescing and cancellation are allocation-free after Prepare.
  CallbackState allocation_state;
  auto allocation_task = darwin_art::looper::PrepareReusableTask(
      looper, ReentrantCallback, &allocation_state);
  assert(allocation_task != nullptr);
  allocation_state.task = allocation_task;
  g_allocations.store(0, std::memory_order_relaxed);
  g_count_allocations.store(true, std::memory_order_release);
  assert(allocation_task->Request());
  assert(allocation_task->Request());
  assert(allocation_task->Cancel());
  g_count_allocations.store(false, std::memory_order_release);
  assert(g_allocations.load(std::memory_order_relaxed) == 0);
  assert(allocation_task->IsQuiescent());
  assert(!allocation_task->Request());

  // A failed wake is observable while the durable queued request remains;
  // the next coalesced Request retries the signal without reopening a task.
  Counter wake_failure_count;
  auto wake_failure_task = darwin_art::looper::PrepareReusableTask(
      looper, CountCallback, &wake_failure_count);
  assert(wake_failure_task != nullptr);
  g_fail_wake.store(true, std::memory_order_release);
  errno = EAGAIN;
  assert(darwin_art::looper::SignalWake(looper) == -5);
  assert(!wake_failure_task->Request());
  g_fail_wake.store(false, std::memory_order_release);
  assert(wake_failure_task->Request());
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(wake_failure_count.value.load() == 1);
  assert(wake_failure_task->Cancel());
  assert(wake_failure_task->IsQuiescent());

  // Cross-thread requests enqueue on the prepared Looper, but callbacks stay
  // on its owner thread. Reentry coalesces and waits for a following poll.
  CallbackState reentrant_state;
  reentrant_state.owner = std::this_thread::get_id();
  auto reentrant_task = darwin_art::looper::PrepareReusableTask(
      looper, ReentrantCallback, &reentrant_state);
  assert(reentrant_task != nullptr);
  reentrant_state.task = reentrant_task;
  std::thread requester([&] { assert(reentrant_task->Request()); });
  requester.join();
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(reentrant_state.calls.load() == 1);
  assert(reentrant_state.nested_poll.load());
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(reentrant_state.calls.load() == 2);
  assert(!reentrant_state.wrong_thread.load());
  assert(reentrant_task->Cancel());
  assert(reentrant_task->IsQuiescent());

  // Cancel closes immediately while an admitted callback is running; the
  // callback pin keeps state alive until completion and IsQuiescent stays false.
  BlockingState blocking_state;
  auto blocking_task = darwin_art::looper::PrepareReusableTask(
      looper, {.callback = BlockingCallback, .context = &blocking_state,
               .on_quiescent = BlockingQuiescence});
  blocking_state.task = blocking_task;
  assert(blocking_task != nullptr && blocking_task->Request());
  std::thread canceler([&] {
    std::unique_lock<std::mutex> lock(blocking_state.mutex);
    blocking_state.condition.wait(lock,
                                  [&] { return blocking_state.entered; });
    lock.unlock();
    assert(blocking_task->Cancel());
    assert(!blocking_task->IsQuiescent());
    assert(blocking_state.completions.load() == 0);
    {
      std::lock_guard<std::mutex> release_lock(blocking_state.mutex);
      blocking_state.release = true;
    }
    blocking_state.condition.notify_all();
  });
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  canceler.join();
  assert(blocking_state.calls == 1);
  assert(blocking_task->IsQuiescent());
  assert(blocking_state.completions.load() == 1);

  // A dropped last external handle invokes the documented RAII Cancel and
  // cannot leave a stale callback in the queue.
  Counter dropped_count;
  {
    auto dropped_task = darwin_art::looper::PrepareReusableTask(
        looper, CountCallback, &dropped_count);
    assert(dropped_task != nullptr && dropped_task->Request());
  }
  (void)ALooper_pollOnce(0, nullptr, nullptr, nullptr);
  assert(dropped_count.value.load() == 0);

  // A reusable task, due timer, and ready FD all make progress in one real
  // poll. Self-requeue is beyond the dispatch entry cutoff and runs next turn.
  Counter fair_count;
  auto fair_task = darwin_art::looper::PrepareReusableTask(
      looper, CountCallback, &fair_count);
  assert(fair_task != nullptr && fair_task->Request());
  int pair[2] = {-1, -1};
  assert(pipe(pair) == 0);
  Counter fd_count;
  assert(ALooper_addFd(static_cast<ALooper*>(looper), pair[0], 42,
                       ALOOPER_EVENT_INPUT, CountFd, &fd_count) == 1);
  assert(write(pair[1], "f", 1) == 1);
  Counter timer_count;
  assert(darwin_art::looper::ScheduleTimedTaskAt(
             looper, MonotonicNanos(), CountCallback, &timer_count,
             CountRelease) == 1);
  assert(ALooper_pollOnce(100, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  assert(fair_count.value.load() == 1);
  assert(fd_count.value.load() == 1);
  assert(timer_count.value.load() == 2);
  assert(fair_task->Cancel());
  close(pair[0]);
  close(pair[1]);

  // The fixed per-poll budget prevents an unbounded ready queue from starving
  // other Looper work.
  std::vector<Counter> bounded_counts(16);
  std::vector<darwin_art::looper::ReusableLooperTaskHandle> bounded_tasks;
  bounded_tasks.reserve(bounded_counts.size());
  for (Counter& count : bounded_counts) {
    auto task = darwin_art::looper::PrepareReusableTask(looper, CountCallback,
                                                         &count);
    assert(task != nullptr && task->Request());
    bounded_tasks.push_back(std::move(task));
  }
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  int first_turn = 0;
  for (const Counter& count : bounded_counts) first_turn += count.value.load();
  assert(first_turn == 8);
  assert(ALooper_pollOnce(0, nullptr, nullptr, nullptr) ==
         ALOOPER_POLL_CALLBACK);
  int all_turns = 0;
  for (const Counter& count : bounded_counts) all_turns += count.value.load();
  assert(all_turns == 16);
  for (auto& task : bounded_tasks) assert(task->Cancel());

  std::puts("reusable owner-Looper task: coalescing/OOM, affinity/reentry, cancellation, fairness and cleanup PASS");
  return 0;
}
