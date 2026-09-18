#include "compat/window/composition_fence_monitor.h"
#include "darwin_art_bionic_socket_broker.h"

#include <assert.h>
#include <poll.h>
#include <unistd.h>

#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <functional>
#include <mutex>
#include <stdexcept>
#include <thread>
#include <vector>

namespace {

std::atomic<int> g_close_count{0};
std::atomic<int> g_close_attempts{0};
std::atomic<int> g_close_failures{0};

int PollPipes(DarwinArtBionicPollFd* descriptors, size_t count,
              int timeout_ms) noexcept {
  std::vector<pollfd> host(count);
  for (size_t index = 0; index < count; ++index) {
    host[index] = {descriptors[index].fd, descriptors[index].events, 0};
  }
  const int result = ::poll(host.data(), host.size(), timeout_ms);
  if (result >= 0) {
    for (size_t index = 0; index < count; ++index) {
      descriptors[index].revents = host[index].revents;
    }
  }
  return result;
}

int ClosePipe(int descriptor) noexcept {
  if (descriptor < 0) return 0;
  ++g_close_attempts;
  const int result = ::close(descriptor);
  if (result == 0) ++g_close_count;
  else ++g_close_failures;
  return result;
}

// The production TU contains the default broker adapter even though these
// tests inject TestBroker(). Keep those symbols explicit and test-only.
extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout_ms) {
  return PollPipes(descriptors, count, timeout_ms);
}

extern "C" int darwin_art_bionic_socket_broker_close(int descriptor) {
  return ClosePipe(descriptor);
}

struct CallbackState {
  std::mutex mutex;
  std::condition_variable changed;
  uint64_t generations[8]{};
  size_t count = 0;
  bool block = false;
  bool entered = false;
  bool release = false;
};

void OnReady(void* opaque, uint64_t generation) noexcept {
  auto* state = static_cast<CallbackState*>(opaque);
  std::unique_lock<std::mutex> lock(state->mutex);
  if (state->count < 8) state->generations[state->count++] = generation;
  state->entered = true;
  state->changed.notify_all();
  if (state->block) {
    state->changed.wait(lock, [state] { return state->release; });
  }
  state->changed.notify_all();
}

bool WaitFor(CallbackState& state, const std::function<bool()>& predicate) {
  std::unique_lock<std::mutex> lock(state.mutex);
  return state.changed.wait_for(lock, std::chrono::seconds(2), predicate);
}

size_t CallbackCount(CallbackState& state) {
  std::lock_guard<std::mutex> lock(state.mutex);
  return state.count;
}

void ClosePair(int pair[2]) {
  assert(::close(pair[1]) == 0);
  pair[1] = -1;
}

darwin_art::window::CompositionFenceBroker TestBroker() {
  return {&PollPipes, &ClosePipe};
}

void TestFifoGenerationReadiness() {
  int first[2]{-1, -1};
  int second[2]{-1, -1};
  assert(::pipe(first) == 0 && ::pipe(second) == 0);
  CallbackState callback;
  {
    darwin_art::window::CompositionFenceMonitor monitor(
        &OnReady, &callback, TestBroker());
    assert(monitor.ScanoutReady());
    assert(monitor.Track(first[0]));
    first[0] = -1;
    assert(!monitor.ScanoutReady());
    assert(monitor.Track(second[0]));
    second[0] = -1;

    // A later fence can signal first, but must not publish past the FIFO head.
    assert(::write(second[1], "s", 1) == 1);
    std::this_thread::sleep_for(std::chrono::milliseconds(40));
    assert(CallbackCount(callback) == 0);
    assert(monitor.ReadyGeneration() == 0);
    assert(!monitor.ScanoutReady());

    assert(::write(first[1], "f", 1) == 1);
    assert(WaitFor(callback, [&] { return callback.count == 1; }));
    assert(callback.generations[0] == 2);
    assert(monitor.SubmittedGeneration() == 2);
    assert(monitor.ReadyGeneration() == 2);
    assert(monitor.ScanoutReady());
  }
  ClosePair(first);
  ClosePair(second);
  assert(g_close_failures.load() == 0);
  assert(g_close_count.load() == 2);
}

void TestStopWaitsForCallbackAndOwnsPendingDescriptors() {
  int signaled[2]{-1, -1};
  int pending[2]{-1, -1};
  assert(::pipe(signaled) == 0 && ::pipe(pending) == 0);
  CallbackState callback;
  callback.block = true;
  const int closes_before = g_close_count.load();
  std::atomic<bool> stopped{false};
  {
    darwin_art::window::CompositionFenceMonitor monitor(
        &OnReady, &callback, TestBroker());
    assert(monitor.Track(signaled[0]));
    signaled[0] = -1;
    assert(monitor.Track(pending[0]));
    pending[0] = -1;
    assert(::write(signaled[1], "x", 1) == 1);
    assert(WaitFor(callback, [&] { return callback.entered; }));

    std::thread stopper([&] {
      monitor.Stop();
      stopped.store(true, std::memory_order_release);
    });
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    assert(!stopped.load(std::memory_order_acquire));
    {
      std::lock_guard<std::mutex> lock(callback.mutex);
      callback.release = true;
      callback.changed.notify_all();
    }
    stopper.join();
    assert(stopped.load(std::memory_order_acquire));
  }
  ClosePair(signaled);
  ClosePair(pending);
  assert(g_close_count.load() == closes_before + 2);
}

