// Component-only host wake/broker ports. The production ALooper and reusable
// task queue execute unchanged; pipe/poll are a controlled stand-in for the
// Android eventfd broker. This does not establish production broker coverage.
#include "darwin_art_bionic_socket_broker.h"
#include <cerrno>
#include <fcntl.h>
#include <map>
#include <mutex>
#include <poll.h>
#include <unistd.h>
#include <vector>

namespace {
std::mutex wake_mutex;
std::map<int, int> writers;
thread_local int broker_errno = 0;
template <typename T> T Observe(T result) {
  if (result < 0) broker_errno = errno;
  return result;
}
}
extern "C" int darwin_art_bionic_socket_broker_eventfd(uint32_t, int) {
  int descriptors[2];
  if (pipe(descriptors) != 0) return Observe(-1);
  std::lock_guard lock(wake_mutex);
  writers.emplace(descriptors[0], descriptors[1]);
  return descriptors[0];
}
extern "C" intptr_t darwin_art_bionic_socket_broker_read(
    int fd, void* bytes, size_t size) { return Observe(read(fd, bytes, size)); }
extern "C" intptr_t darwin_art_bionic_socket_broker_write(
    int fd, const void* bytes, size_t size) {
  std::lock_guard lock(wake_mutex);
  const auto found = writers.find(fd);
  if (found == writers.end()) { broker_errno = EBADF; return -1; }
  return Observe(write(found->second, bytes, size));
}
extern "C" int darwin_art_bionic_socket_broker_fcntl(
    int fd, int command, intptr_t argument) { return Observe(fcntl(fd, command, argument)); }
extern "C" int darwin_art_bionic_socket_broker_poll(
    DarwinArtBionicPollFd* descriptors, size_t count, int timeout) {
  std::vector<pollfd> host;
  for (size_t i = 0; i < count; ++i)
    host.push_back({descriptors[i].fd, descriptors[i].events, 0});
  const int result = Observe(poll(host.data(), host.size(), timeout));
  if (result >= 0)
    for (size_t i = 0; i < count; ++i) descriptors[i].revents = host[i].revents;
  return result;
}
extern "C" int darwin_art_bionic_errno_load() { return broker_errno; }
