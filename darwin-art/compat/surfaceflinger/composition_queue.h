#pragma once

#include "composition_protocol.h"

#include <atomic>
#include <cstddef>
#include <condition_variable>
#include <deque>
#include <memory>
#include <mutex>
#include <thread>

namespace darwin_art::process {
class ServiceEndpoint;
}

namespace darwin_art::surfaceflinger {

// Owns the bounded FIFO and its two workers. Callbacks are immutable function
// pointers, never captures of the service thread, so a worker cannot retain a
// dangling endpoint or service object. The endpoint is retained until the
// queue stops; no reference cycle with the accept loop is required.
class CompositionQueue {
 public:
  static constexpr size_t kCapacity = 128;
  // The queue retains descriptor ownership while the callback runs. The
  // callback must exchange any descriptor it consumes to -1 on the job;
  // this lets the queue safely reject the remainder if the callback throws.
  using ComposeCallback = void (*)(CompositionJob&);
  // Takes the fence descriptor and closes it on either success or failure.
  using FenceCallback = bool (*)(int) noexcept;

  ~CompositionQueue();

  bool Start(std::shared_ptr<process::ServiceEndpoint> endpoint,
             ComposeCallback compose, FenceCallback consume_fence,
             std::shared_ptr<std::atomic<bool>> running = nullptr);
  // Descriptor ownership transfers only when this returns true.
  bool Enqueue(CompositionJob job);

  // Borrowed read end for the owning accept loop. It becomes readable exactly
  // once when the queue stops; the queue owns both ends.
  int failure_descriptor() const { return failure_pipe_[0]; }

  // Abort startup without publishing a capability. This also wakes all
  // blocked producers and rejects every queued completion descriptor.
  void Abort();

  // Report an actual worker/accept-loop failure. The readiness IPC occurs only
  // after the queue lock is released and is emitted once per queue lifetime.
  void Fail();

 private:
  void MoveReadyPrefixLocked();
  void MonitorFences();
  void ComposeReady();
  void Transition(bool publish_loss);
  void Reject(std::deque<CompositionJob>* jobs);

  std::mutex mutex_;
  std::condition_variable pending_available_;
  std::condition_variable ready_available_;
  std::condition_variable not_full_;
  std::deque<CompositionJob> pending_;
  std::deque<CompositionJob> ready_;
  std::shared_ptr<process::ServiceEndpoint> endpoint_;
  std::shared_ptr<std::atomic<bool>> running_;
  ComposeCallback compose_ = nullptr;
  FenceCallback consume_fence_ = nullptr;
  bool started_ = false;
  bool failed_ = false;
  bool loss_reported_ = false;
  int failure_pipe_[2] = {-1, -1};
  std::thread monitor_thread_;
  std::thread compose_thread_;
};

}  // namespace darwin_art::surfaceflinger
