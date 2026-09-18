#pragma once

#include <cerrno>
#include <chrono>
#include <climits>
#include <poll.h>
#include <unistd.h>

namespace darwin_art::surfaceflinger {

// Transport deadline, not a GPU fence wait. A fragmented composition request
// shares one deadline across its header, layer payload and descriptor marker.
using SocketDeadline = std::chrono::steady_clock::time_point;

inline bool WaitReadableUntil(int descriptor, SocketDeadline deadline) {
  for (;;) {
    const auto remaining = deadline - std::chrono::steady_clock::now();
    if (remaining <= SocketDeadline::duration::zero()) {
      errno = ETIMEDOUT;
      return false;
    }
    const auto milliseconds =
        std::chrono::duration_cast<std::chrono::milliseconds>(remaining).count();
    pollfd waiter{descriptor, POLLIN, 0};
    const int result = poll(&waiter, 1,
        static_cast<int>(milliseconds < INT_MAX ? milliseconds + 1 : INT_MAX));
    if (result < 0 && errno == EINTR) continue;
    if (result <= 0) {
      if (result == 0) errno = ETIMEDOUT;
      return false;
    }
    return (waiter.revents & POLLIN) != 0;
  }
}

inline bool ReadAllUntil(int descriptor, void* destination, size_t size,
                         SocketDeadline deadline) {
  auto* bytes = static_cast<unsigned char*>(destination);
  while (size != 0) {
    if (!WaitReadableUntil(descriptor, deadline)) return false;
    const ssize_t received = read(descriptor, bytes, size);
    if (received < 0 && (errno == EINTR || errno == EAGAIN ||
                         errno == EWOULDBLOCK)) continue;
    if (received <= 0) return false;
    bytes += received;
    size -= static_cast<size_t>(received);
  }
  return true;
}

}  // namespace darwin_art::surfaceflinger
