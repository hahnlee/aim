#include "socket_transport.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace darwin_art::surfaceflinger {
bool WriteAll(int fd, const void* data, size_t size) {
  const auto* bytes = static_cast<const uint8_t*>(data);
  while (size != 0) {
    const ssize_t result = write(fd, bytes, size);
    if (result < 0 && errno == EINTR) continue;
    if (result <= 0) return false;
    bytes += result;
    size -= static_cast<size_t>(result);
  }
  return true;
}

bool ReadAll(int fd, void* data, size_t size) {
  auto* bytes = static_cast<uint8_t*>(data);
  while (size != 0) {
    const ssize_t result = read(fd, bytes, size);
    if (result < 0 && errno == EINTR) continue;
    if (result <= 0) return false;
    bytes += result;
    size -= static_cast<size_t>(result);
  }
  return true;
}

bool SendDescriptor(int socket_fd, int descriptor) {
  char marker = descriptor >= 0 ? 1 : 0;
  iovec vector{.iov_base = &marker, .iov_len = sizeof(marker)};
  std::array<char, CMSG_SPACE(sizeof(int))> control{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  if (descriptor >= 0) {
    message.msg_control = control.data();
    message.msg_controllen = control.size();
    cmsghdr* header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    std::memcpy(CMSG_DATA(header), &descriptor, sizeof(descriptor));
  }
  for (;;) {
    const ssize_t sent = sendmsg(socket_fd, &message, 0);
    if (sent < 0 && errno == EINTR) continue;
    return sent == static_cast<ssize_t>(sizeof(marker));
  }
}

bool ReceiveDescriptor(int socket_fd, bool expected, int* descriptor) {
  if (descriptor == nullptr) return false;
  *descriptor = -1;
  char marker = 0;
  iovec vector{.iov_base = &marker, .iov_len = sizeof(marker)};
  // XNU bsd/kern/uipc_usrreq.c UIPC_MAX_CMSG_FD is 512:
  // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/uipc_usrreq.c
  // Receive the whole rights
  // batch before rejecting extras: undersized ancillary buffers on Darwin
  // can install rights whose numbers are not returned to userspace.
  std::array<char, CMSG_SPACE(512 * sizeof(int))> control{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control.data();
  message.msg_controllen = control.size();
  ssize_t received;
  do {
    received = recvmsg(socket_fd, &message, 0);
  } while (received < 0 && errno == EINTR);
  // recvmsg can install descriptors even on truncated/malformed payloads.
  // Inspect every delivered right and close all extras before validating.
  unsigned rights = 0;
  for (cmsghdr* header = CMSG_FIRSTHDR(&message); header != nullptr;
       header = CMSG_NXTHDR(&message, header)) {
    if (header->cmsg_level != SOL_SOCKET ||
        header->cmsg_type != SCM_RIGHTS ||
        header->cmsg_len < CMSG_LEN(0)) continue;
    const auto* payload = CMSG_DATA(header);
    const auto* control_end = reinterpret_cast<const unsigned char*>(control.data()) +
        std::min(control.size(), static_cast<size_t>(message.msg_controllen));
    if (payload > control_end) continue;
    const size_t payload_size = std::min(
        static_cast<size_t>(header->cmsg_len - CMSG_LEN(0)),
        static_cast<size_t>(control_end - payload));
    const size_t count = payload_size / sizeof(int);
    for (size_t index = 0; index < count; ++index) {
      int value = -1;
      std::memcpy(&value, CMSG_DATA(header) + index * sizeof(int), sizeof(value));
      if (++rights == 1) *descriptor = value;
      else if (value >= 0) close(value);
    }
  }
  if (received != static_cast<ssize_t>(sizeof(marker)) ||
      (message.msg_flags & (MSG_CTRUNC | MSG_TRUNC)) != 0 ||
      marker != (expected ? 1 : 0) || rights != (expected ? 1u : 0u) ||
      ((*descriptor >= 0) != expected)) {
    if (*descriptor >= 0) close(*descriptor);
    *descriptor = -1;
    return false;
  }
  if (*descriptor >= 0) {
    const int flags = fcntl(*descriptor, F_GETFD);
    if (flags < 0 || fcntl(*descriptor, F_SETFD, flags | FD_CLOEXEC) != 0) {
      close(*descriptor);
      *descriptor = -1;
      return false;
    }
  }
  return true;
}

int Connect(const char* path) {
  if (path == nullptr || path[0] == '\0') return -1;
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (std::strlen(path) >= sizeof(address.sun_path)) return -1;
  std::memcpy(address.sun_path, path, std::strlen(path) + 1);
  const int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) return -1;
  const int no_sigpipe = 1;
  if (fcntl(fd, F_SETFD, FD_CLOEXEC) != 0 ||
      setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe,
                 sizeof(no_sigpipe)) != 0) {
    close(fd);
    return -1;
  }
  if (connect(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
    close(fd);
    return -1;
  }
  return fd;
}

}  // namespace darwin_art::surfaceflinger
