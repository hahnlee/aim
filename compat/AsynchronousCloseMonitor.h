#ifndef AIM_ASYNCHRONOUS_CLOSE_MONITOR_H_
#define AIM_ASYNCHRONOUS_CLOSE_MONITOR_H_

// ABI-compatible Darwin implementation boundary for Android libandroidio.
extern "C" {
void async_close_monitor_destroy(void* instance);
void async_close_monitor_static_init();
void async_close_monitor_signal_blocked_threads(int fd);
int async_close_monitor_was_signalled(const void* instance);
void* async_close_monitor_create(int fd);
}

class AsynchronousCloseMonitor {
 public:
  explicit AsynchronousCloseMonitor(int fd)
      : instance_(async_close_monitor_create(fd)) {}
  ~AsynchronousCloseMonitor() {
    async_close_monitor_destroy(instance_);
  }

  bool wasSignaled() const {
    return async_close_monitor_was_signalled(instance_) != 0;
  }

  static void init() {
    async_close_monitor_static_init();
  }

  static void signalBlockedThreads(int fd) {
    async_close_monitor_signal_blocked_threads(fd);
  }

 private:
  AsynchronousCloseMonitor(const AsynchronousCloseMonitor&) = delete;
  AsynchronousCloseMonitor& operator=(const AsynchronousCloseMonitor&) = delete;

  void* instance_;
};

#endif  // AIM_ASYNCHRONOUS_CLOSE_MONITOR_H_
