#include "network/multinetwork.h"

#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <sys/socket.h>
#include <unistd.h>

namespace {
int32_t g_android_errno = 0;
}

extern "C" void darwin_art_bionic_errno_store(int32_t value) {
  g_android_errno = value;
}
extern "C" int darwin_art_bionic_errno_set_from_darwin(int value) {
  g_android_errno = value == EBADF ? 9 : value == ENOTSOCK ? 88 : 5;
  return g_android_errno;
}
extern "C" int darwin_art_bionic_fd_dup_host_fd_core(int fd, int* native_fd) {
  if (fd < 0) {
    g_android_errno = 9;
    return -1;
  }
  *native_fd = dup(fd);
  return 1;
}
extern "C" int darwin_art_bionic_socket_broker_res_nquery(
    uint64_t, const char*, int, int, uint32_t) {
  return 123;
}
extern "C" int darwin_art_bionic_socket_broker_res_nresult(
    int, int*, uint8_t*, size_t) {
  return 7;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  return close(fd);
}

template <typename Function>
Function Resolve(const char* name, const char* version = "LIBANDROID") {
  auto address = darwin_art_android_multinetwork_symbol(name, version);
  assert(address != nullptr);
  return reinterpret_cast<Function>(address);
}

int main() {
  using SetNetwork = int (*)(uint64_t);
  using GetNetwork = int (*)(uint64_t*);
  using SetSocket = int (*)(uint64_t, int);
  using Query = int (*)(uint64_t, const char*, int, int, uint32_t);
  using Send = int (*)(uint64_t, const uint8_t*, size_t, uint32_t);

  assert(darwin_art_android_multinetwork_symbol("android_setprocnetwork",
                                                 "WRONG") == nullptr);
  assert(darwin_art_android_multinetwork_symbol("private_symbol",
                                                 "LIBANDROID") == nullptr);
  auto set_process = Resolve<SetNetwork>("android_setprocnetwork");
  auto get_process = Resolve<GetNetwork>("android_getprocnetwork");
  auto set_dns = Resolve<SetNetwork>("android_setprocdns");
  auto get_dns = Resolve<GetNetwork>("android_getprocdns", nullptr);
  auto set_socket = Resolve<SetSocket>("android_setsocknetwork");
  auto query = Resolve<Query>("android_res_nquery");
  auto send = Resolve<Send>("android_res_nsend");

  uint64_t network = 99;
  assert(set_process(0) == 0 && get_process(&network) == 0 && network == 0);
  assert(get_process(nullptr) == -1 && g_android_errno == 22);
  assert(set_process((UINT64_C(17) << 32) | UINT64_C(0xcafed00d)) == -1 &&
         g_android_errno == 64);
  assert(get_process(&network) == 0 && network == 0);
  assert(set_dns(0) == 0 && get_dns(&network) == 0 && network == 0);

  int sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  assert(set_socket(0, sockets[0]) == 0);
  int pipes[2];
  assert(pipe(pipes) == 0);
  assert(set_socket(0, pipes[0]) == -1 && g_android_errno == 88);
  assert(set_socket(0, -1) == -1 && g_android_errno == 9);
  using TagWithUid = int (*)(int, uint32_t, uint32_t);
  using Untag = int (*)(int);
  auto tag = Resolve<TagWithUid>("android_tag_socket_with_uid");
  auto untag = Resolve<Untag>("android_untag_socket");
  // libnetd_client tagSocket/untagSocket return negative errno values.
  assert(tag(sockets[0], 7, 10042) == 0 && untag(sockets[0]) == 0);
  assert(tag(pipes[0], 7, 10042) == -88 && untag(pipes[0]) == -88);
  assert(tag(-1, 7, 10042) == -9 && untag(-1) == -9);
  close(sockets[0]);
  close(sockets[1]);
  close(pipes[0]);
  close(pipes[1]);

  assert(query(0, "example.com", 1, 1, 0) == 123);
  assert(query((UINT64_C(8) << 32) | UINT64_C(0xcafed00d),
               "example.com", 1, 1, 0) == -64);
  assert(send(0, nullptr, 0, 0) == -95);
  return 0;
}
