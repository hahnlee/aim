#include "composition_queue.h"

#include "../process/service_endpoint.h"

#include <cerrno>
#include <fcntl.h>
#include <poll.h>
#include <unistd.h>

#include <iterator>
#include <utility>
#include <vector>

namespace darwin_art::surfaceflinger {
namespace {

void RejectJob(CompositionJob* job) {
  if (job == nullptr) return;
  if (job->producer_descriptor >= 0) {
    close(job->producer_descriptor);
    job->producer_descriptor = -1;
  }
  if (job->completion_descriptor >= 0) {
    close(job->completion_descriptor);
    job->completion_descriptor = -1;
  }
}

}  // namespace

CompositionQueue::~CompositionQueue() {
  Abort();
  if (monitor_thread_.joinable()) monitor_thread_.join();
  if (compose_thread_.joinable()) compose_thread_.join();
  if (failure_pipe_[0] >= 0) close(failure_pipe_[0]);
  if (failure_pipe_[1] >= 0) close(failure_pipe_[1]);
}

bool CompositionQueue::Start(std::shared_ptr<process::ServiceEndpoint> endpoint,
                             ComposeCallback compose,
                             FenceCallback consume_fence,
                             std::shared_ptr<std::atomic<bool>> running) {
  if (!endpoint || !*endpoint || compose == nullptr || consume_fence == nullptr) {
    return false;
  }
  std::unique_lock<std::mutex> lock(mutex_);
  if (failed_) return false;
  if (started_) return true;
  if (pipe(failure_pipe_) != 0) return false;
  for (int descriptor : failure_pipe_) {
    const int flags = fcntl(descriptor, F_GETFD);
    if (flags < 0 || fcntl(descriptor, F_SETFD, flags | FD_CLOEXEC) != 0) {
      close(failure_pipe_[0]);
      close(failure_pipe_[1]);
      failure_pipe_[0] = -1;
      failure_pipe_[1] = -1;
      return false;
    }
  }
  endpoint_ = std::move(endpoint);
  running_ = std::move(running);
  compose_ = compose;
  consume_fence_ = consume_fence;
  try {
    monitor_thread_ = std::thread([this] {
      try {
        MonitorFences();
      } catch (...) {
        Fail();
      }
    });
  } catch (...) {
    failed_ = true;
    if (running_) running_->store(false, std::memory_order_release);
    lock.unlock();
    endpoint_.reset();
    running_.reset();
    close(failure_pipe_[0]);
    close(failure_pipe_[1]);
    failure_pipe_[0] = -1;
    failure_pipe_[1] = -1;
    compose_ = nullptr;
    consume_fence_ = nullptr;
    failed_ = true;
    return false;
  }
  try {
    compose_thread_ = std::thread([this] {
      try {
        ComposeReady();
      } catch (...) {
        Fail();
      }
    });
  } catch (...) {
    // No producer can have entered yet because started_ is still false. Wake
    // and join the first worker before returning, so a stack-owned queue is
    // safe even when thread startup is only partially successful.
    failed_ = true;
    if (running_) running_->store(false, std::memory_order_release);
    lock.unlock();
    pending_available_.notify_all();
    ready_available_.notify_all();
    not_full_.notify_all();
    if (monitor_thread_.joinable()) monitor_thread_.join();
    endpoint_.reset();
    running_.reset();
    close(failure_pipe_[0]);
    close(failure_pipe_[1]);
    failure_pipe_[0] = -1;
    failure_pipe_[1] = -1;
    return false;
  }
  started_ = true;
  return true;
}

bool CompositionQueue::Enqueue(CompositionJob job) {
  std::unique_lock<std::mutex> lock(mutex_);
  if (!started_ || failed_) return false;
  not_full_.wait(lock, [this] {
    return failed_ || pending_.size() + ready_.size() < kCapacity;
  });
  if (failed_) return false;
  pending_.push_back(std::move(job));
  pending_available_.notify_one();
  return true;
}

void CompositionQueue::Abort() {
  Transition(false);
}

void CompositionQueue::Fail() {
  Transition(true);
}

void CompositionQueue::Transition(bool publish_loss) {
  std::shared_ptr<process::ServiceEndpoint> endpoint;
  {
    std::unique_lock<std::mutex> lock(mutex_);
    if (failed_) return;
    failed_ = true;
    if (running_) running_->store(false, std::memory_order_release);
    if (publish_loss && !loss_reported_) {
      loss_reported_ = true;
    }
    endpoint = std::move(endpoint_);
    // Even constructing an empty deque may allocate. Drain the bounded
    // descriptors without allocating or calling any readiness IPC here.
    Reject(&pending_);
    Reject(&ready_);
    pending_.clear();
    ready_.clear();
    lock.unlock();
  }
  pending_available_.notify_all();
  ready_available_.notify_all();
  not_full_.notify_all();
  if (failure_pipe_[1] >= 0) {
    const uint8_t signal = 1;
    ssize_t written;
    do { written = write(failure_pipe_[1], &signal, sizeof(signal)); }
    while (written < 0 && errno == EINTR);
  }
  if (endpoint && publish_loss) {
    // report_lost performs IPC. Never invoke it while holding queue mutex_.
    (void)endpoint->report_lost(process::ServiceReadiness::Compositor);
  }
  // For startup aborts this closes the Rust-owned endpoint after all queue
  // state is detached, avoiding a leaked socket when Serve never launched.
  endpoint.reset();
}

void CompositionQueue::Reject(std::deque<CompositionJob>* jobs) {
  if (jobs == nullptr) return;
  for (CompositionJob& job : *jobs) RejectJob(&job);
}

void CompositionQueue::MoveReadyPrefixLocked() {
  while (!pending_.empty() && pending_.front().producer_descriptor < 0) {
    ready_.push_back(std::move(pending_.front()));
    pending_.pop_front();
  }
  if (!ready_.empty()) ready_available_.notify_one();
  not_full_.notify_all();
}

void CompositionQueue::MonitorFences() {
  for (;;) {
    std::vector<std::pair<size_t, int>> watched;
    {
      std::unique_lock<std::mutex> lock(mutex_);
      pending_available_.wait(lock, [this] { return failed_ || !pending_.empty(); });
      if (failed_) return;
      for (size_t index = 0; index < pending_.size(); ++index) {
        const int descriptor = pending_[index].producer_descriptor;
        if (descriptor >= 0) watched.emplace_back(index, descriptor);
      }
      if (watched.empty()) {
        MoveReadyPrefixLocked();
        continue;
      }
    }

    std::vector<pollfd> waiters;
    waiters.reserve(watched.size());
    for (const auto& [index, descriptor] : watched) {
      (void)index;
      waiters.push_back({.fd = descriptor, .events = POLLIN | POLLHUP, .revents = 0});
    }
    int result = -1;
    do {
      result = poll(waiters.data(), waiters.size(), 16);
    } while (result < 0 && errno == EINTR);
    if (result < 0) {
      Fail();
      return;
    }
    if (result == 0) continue;

    std::lock_guard<std::mutex> lock(mutex_);
    if (failed_) return;
    for (size_t offset = 0; offset < waiters.size(); ++offset) {
      const short events = waiters[offset].revents;
      if (events == 0 || watched[offset].first >= pending_.size()) continue;
      CompositionJob& job = pending_[watched[offset].first];
      if (job.producer_descriptor != watched[offset].second) continue;
      // A bad/closed producer fence belongs to this request, not to the
      // compositor transport. Preserve the queued completion response.
      if ((events & (POLLERR | POLLNVAL)) != 0) {
        close(job.producer_descriptor);
        job.producer_descriptor = -1;
        job.fence_failed = true;
      } else {
        const int descriptor = std::exchange(job.producer_descriptor, -1);
        job.fence_failed = !consume_fence_(descriptor);
      }
    }
    MoveReadyPrefixLocked();
  }
}

void CompositionQueue::ComposeReady() {
  for (;;) {
    CompositionJob job;
    ComposeCallback compose = nullptr;
    {
      std::unique_lock<std::mutex> lock(mutex_);
      ready_available_.wait(lock, [this] { return failed_ || !ready_.empty(); });
      if (failed_) return;
      job = std::move(ready_.front());
      ready_.pop_front();
      compose = compose_;
      not_full_.notify_all();
    }
    if (job.fence_failed) {
      RejectJob(&job);
      continue;
    }
    try {
      compose(job);
    } catch (...) {
      RejectJob(&job);
      Fail();
      return;
    }
    RejectJob(&job);
  }
}

}  // namespace darwin_art::surfaceflinger
