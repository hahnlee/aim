#include <chrono>
#include <cstdint>
#include <iostream>
#include <thread>

#include "base/atomic.h"
#include "base/locks.h"
#include "base/mutex.h"
#include "thread.h"

// The probe links only ART's mutex object, not a complete Runtime. These are
// the four process/thread hooks reached by mutex diagnostics; none participate
// in the permit or condition-variable algorithm under test.
namespace art {

bool Thread::is_started_ = false;
thread_local Thread* Thread::self_tls_ = nullptr;
Mutex* Locks::runtime_shutdown_lock_ = nullptr;

bool Locks::IsSafeToCallAbortRacy() {
  return false;
}

uint32_t GetTid() {
  uint64_t tid = 0;
  pthread_threadid_np(nullptr, &tid);
  return static_cast<uint32_t>(tid);
}

}  // namespace art

namespace {

constexpr int32_t kPermitAvailable = 0;
constexpr int32_t kNoPermit = 1;
constexpr int32_t kNoPermitWaiterWaiting = 2;

class Parker {
 public:
  Parker()
      : mutex_("Darwin park probe", art::LockLevel::kThreadWaitLock),
        condition_("Darwin park probe", mutex_),
        state_(kNoPermit) {}

  bool Park(int64_t timeout_ns) {
    int32_t old_state = state_.fetch_add(1, std::memory_order_relaxed);
    if (old_state == kPermitAvailable) {
      return false;
    }
    art::MutexLock lock(nullptr, mutex_);
    bool timed_out = false;
    if (state_.load(std::memory_order_relaxed) == kNoPermitWaiterWaiting) {
      if (timeout_ns == 0) {
        condition_.Wait(nullptr);
      } else {
        timed_out = condition_.TimedWait(nullptr,
                                         timeout_ns / INT64_C(1000000),
                                         static_cast<int32_t>(timeout_ns % INT64_C(1000000)));
      }
    }
    state_.store(kNoPermit, std::memory_order_relaxed);
    return timed_out;
  }

  void Unpark() {
    art::MutexLock lock(nullptr, mutex_);
    if (state_.exchange(kPermitAvailable, std::memory_order_relaxed) ==
        kNoPermitWaiterWaiting) {
      condition_.Signal(nullptr);
    }
  }

  bool IsWaiting() const {
    return state_.load(std::memory_order_relaxed) == kNoPermitWaiterWaiting;
  }

 private:
  art::Mutex mutex_;
  art::ConditionVariable condition_;
  art::AtomicInteger state_;
};

}  // namespace

int main() {
  Parker parker;

  parker.Unpark();
  if (parker.Park(0)) {
    std::cerr << "pre-issued permit unexpectedly timed out\n";
    return 1;
  }

  for (int iteration = 0; iteration != 200; ++iteration) {
    std::thread waiter([&] { parker.Park(0); });
    while (!parker.IsWaiting()) {
      std::this_thread::yield();
    }
    parker.Unpark();
    waiter.join();
  }

  auto start = std::chrono::steady_clock::now();
  if (!parker.Park(INT64_C(20000000))) {
    std::cerr << "timed park did not report a timeout\n";
    return 1;
  }
  auto elapsed = std::chrono::steady_clock::now() - start;
  if (elapsed < std::chrono::milliseconds(10) || elapsed > std::chrono::seconds(1)) {
    std::cerr << "timed park duration is outside its tolerance\n";
    return 1;
  }

  std::cout << "ART Darwin park: pre-permit=yes wakeups=200 timeout=yes\n";
  return 0;
}
