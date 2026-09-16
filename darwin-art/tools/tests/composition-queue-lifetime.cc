// Bounded composition queue ownership fixture. This exercises the queue with
// mocked typed-service endpoint FFI; it never starts a profile or Android app.
#include "../../compat/process/service_endpoint.h"
#include "../../compat/surfaceflinger/composition_queue.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <thread>
#include <vector>

namespace {

std::atomic<uint64_t> next_handle{1};
std::atomic<int> endpoint_ready_calls{0};
std::atomic<int> endpoint_loss_calls{0};
std::atomic<int> endpoint_close_calls{0};
std::atomic<uint32_t> endpoint_loss_bits{0};

}  // namespace

extern "C" int32_t darwin_art_service_endpoint_open(
    const uint8_t* path, size_t length, uint64_t* handle, int32_t* descriptor) {
  assert(path != nullptr && length != 0 && handle != nullptr && descriptor != nullptr);
  *handle = next_handle.fetch_add(1, std::memory_order_relaxed);
  // The queue only borrows this value through ServiceEndpoint. No OS fd is
  // installed here, so the mock cannot accidentally close a test pipe.
  *descriptor = -1;
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_close(uint64_t handle) {
  assert(handle != 0);
  endpoint_close_calls.fetch_add(1, std::memory_order_relaxed);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_ready(uint64_t handle,
                                                       uint32_t bits) {
  assert(handle != 0 && bits != 0);
  endpoint_ready_calls.fetch_add(1, std::memory_order_relaxed);
  return 0;
}

extern "C" int32_t darwin_art_service_endpoint_lost(uint64_t handle,
                                                      uint32_t bits) {
  assert(handle != 0 && bits != 0);
  endpoint_loss_calls.fetch_add(1, std::memory_order_relaxed);
  endpoint_loss_bits.fetch_or(bits, std::memory_order_relaxed);
  return 0;
}

namespace {

using darwin_art::process::ServiceEndpoint;
using darwin_art::surfaceflinger::CompositionJob;
using darwin_art::surfaceflinger::CompositionQueue;

struct ComposeState {
  std::mutex mutex;
  std::condition_variable changed;
  std::vector<uint64_t> order;
  bool block = false;
  bool release = false;
  bool throw_after_handoff = false;
  bool entered = false;
  bool handoff_done = false;
  bool threw = false;
  int replacement_descriptor = -1;
};

struct FenceState {
  std::atomic<int> calls{0};
};

ComposeState* compose_state = nullptr;
FenceState* fence_state = nullptr;

template <typename Predicate>
bool WaitFor(std::condition_variable& changed, std::mutex& mutex,
             Predicate predicate, std::chrono::milliseconds timeout =
                 std::chrono::milliseconds(2000)) {
  std::unique_lock<std::mutex> lock(mutex);
  return changed.wait_for(lock, timeout, predicate);
}

bool IsClosed(int descriptor) {
  errno = 0;
  return fcntl(descriptor, F_GETFD) == -1 && errno == EBADF;
}

bool WaitForClosed(int descriptor) {
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::seconds(2);
  while (std::chrono::steady_clock::now() < deadline) {
    if (IsClosed(descriptor)) return true;
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  return IsClosed(descriptor);
}

void ClosePair(int (&pair)[2]) {
  for (int& descriptor : pair) {
    if (descriptor >= 0) {
      close(descriptor);
      descriptor = -1;
    }
  }
}

CompositionJob Job(uint64_t transaction_id, int producer = -1,
                   int completion = -1) {
  CompositionJob job;
  job.header.transaction_id = transaction_id;
  job.producer_descriptor = producer;
  job.completion_descriptor = completion;
  return job;
}

void Compose(CompositionJob& job) {
  ComposeState* state = compose_state;
  assert(state != nullptr);
  std::unique_lock<std::mutex> lock(state->mutex);
  state->entered = true;
  state->changed.notify_all();
  state->changed.wait(lock, [state] { return !state->block || state->release; });
  state->order.push_back(job.header.transaction_id);
  if (!state->throw_after_handoff) {
    state->changed.notify_all();
    return;
  }

  const int handed_off = job.completion_descriptor;
  assert(handed_off >= 0);
  job.completion_descriptor = -1;
  assert(close(handed_off) == 0);

  // Reuse the handed-off number for a sentinel. If queue cleanup closes the
  // field after the callback throws, this sentinel becomes invalid and the
  // test detects the double close after worker join.
  const int source = open("/dev/null", O_WRONLY);
  assert(source >= 0);
  if (source != handed_off) {
    assert(dup2(source, handed_off) == handed_off);
    close(source);
  }
  state->replacement_descriptor = handed_off;
  state->handoff_done = true;
  state->threw = true;
  state->changed.notify_all();
  throw std::runtime_error("composition callback fixture failure");
}

bool ConsumeFence(int descriptor) noexcept {
  assert(descriptor >= 0);
  char marker = 0;
  ssize_t received;
  do {
    received = read(descriptor, &marker, sizeof(marker));
  } while (received < 0 && errno == EINTR);
  assert(close(descriptor) == 0);
  assert(fence_state != nullptr);
  fence_state->calls.fetch_add(1, std::memory_order_relaxed);
  return false;
}

std::shared_ptr<ServiceEndpoint> Endpoint() {
  return std::make_shared<ServiceEndpoint>("/composition-queue-fixture");
}

void ResetEndpointCounters() {
  endpoint_ready_calls.store(0, std::memory_order_relaxed);
  endpoint_loss_calls.store(0, std::memory_order_relaxed);
  endpoint_close_calls.store(0, std::memory_order_relaxed);
  endpoint_loss_bits.store(0, std::memory_order_relaxed);
}

void TestFifoOrder() {
  ResetEndpointCounters();
  ComposeState state;
  compose_state = &state;
  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    for (uint64_t id = 1; id <= 8; ++id) assert(queue.Enqueue(Job(id)));
    assert(WaitFor(state.changed, state.mutex,
                   [&state] { return state.order.size() == 8; }));
  }
  compose_state = nullptr;
  assert(state.order == std::vector<uint64_t>({1, 2, 3, 4, 5, 6, 7, 8}));
  assert(endpoint_loss_calls.load(std::memory_order_relaxed) == 0);
  assert(endpoint_close_calls.load(std::memory_order_relaxed) == 1);
}

void TestFenceFailureIsJobLocal() {
  ResetEndpointCounters();
  FenceState fences;
  fence_state = &fences;
  ComposeState state;
  compose_state = &state;
  int producer[2] = {-1, -1};
  int completion[2] = {-1, -1};
  assert(pipe(producer) == 0 && pipe(completion) == 0);
  close(producer[1]);
  producer[1] = -1;
  close(completion[0]);
  completion[0] = -1;
  const int completion_descriptor = completion[1];
  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    assert(queue.Enqueue(Job(99, producer[0], completion_descriptor)));
    producer[0] = -1;
    completion[1] = -1;
    const auto deadline = std::chrono::steady_clock::now() +
                          std::chrono::seconds(2);
    while (fences.calls.load(std::memory_order_relaxed) == 0 &&
           std::chrono::steady_clock::now() < deadline) {
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    assert(fences.calls.load(std::memory_order_relaxed) == 1);
    assert(WaitForClosed(completion_descriptor));
  }
  compose_state = nullptr;
  fence_state = nullptr;
  assert(state.order.empty());
  assert(endpoint_loss_calls.load(std::memory_order_relaxed) == 0);
  assert(endpoint_close_calls.load(std::memory_order_relaxed) == 1);
  ClosePair(producer);
  ClosePair(completion);
}

void TestFailWakesBackpressureAndSignalsLoss() {
  ResetEndpointCounters();
  ComposeState state;
  state.block = true;
  compose_state = &state;
  auto running = std::make_shared<std::atomic<bool>>(false);
  std::atomic<size_t> enqueued{0};
  std::atomic<bool> producer_done{false};
  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence, running));
    running->store(true, std::memory_order_release);
    std::thread producer([&queue, &enqueued, &producer_done] {
      for (size_t id = 0; id < CompositionQueue::kCapacity + 2; ++id) {
        if (!queue.Enqueue(Job(1000 + id))) break;
        enqueued.fetch_add(1, std::memory_order_release);
      }
      producer_done.store(true, std::memory_order_release);
    });
    assert(WaitFor(state.changed, state.mutex,
                   [&state] { return state.entered; }));
    const auto deadline = std::chrono::steady_clock::now() +
                          std::chrono::seconds(2);
    while (enqueued.load(std::memory_order_acquire) <
               CompositionQueue::kCapacity + 1 &&
           std::chrono::steady_clock::now() < deadline) {
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    assert(enqueued.load(std::memory_order_acquire) >= CompositionQueue::kCapacity + 1);
    assert(!producer_done.load(std::memory_order_acquire));

    queue.Fail();
    pollfd waiter{queue.failure_descriptor(), POLLIN, 0};
    assert(poll(&waiter, 1, 2000) == 1 && (waiter.revents & POLLIN) != 0);
    uint8_t signal = 0;
    assert(read(queue.failure_descriptor(), &signal, sizeof(signal)) == 1);
    assert(signal == 1);
    assert(!running->load(std::memory_order_acquire));
    {
      std::lock_guard<std::mutex> lock(state.mutex);
      state.release = true;
    }
    state.changed.notify_all();
    const auto producer_deadline = std::chrono::steady_clock::now() +
                                   std::chrono::seconds(2);
    while (!producer_done.load(std::memory_order_acquire) &&
           std::chrono::steady_clock::now() < producer_deadline) {
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    assert(producer_done.load(std::memory_order_acquire));
    producer.join();
  }
  compose_state = nullptr;
  assert(endpoint_loss_calls.load(std::memory_order_relaxed) == 1);
  assert((endpoint_loss_bits.load(std::memory_order_relaxed) & 2u) != 0);
  assert(endpoint_close_calls.load(std::memory_order_relaxed) == 1);
}

void TestCompletionHandoffSurvivesThrow() {
  ResetEndpointCounters();
  ComposeState state;
  state.throw_after_handoff = true;
  compose_state = &state;
  int completion[2] = {-1, -1};
  assert(pipe(completion) == 0);
  close(completion[0]);
  completion[0] = -1;
  const int original = completion[1];
  {
    CompositionQueue queue;
    assert(queue.Start(Endpoint(), &Compose, &ConsumeFence));
    assert(queue.Enqueue(Job(777, -1, original)));
    completion[1] = -1;
    assert(WaitFor(state.changed, state.mutex,
                   [&state] { return state.handoff_done; }));
    assert(WaitFor(state.changed, state.mutex,
                   [&state] { return state.threw; }));
    pollfd waiter{queue.failure_descriptor(), POLLIN, 0};
    assert(poll(&waiter, 1, 2000) == 1 && (waiter.revents & POLLIN) != 0);
    assert(!queue.Enqueue(Job(778)));
  }
  compose_state = nullptr;
  assert(fcntl(state.replacement_descriptor, F_GETFD) >= 0);
  close(state.replacement_descriptor);
  assert(endpoint_loss_calls.load(std::memory_order_relaxed) == 1);
  assert((endpoint_loss_bits.load(std::memory_order_relaxed) & 2u) != 0);
  assert(endpoint_close_calls.load(std::memory_order_relaxed) == 1);
  ClosePair(completion);
}

}  // namespace

int main() {
  alarm(10);
  // Exercise the endpoint's readiness mock independently; queue failure tests
  // then verify that only the compositor capability is reported as lost.
  ResetEndpointCounters();
  {
    auto endpoint = Endpoint();
    assert(endpoint->publish_ready(darwin_art::process::ServiceReadiness::Binder) ==
           darwin_art::process::ReadinessPublication::Published);
    assert(endpoint_ready_calls.load(std::memory_order_relaxed) == 1);
  }
  assert(endpoint_loss_calls.load(std::memory_order_relaxed) == 1);
  assert(endpoint_close_calls.load(std::memory_order_relaxed) == 1);

  TestFifoOrder();
  TestFenceFailureIsJobLocal();
  TestFailWakesBackpressureAndSignalsLoss();
  TestCompletionHandoffSurvivesThrow();
  alarm(0);
  std::puts("composition queue lifetime: FIFO, fence isolation, failure wake/loss and handoff cleanup PASS");
}
