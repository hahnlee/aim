#include "composition_fence_monitor.h"

#include "../../tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h"

#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <exception>
#include <mutex>
#include <new>
#include <stdexcept>
#include <thread>
#include <utility>
#include <vector>

namespace darwin_art::window {
namespace {

int DefaultPoll(DarwinArtBionicPollFd* descriptors, size_t count,
                int timeout_ms) noexcept {
  return darwin_art_bionic_socket_broker_poll(descriptors, count, timeout_ms);
}

int DefaultClose(int fence_fd) noexcept {
  return darwin_art_bionic_socket_broker_close(fence_fd);
}

struct Fence {
  int descriptor = -1;
  uint64_t generation = 0;
  bool ready = false;
};

}  // namespace

struct CompositionFenceMonitor::State {
  explicit State(CompositionFenceReadyCallback ready_callback,
                 void* ready_context, CompositionFenceBroker fence_broker)
      : callback(ready_callback),
        callback_context(ready_context),
        broker(fence_broker) {}

  std::mutex mutex;
  std::condition_variable condition;
  std::deque<Fence> fences;
  std::thread worker;
  CompositionFenceReadyCallback callback = nullptr;
  void* callback_context = nullptr;
  CompositionFenceBroker broker;
  uint64_t next_generation = 0;
  std::atomic<uint64_t> submitted_generation{0};
  std::atomic<uint64_t> ready_generation{0};
  bool stop_requested = false;
  bool worker_started = false;
  bool worker_exited = true;
  bool join_in_progress = false;
  bool resources_closed = false;
};

CompositionFenceBroker CompositionFenceMonitor::DefaultBroker() noexcept {
  return {&DefaultPoll, &DefaultClose};
}

CompositionFenceMonitor::CompositionFenceMonitor(
    CompositionFenceReadyCallback callback, void* callback_context,
    CompositionFenceBroker broker)
    : state_(new State(callback, callback_context, broker)) {
  if (state_->broker.poll == nullptr || state_->broker.close == nullptr) {
    throw std::invalid_argument("composition fence broker is incomplete");
  }
}

CompositionFenceMonitor::~CompositionFenceMonitor() {
  Stop();
}

bool CompositionFenceMonitor::Track(int fence_fd) noexcept {
  if (fence_fd < 0 || state_ == nullptr) {
    if (fence_fd >= 0) CloseRejected(fence_fd);
    return false;
  }

  bool close_rejected = false;
  {
    std::lock_guard<std::mutex> lock(state_->mutex);
    if (state_->stop_requested || state_->resources_closed) {
      close_rejected = true;
    } else {
      const uint64_t generation = state_->next_generation + 1;
      try {
        // Publish the generation only after the queue owns the descriptor.
        // This makes allocation failure indistinguishable from no admission.
        state_->fences.push_back(Fence{fence_fd, generation, false});
        state_->next_generation = generation;
      } catch (...) {
        close_rejected = true;
      }

      if (!close_rejected && !state_->worker_started) {
        try {
          state_->worker_started = true;
          state_->worker_exited = false;
          state_->worker = std::thread(
              [state = state_.get()] { CompositionFenceMonitor::Run(state); });
        } catch (...) {
          state_->worker_started = false;
          state_->worker_exited = true;
          state_->fences.pop_back();
          state_->next_generation = generation - 1;
          state_->submitted_generation.store(
              state_->fences.empty() ? 0 : state_->fences.back().generation,
              std::memory_order_release);
          close_rejected = true;
        }
      }
      if (!close_rejected) {
        // The worker cannot be notified until after this lock is released, so
        // its first poll observes the fully published generation.
        state_->submitted_generation.store(generation,
                                           std::memory_order_release);
      }
    }
  }

  if (close_rejected) {
    // The descriptor was never admitted, or was rolled back before worker
    // start. No monitor thread can observe it after the mutex is released.
    (void)state_->broker.close(fence_fd);
    return false;
  }
  state_->condition.notify_one();
  return true;
}

bool CompositionFenceMonitor::ScanoutReady() const noexcept {
  if (state_ == nullptr) return false;
  const uint64_t submitted =
      state_->submitted_generation.load(std::memory_order_acquire);
  return submitted == 0 ||
         state_->ready_generation.load(std::memory_order_acquire) >= submitted;
}

uint64_t CompositionFenceMonitor::SubmittedGeneration() const noexcept {
  return state_ == nullptr
             ? 0
             : state_->submitted_generation.load(std::memory_order_acquire);
}

uint64_t CompositionFenceMonitor::ReadyGeneration() const noexcept {
  return state_ == nullptr
             ? 0
             : state_->ready_generation.load(std::memory_order_acquire);
}

void CompositionFenceMonitor::Run(State* state) noexcept {
  for (;;) {
    std::vector<std::pair<int, uint64_t>> watched;
    try {
      std::unique_lock<std::mutex> lock(state->mutex);
      state->condition.wait(lock, [state] {
        return state->stop_requested || !state->fences.empty();
      });
      if (state->stop_requested) break;
      watched.reserve(state->fences.size());
      for (const Fence& fence : state->fences) {
        if (!fence.ready) watched.emplace_back(fence.descriptor,
                                                fence.generation);
      }
      if (watched.empty()) continue;
    } catch (const std::bad_alloc&) {
      // A transient inability to allocate a poll snapshot cannot transfer
      // ownership or declare a fence ready. Retry while admission remains
      // open; teardown still wakes the condition variable.
      std::unique_lock<std::mutex> lock(state->mutex);
      if (state->stop_requested) break;
      state->condition.wait_for(lock, std::chrono::milliseconds(1));
      continue;
    }

    std::vector<DarwinArtBionicPollFd> descriptors;
    try {
      descriptors.reserve(watched.size());
      for (const auto& [descriptor, generation] : watched) {
        (void)generation;
        descriptors.push_back({descriptor, 0x0001, 0});  // POLLIN
      }
    } catch (const std::bad_alloc&) {
      std::unique_lock<std::mutex> lock(state->mutex);
      if (state->stop_requested) break;
      state->condition.wait_for(lock, std::chrono::milliseconds(1));
      continue;
    }

    int result = -1;
    do {
      // The finite timeout lets Stop establish quiescence without closing an
      // FD that the broker is still polling. No close occurs in this phase.
      result = state->broker.poll(descriptors.data(), descriptors.size(), 16);
      const bool interrupted = result < 0 && errno == EINTR;
      if (interrupted) {
        std::lock_guard<std::mutex> lock(state->mutex);
        if (state->stop_requested) break;
      }
      if (!interrupted) break;
    } while (true);
    if (result <= 0) continue;

    bool advanced = false;
    uint64_t last_ready_generation = 0;
    {
      std::lock_guard<std::mutex> lock(state->mutex);
      if (state->stop_requested) break;
      for (size_t index = 0; index < descriptors.size(); ++index) {
        if (descriptors[index].revents == 0) continue;
        const int descriptor = watched[index].first;
        const uint64_t generation = watched[index].second;
        for (Fence& fence : state->fences) {
          if (fence.ready || fence.descriptor != descriptor ||
              fence.generation != generation) {
            continue;
          }
          // The owner closes exactly once, before marking this one-shot
          // fence ready. FIFO retirement below prevents overtaking.
          (void)state->broker.close(fence.descriptor);
          fence.descriptor = -1;
          fence.ready = true;
          break;
        }
      }
      while (!state->fences.empty() && state->fences.front().ready) {
        const uint64_t generation = state->fences.front().generation;
        state->fences.pop_front();
        state->ready_generation.store(generation, std::memory_order_release);
        last_ready_generation = generation;
        advanced = true;
      }
    }
    if (advanced && state->callback != nullptr) {
      // The callback is outside the monitor lock: it may re-enter the public
      // surface scheduling path, and Stop waits for its complete return.
      state->callback(state->callback_context, last_ready_generation);
    }
  }

  std::lock_guard<std::mutex> lock(state->mutex);
  state->worker_exited = true;
  state->condition.notify_all();
}

void CompositionFenceMonitor::Stop() noexcept {
  if (state_ == nullptr) return;

  std::thread joiner;
  {
    std::unique_lock<std::mutex> lock(state_->mutex);
    state_->stop_requested = true;
    state_->condition.notify_all();
    if (state_->worker.get_id() == std::this_thread::get_id()) {
      // Production callbacks only schedule the owner actor. A callback that
      // destroys its monitor cannot synchronously join itself; allowing the
      // state to destruct here would leave the worker with a dangling State.
      std::terminate();
    }
    while (state_->worker_started && !state_->worker_exited) {
      state_->condition.wait(lock);
    }
    while (state_->join_in_progress) {
      state_->condition.wait(lock);
    }
    if (state_->worker.joinable()) {
      state_->join_in_progress = true;
      joiner = std::move(state_->worker);
    }
  }
  if (joiner.joinable()) joiner.join();

  std::lock_guard<std::mutex> lock(state_->mutex);
  state_->join_in_progress = false;
  if (!state_->resources_closed) {
    // This is deliberately after join. The poll snapshot can therefore never
    // observe a descriptor that teardown has already closed and reused.
    for (Fence& fence : state_->fences) {
      if (fence.descriptor >= 0) {
        (void)state_->broker.close(fence.descriptor);
        fence.descriptor = -1;
      }
    }
    state_->fences.clear();
    state_->resources_closed = true;
  }
  state_->condition.notify_all();
}

void CompositionFenceMonitor::CloseRejected(int fence_fd) noexcept {
  if (fence_fd >= 0) (void)DefaultClose(fence_fd);
}

}  // namespace darwin_art::window
