#include <netinet/in.h>
#include <poll.h>
#include <stdint.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

__attribute__((visibility("default"))) int SocketFixtureTcp(int32_t out[3]) {
  int one = 1;
  int server = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (server < 0) return 10;
  if (setsockopt(server, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)) != 0) return 11;
  struct sockaddr_in address = {.sin_family = AF_INET,
                                .sin_port = 0,
                                .sin_addr = {.s_addr = htonl(INADDR_LOOPBACK)}};
  if (bind(server, (const struct sockaddr*)&address, sizeof(address)) != 0) return 12;
  socklen_t length = sizeof(address);
  if (getsockname(server, (struct sockaddr*)&address, &length) != 0 ||
      length != sizeof(address) || address.sin_port == 0) return 13;
  if (listen(server, 4) != 0) return 14;
  int client = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (client < 0) return 15;
  if (connect(client, (const struct sockaddr*)&address, sizeof(address)) != 0) return 16;
  const char request[4] = {'p', 'i', 'n', 'g'};
  if (send(client, request, sizeof(request), MSG_NOSIGNAL) != sizeof(request)) return 20;
  struct sockaddr_in peer = {0};
  length = sizeof(peer);
  int accepted = accept4(server, (struct sockaddr*)&peer, &length,
                         SOCK_CLOEXEC);
  if (accepted < 0 || peer.sin_family != AF_INET || length != sizeof(peer)) return 17;
  length = sizeof(peer);
  if (getpeername(client, (struct sockaddr*)&peer, &length) != 0 ||
      peer.sin_port != address.sin_port) return 18;
  int type = 0;
  length = sizeof(type);
  if (getsockopt(accepted, SOL_SOCKET, SO_TYPE, &type, &length) != 0 ||
      type != SOCK_STREAM) return 19;
  char response[4] = {0};
  if (recv(accepted, response, sizeof(response), MSG_WAITALL) != sizeof(response)) return 21;
  if (response[0] != 'p' || response[3] != 'g') return 22;
  out[0] = server;
  out[1] = client;
  out[2] = accepted;
  return 0;
}

