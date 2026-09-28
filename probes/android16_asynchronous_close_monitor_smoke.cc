#include "AsynchronousCloseMonitor.h"

#include <errno.h>
#include <unistd.h>

#include <atomic>
#include <cstdio>
#include <thread>

namespace {

struct Result {
  ssize_t read_result = 0;
  int error = 0;
  bool signaled = false;
};

void BlockingRead(int fd, std::atomic<int>* ready, Result* result) {
  AsynchronousCloseMonitor monitor(fd);
  ready->fetch_add(1, std::memory_order_release);
  char byte;
  result->read_result = read(fd, &byte, sizeof(byte));
  result->error = errno;
  result->signaled = monitor.wasSignaled();
}

}  // namespace

int main() {
  AsynchronousCloseMonitor::init();
  int pipe_fds[2];
  if (pipe(pipe_fds) == -1) {
    return 2;
  }

  std::atomic<int> ready {0};
  Result first;
  Result second;
  std::thread first_thread(BlockingRead, pipe_fds[0], &ready, &first);
  std::thread second_thread(BlockingRead, pipe_fds[0], &ready, &second);
  while (ready.load(std::memory_order_acquire) != 2) {
    std::this_thread::yield();
  }

  AsynchronousCloseMonitor::signalBlockedThreads(pipe_fds[0]);
  first_thread.join();
  second_thread.join();
  close(pipe_fds[0]);
  close(pipe_fds[1]);

  if (first.read_result != -1 || first.error != EINTR || !first.signaled ||
      second.read_result != -1 || second.error != EINTR || !second.signaled) {
    std::fprintf(stderr,
                 "async-close: first=%zd/%d/%d second=%zd/%d/%d\n",
                 first.read_result, first.error, first.signaled,
                 second.read_result, second.error, second.signaled);
    return 3;
  }
  std::puts("async-close: two-blocked-readers=EINTR signaled=2");
  return 0;
}
