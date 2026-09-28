#include <sync/sync.h>

#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <thread>

#include <fcntl.h>
#include <poll.h>
#include <unistd.h>

namespace {

int WaitForFd(int fd, int timeout_ms) {
  pollfd descriptor{.fd = fd, .events = POLLIN | POLLHUP, .revents = 0};
  int result;
  do {
    result = poll(&descriptor, 1, timeout_ms);
  } while (result < 0 && errno == EINTR);
  if (result > 0) return 0;
  if (result == 0) errno = ETIME;
  return -1;
}

}  // namespace

extern "C" int sync_wait(int fd, int timeout_ms) {
  if (fd < 0) return 0;
  return WaitForFd(fd, timeout_ms);
}

extern "C" int sync_merge(const char*, int fd1, int fd2) {
  int descriptors[2];
  if (pipe(descriptors) != 0) return -1;
  fcntl(descriptors[0], F_SETFD, FD_CLOEXEC);
  fcntl(descriptors[1], F_SETFD, FD_CLOEXEC);
#ifdef F_SETNOSIGPIPE
  // Closing a sync fence cancels observation, not the underlying GPU work.
  // Its internal completion writer must not raise an application SIGPIPE.
  if (fcntl(descriptors[1], F_SETNOSIGPIPE, 1) != 0) {
    close(descriptors[0]);
    close(descriptors[1]);
    return -1;
  }
#endif

  const int first = dup(fd1);
  const int second = dup(fd2);
  if (first < 0 || second < 0) {
    if (first >= 0) close(first);
    if (second >= 0) close(second);
    close(descriptors[0]);
    close(descriptors[1]);
    return -1;
  }

  std::thread([first, second, signal = descriptors[1]] {
    WaitForFd(first, -1);
    WaitForFd(second, -1);
    const uint8_t ready = 1;
    (void)write(signal, &ready, sizeof(ready));
    close(first);
    close(second);
    close(signal);
  }).detach();
  return descriptors[0];
}

extern "C" struct sync_file_info* sync_file_info(int fd) {
  const size_t allocation_size =
      sizeof(struct sync_file_info) + sizeof(struct sync_fence_info);
  auto* info = static_cast<struct sync_file_info*>(calloc(1, allocation_size));
  if (info == nullptr) return nullptr;
  std::strncpy(info->name, "darwin-fd-fence", sizeof(info->name) - 1);
  info->status = WaitForFd(fd, 0) == 0 ? 1 : 0;
  info->num_fences = 1;
  info->sync_fence_info = sizeof(struct sync_file_info);
  auto* fence = reinterpret_cast<struct sync_fence_info*>(info + 1);
  std::strncpy(fence->obj_name, "darwin", sizeof(fence->obj_name) - 1);
  std::strncpy(fence->driver_name, "poll", sizeof(fence->driver_name) - 1);
  fence->status = info->status;
  return info;
}

extern "C" struct sync_fence_info* sync_get_fence_info(
    struct sync_file_info* info) {
  if (info == nullptr || info->num_fences == 0) return nullptr;
  return reinterpret_cast<struct sync_fence_info*>(
      reinterpret_cast<uint8_t*>(info) + info->sync_fence_info);
}

extern "C" void sync_file_info_free(struct sync_file_info* info) {
  free(info);
}