__attribute__((visibility("default"))) int SocketFixtureNonblock(
    int32_t out[3]) {
  int server = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (server < 0) return 40;
  struct sockaddr_in address = {.sin_family = AF_INET,
                                .sin_port = 0,
                                .sin_addr = {.s_addr = htonl(INADDR_LOOPBACK)}};
  if (bind(server, (const struct sockaddr*)&address, sizeof(address)) != 0) return 41;
  socklen_t length = sizeof(address);
  if (getsockname(server, (struct sockaddr*)&address, &length) != 0 ||
      listen(server, 1) != 0) return 42;
  int client = socket(AF_INET, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (client < 0 ||
      connect(client, (const struct sockaddr*)&address, sizeof(address)) != 0) return 43;
  int accepted = accept4(server, (struct sockaddr*)0, (socklen_t*)0,
                         SOCK_CLOEXEC | SOCK_NONBLOCK);
  if (accepted < 0) return 44;
  out[0] = server;
  out[1] = client;
  out[2] = accepted;
  return 0;
}

__attribute__((visibility("default"))) int SocketFixtureUdp(int32_t out[2]) {
  int receiver = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int sender = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (receiver < 0 || sender < 0) return 30;
  struct sockaddr_in address = {.sin_family = AF_INET,
                                .sin_port = 0,
                                .sin_addr = {.s_addr = htonl(INADDR_LOOPBACK)}};
  if (bind(receiver, (const struct sockaddr*)&address, sizeof(address)) != 0) return 31;
  socklen_t length = sizeof(address);
  if (getsockname(receiver, (struct sockaddr*)&address, &length) != 0) return 32;
  const char payload[3] = {'u', 'd', 'p'};
  if (sendto(sender, payload, sizeof(payload), MSG_NOSIGNAL,
             (const struct sockaddr*)&address, sizeof(address)) != sizeof(payload)) return 33;
  char result[3] = {0};
  struct sockaddr_in source = {0};
  length = sizeof(source);
  if (recvfrom(receiver, result, sizeof(result), 0,
               (struct sockaddr*)&source, &length) != sizeof(result)) return 34;
  if (result[0] != 'u' || result[2] != 'p' || source.sin_family != AF_INET) return 35;
  out[0] = receiver;
  out[1] = sender;
  return 0;
}

__attribute__((visibility("default"))) int SocketFixtureUdp6(int32_t out[2]) {
  int receiver = socket(AF_INET6, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int sender = socket(AF_INET6, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (receiver < 0 || sender < 0) return 50;
  struct sockaddr_in6 address = {.sin6_family = AF_INET6,
                                 .sin6_port = 0,
                                 .sin6_addr = IN6ADDR_LOOPBACK_INIT};
  if (bind(receiver, (const struct sockaddr*)&address, sizeof(address)) != 0) return 51;
  socklen_t length = sizeof(address);
  if (getsockname(receiver, (struct sockaddr*)&address, &length) != 0 ||
      length != sizeof(address) || address.sin6_family != AF_INET6) return 52;
  const char payload[2] = {'v', '6'};
  if (sendto(sender, payload, sizeof(payload), MSG_NOSIGNAL,
             (const struct sockaddr*)&address, sizeof(address)) != sizeof(payload)) return 53;
  char result[2] = {0};
  struct sockaddr_in6 source = {0};
  length = sizeof(source);
  if (recvfrom(receiver, result, sizeof(result), 0,
               (struct sockaddr*)&source, &length) != sizeof(result)) return 54;
  if (result[0] != 'v' || result[1] != '6' ||
      source.sin6_family != AF_INET6 || length != sizeof(source)) return 55;
  out[0] = receiver;
  out[1] = sender;
  return 0;
}

__attribute__((visibility("default"))) int SocketFixturePair(int32_t out[2]) {
  return socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, out);
}

__attribute__((visibility("default"))) intptr_t SocketFixtureSend(
    int fd, const void* data, size_t length, int flags) {
  return send(fd, data, length, flags);
}

__attribute__((visibility("default"))) intptr_t SocketFixtureRecv(
    int fd, void* data, size_t length, int flags) {
  return recv(fd, data, length, flags);
}

__attribute__((visibility("default"))) int SocketFixtureClose(int fd) {
  return close(fd);
}

__attribute__((visibility("default"))) int SocketFixtureShutdown(int fd,
                                                                    int how) {
  return shutdown(fd, how);
}

__attribute__((visibility("default"))) int SocketFixtureUnsupportedOption(
    int fd) {
  int value = 0;
  socklen_t length = sizeof(value);
  return getsockopt(fd, SOL_SOCKET, 0x7fffffff, &value, &length);
}

__attribute__((visibility("default"))) int SocketFixtureUnsupportedFlags(
    int fd) {
  char byte = 0;
  return (int)recv(fd, &byte, 1, 0x20000000);
}

__attribute__((visibility("default"))) int SocketFixturePollOne(
    int fd, int timeout_ms, int16_t* revents) {
  struct pollfd descriptor = {.fd = fd, .events = POLLIN, .revents = 0};
  int result = poll(&descriptor, 1, timeout_ms);
  *revents = descriptor.revents;
  return result;
}

__attribute__((visibility("default"))) int SocketFixturePpollOne(
    int fd, int64_t seconds, int16_t* revents) {
  struct pollfd descriptor = {.fd = fd, .events = POLLIN, .revents = 0};
  struct timespec timeout = {.tv_sec = seconds, .tv_nsec = 0};
  int result = ppoll(&descriptor, 1, &timeout, (const sigset_t*)0);
  *revents = descriptor.revents;
  return result;
}

__attribute__((visibility("default"))) int SocketFixturePollTimeout(
    int use_ppoll) {
  if (use_ppoll) {
    struct timespec timeout = {.tv_sec = 0, .tv_nsec = 5000000};
    return ppoll((struct pollfd*)0, 0, &timeout, (const sigset_t*)0);
  }
  return poll((struct pollfd*)0, 0, 5);
}

__attribute__((visibility("default"))) int SocketFixturePpollMaskRejected(
    int fd) {
  struct pollfd descriptor = {.fd = fd, .events = POLLIN, .revents = 0};
  struct timespec timeout = {.tv_sec = 0, .tv_nsec = 0};
  sigset_t mask = {0};
  return ppoll(&descriptor, 1, &timeout, &mask);
}

static int scatter_udp4(int32_t out[2]) {
  int receiver = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int sender = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (receiver < 0 || sender < 0) return 60;
  struct sockaddr_in address = {.sin_family = AF_INET,
                                .sin_port = 0,
                                .sin_addr = {.s_addr = htonl(INADDR_LOOPBACK)}};
  if (bind(receiver, (const struct sockaddr*)&address, sizeof(address)) != 0) return 61;
  socklen_t length = sizeof(address);
  if (getsockname(receiver, (struct sockaddr*)&address, &length) != 0) return 62;
  const char first[2] = {'s', 'c'};
  const char second[3] = {'a', 't', '4'};
  struct iovec send_iov[2] = {{.iov_base = (void*)first, .iov_len = sizeof(first)},
                              {.iov_base = (void*)second, .iov_len = sizeof(second)}};
  struct msghdr outbound = {.msg_name = &address,
                            .msg_namelen = sizeof(address),
                            .msg_iov = send_iov,
                            .msg_iovlen = 2};
  if (sendmsg(sender, &outbound, MSG_NOSIGNAL) != 5) return 63;
  char left[1] = {0};
  char right[4] = {0};
  struct iovec recv_iov[2] = {{.iov_base = left, .iov_len = sizeof(left)},
                              {.iov_base = right, .iov_len = sizeof(right)}};
  struct sockaddr_in source = {0};
  struct msghdr inbound = {.msg_name = &source,
                           .msg_namelen = sizeof(source),
                           .msg_iov = recv_iov,
                           .msg_iovlen = 2};
  if (recvmsg(receiver, &inbound, 0) != 5) return 64;
  if (left[0] != 's' || right[0] != 'c' || right[3] != '4' ||
      source.sin_family != AF_INET || inbound.msg_namelen != sizeof(source) ||
      inbound.msg_controllen != 0 || inbound.msg_flags != 0) return 65;
  if (sendmsg(sender, &outbound, MSG_NOSIGNAL) != 5) return 66;
  inbound.msg_iov = recv_iov;
  inbound.msg_iovlen = 1;
  inbound.msg_namelen = sizeof(source);
  inbound.msg_flags = 0;
  if (recvmsg(receiver, &inbound, 0) != 1 ||
      (inbound.msg_flags & MSG_TRUNC) == 0) return 67;
  out[0] = receiver;
  out[1] = sender;
  return 0;
}

static int scatter_udp6(int32_t out[2]) {
  int receiver = socket(AF_INET6, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  int sender = socket(AF_INET6, SOCK_DGRAM | SOCK_CLOEXEC, 0);
  if (receiver < 0 || sender < 0) return 70;
  struct sockaddr_in6 address = {.sin6_family = AF_INET6,
                                 .sin6_port = 0,
                                 .sin6_addr = IN6ADDR_LOOPBACK_INIT};
  if (bind(receiver, (const struct sockaddr*)&address, sizeof(address)) != 0) return 71;
  socklen_t length = sizeof(address);
  if (getsockname(receiver, (struct sockaddr*)&address, &length) != 0) return 72;
  const char first[3] = {'v', '6', '-'};
  const char second[2] = {'s', 'g'};
  struct iovec send_iov[2] = {{.iov_base = (void*)first, .iov_len = sizeof(first)},
                              {.iov_base = (void*)second, .iov_len = sizeof(second)}};
  struct msghdr outbound = {.msg_name = &address,
                            .msg_namelen = sizeof(address),
                            .msg_iov = send_iov,
                            .msg_iovlen = 2};
  if (sendmsg(sender, &outbound, MSG_NOSIGNAL) != 5) return 73;
  char result[5] = {0};
  struct iovec recv_iov[2] = {{.iov_base = result, .iov_len = 2},
                              {.iov_base = result + 2, .iov_len = 3}};
  struct sockaddr_in6 source = {0};
  struct msghdr inbound = {.msg_name = &source,
                           .msg_namelen = sizeof(source),
                           .msg_iov = recv_iov,
                           .msg_iovlen = 2};
  if (recvmsg(receiver, &inbound, 0) != 5) return 74;
  if (result[0] != 'v' || result[4] != 'g' ||
      source.sin6_family != AF_INET6 || inbound.msg_namelen != sizeof(source)) return 75;
  out[0] = receiver;
  out[1] = sender;
  return 0;
}

__attribute__((visibility("default"))) int SocketFixtureScatterUdp4(
    int32_t out[2]) {
  return scatter_udp4(out);
}

__attribute__((visibility("default"))) int SocketFixtureScatterUdp6(
    int32_t out[2]) {
  return scatter_udp6(out);
}

__attribute__((visibility("default"))) int SocketFixtureScmRightsRejected(
    int fd) {
  char byte = 'x';
  struct iovec iov = {.iov_base = &byte, .iov_len = 1};
  union {
    struct cmsghdr align;
    unsigned char bytes[CMSG_SPACE(sizeof(int))];
  } control = {0};
  struct cmsghdr* header = (struct cmsghdr*)control.bytes;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  struct msghdr message = {.msg_iov = &iov,
                           .msg_iovlen = 1,
                           .msg_control = control.bytes,
                           .msg_controllen = sizeof(control.bytes)};
  return (int)sendmsg(fd, &message, 0);
}

__attribute__((visibility("default"))) int SocketFixtureRecvControlRejected(
    int fd) {
  char byte = 0;
  struct iovec iov = {.iov_base = &byte, .iov_len = 1};
  unsigned char control[CMSG_SPACE(sizeof(int))] = {0};
  struct msghdr message = {.msg_iov = &iov,
                           .msg_iovlen = 1,
                           .msg_control = control,
                           .msg_controllen = sizeof(control)};
  return (int)recvmsg(fd, &message, MSG_DONTWAIT);
}