void TestRejectedTrackClosesDescriptor() {
  int pair[2]{-1, -1};
  assert(::pipe(pair) == 0);
  CallbackState callback;
  darwin_art::window::CompositionFenceMonitor monitor(
      &OnReady, &callback, TestBroker());
  monitor.Stop();
  const int before = g_close_count.load();
  assert(!monitor.Track(pair[0]));
  pair[0] = -1;
  assert(g_close_count.load() == before + 1);
  ClosePair(pair);
  assert(g_close_failures.load() == 0);
}

struct PollBarrier {
  std::mutex mutex;
  std::condition_variable changed;
  bool entered = false;
  bool release = false;
};
PollBarrier* g_poll_barrier = nullptr;

int BlockedPoll(DarwinArtBionicPollFd* descriptors, size_t count, int timeout) noexcept {
  {
    std::unique_lock<std::mutex> lock(g_poll_barrier->mutex);
    g_poll_barrier->entered = true;
    g_poll_barrier->changed.notify_all();
    assert(g_poll_barrier->changed.wait_for(lock, std::chrono::seconds(3), [] {
      return g_poll_barrier->release;
    }));
  }
  return PollPipes(descriptors, count, timeout);
}

void TestStopDoesNotCloseAnInflightPollSnapshot() {
  PollBarrier barrier;
  g_poll_barrier = &barrier;
  int pending[2];
  assert(::pipe(pending) == 0);
  const int before = g_close_attempts.load();
  darwin_art::window::CompositionFenceMonitor monitor(
      nullptr, nullptr, {&BlockedPoll, &ClosePipe});
  assert(monitor.Track(pending[0]));
  {
    std::unique_lock<std::mutex> lock(barrier.mutex);
    assert(barrier.changed.wait_for(lock, std::chrono::seconds(2), [&] {
      return barrier.entered;
    }));
  }
  std::atomic<bool> stopped{false};
  std::thread stopper([&] {
    monitor.Stop();
    stopped.store(true, std::memory_order_release);
  });
  // Observe the actual admission barrier, not just a stopper-thread flag or
  // elapsed delay. Admitted trial FDs remain owned until the poll settles.
  size_t admitted_trials = 0;
  bool rejected = false;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  while (!rejected && std::chrono::steady_clock::now() < deadline) {
    int trial[2];
    assert(::pipe(trial) == 0);
    if (monitor.Track(trial[0])) ++admitted_trials;
    else rejected = true;
    assert(::close(trial[1]) == 0);
    std::this_thread::yield();
  }
  assert(rejected);
  assert(!stopped.load(std::memory_order_acquire));
  // Only the rejected trial may be closed while the snapshot is borrowed.
  assert(g_close_attempts.load() == before + 1);
  {
    std::lock_guard<std::mutex> lock(barrier.mutex);
    barrier.release = true;
    barrier.changed.notify_all();
  }
  stopper.join();
  assert(g_close_attempts.load() == before + 2 + static_cast<int>(admitted_trials));
  assert(g_close_failures.load() == 0);
  assert(::close(pending[1]) == 0);
  g_poll_barrier = nullptr;
}

std::atomic<size_t> g_interrupts{0};
int InterruptedPoll(DarwinArtBionicPollFd*, size_t, int) noexcept {
  ++g_interrupts;
  errno = EINTR;
  return -1;
}

void TestRepeatedInterruptsDoNotPreventRetirement() {
  int pending[2];
  assert(::pipe(pending) == 0);
  darwin_art::window::CompositionFenceMonitor monitor(
      nullptr, nullptr, {&InterruptedPoll, &ClosePipe});
  assert(monitor.Track(pending[0]));
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  while (g_interrupts.load() == 0 && std::chrono::steady_clock::now() < deadline)
    std::this_thread::yield();
  assert(g_interrupts.load() != 0);
  std::mutex mutex;
  std::condition_variable changed;
  bool stopped = false;
  std::thread watchdog([&] {
    std::unique_lock<std::mutex> lock(mutex);
    if (!changed.wait_for(lock, std::chrono::seconds(3), [&] { return stopped; }))
      ::_exit(79); // A broken EINTR loop must fail, not strand the test runner.
  });
  monitor.Stop();
  {
    std::lock_guard<std::mutex> lock(mutex);
    stopped = true;
    changed.notify_all();
  }
  watchdog.join();
  assert(::close(pending[1]) == 0);
}

void TestIncompleteBrokerIsRejectedWithoutFallback() {
  bool rejected = false;
  try {
    darwin_art::window::CompositionFenceMonitor monitor(
        nullptr, nullptr, {&PollPipes, nullptr});
  } catch (const std::invalid_argument&) {
    rejected = true;
  }
  assert(rejected);
}

}  // namespace

int main() {
  TestFifoGenerationReadiness();
  TestStopWaitsForCallbackAndOwnsPendingDescriptors();
  TestRejectedTrackClosesDescriptor();
  TestStopDoesNotCloseAnInflightPollSnapshot();
  TestRepeatedInterruptsDoNotPreventRetirement();
  TestIncompleteBrokerIsRejectedWithoutFallback();
  return 0;
}
