#define LOG_TAG "AsynchronousCloseMonitor"

#include "AsynchronousCloseMonitor.h"

#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <string.h>

#include <atomic>
#include <mutex>

#include <log/log.h>

namespace {

// Darwin has no POSIX realtime-signal range. ART reserves SIGQUIT and SIGUSR1
// for its signal catcher; the Darwin Android runtime reserves SIGUSR2 for
// asynchronous-close interruption.
constexpr int kBlockedThreadSignal = SIGUSR2;

class DarwinAsynchronousCloseMonitor {
 public:
  explicit DarwinAsynchronousCloseMonitor(int fd);
  ~DarwinAsynchronousCloseMonitor();

  bool WasSignaled() const {
    return signaled_.load(std::memory_order_acquire);
  }

  static void Init();
  static void SignalBlockedThreads(int fd);

 private:
  DarwinAsynchronousCloseMonitor(const DarwinAsynchronousCloseMonitor&) = delete;
  DarwinAsynchronousCloseMonitor& operator=(
      const DarwinAsynchronousCloseMonitor&) = delete;

  DarwinAsynchronousCloseMonitor* previous_ = nullptr;
  DarwinAsynchronousCloseMonitor* next_ = nullptr;
  pthread_t thread_ {};
  int fd_ = -1;
  std::atomic<bool> signaled_ {false};

  static std::mutex list_mutex_;
  static DarwinAsynchronousCloseMonitor* list_;
};

std::mutex DarwinAsynchronousCloseMonitor::list_mutex_;
DarwinAsynchronousCloseMonitor* DarwinAsynchronousCloseMonitor::list_ =
    nullptr;
std::once_flag init_once;

void BlockedThreadSignalHandler(int) {
  // Deliberately empty: delivery without SA_RESTART interrupts the syscall.
}

void DarwinAsynchronousCloseMonitor::Init() {
  std::call_once(init_once, [] {
    struct sigaction action {};
    action.sa_handler = BlockedThreadSignalHandler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = 0;
    if (sigaction(kBlockedThreadSignal, &action, nullptr) == -1) {
      ALOGE("installing Darwin blocked-thread handler failed: %s",
            strerror(errno));
    }
  });
}

void DarwinAsynchronousCloseMonitor::SignalBlockedThreads(int fd) {
  std::lock_guard<std::mutex> lock(list_mutex_);
  for (DarwinAsynchronousCloseMonitor* monitor = list_; monitor != nullptr;
       monitor = monitor->next_) {
    if (monitor->fd_ == fd) {
      monitor->signaled_.store(true, std::memory_order_release);
      const int error = pthread_kill(monitor->thread_, kBlockedThreadSignal);
      if (error != 0 && error != ESRCH) {
        ALOGE("interrupting Darwin blocked thread failed: %s",
              strerror(error));
      }
    }
  }
}

DarwinAsynchronousCloseMonitor::DarwinAsynchronousCloseMonitor(int fd)
    : thread_(pthread_self()), fd_(fd) {
  std::lock_guard<std::mutex> lock(list_mutex_);
  next_ = list_;
  if (next_ != nullptr) {
    next_->previous_ = this;
  }
  list_ = this;
}

DarwinAsynchronousCloseMonitor::~DarwinAsynchronousCloseMonitor() {
  std::lock_guard<std::mutex> lock(list_mutex_);
  if (next_ != nullptr) {
    next_->previous_ = previous_;
  }
  if (previous_ == nullptr) {
    list_ = next_;
  } else {
    previous_->next_ = next_;
  }
}

}  // namespace

extern "C" void async_close_monitor_static_init() {
  DarwinAsynchronousCloseMonitor::Init();
}

extern "C" void async_close_monitor_signal_blocked_threads(int fd) {
  DarwinAsynchronousCloseMonitor::SignalBlockedThreads(fd);
}

extern "C" void* async_close_monitor_create(int fd) {
  return new DarwinAsynchronousCloseMonitor(fd);
}

extern "C" void async_close_monitor_destroy(void* instance) {
  delete static_cast<DarwinAsynchronousCloseMonitor*>(instance);
}

extern "C" int async_close_monitor_was_signalled(const void* instance) {
  const auto* monitor =
      static_cast<const DarwinAsynchronousCloseMonitor*>(instance);
  return monitor->WasSignaled() ? 1 : 0;
}
