#include "aim_bionic_errno.h"
#include "aim_bionic_socket.h"
#include "aim_elf_loader.h"

#include <errno.h>
#include <signal.h>
#include <pthread.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <thread>
#include <vector>

namespace {

void InterruptHandler(int) {}

void Check(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-socket-facade: FAIL %s android-errno=%d\n",
                 message, aim_bionic_errno_load());
    std::abort();
  }
}

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  const auto function = aim_bionic_socket_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (function == nullptr) return AIM_ELF_RESOLVE_NOT_FOUND;
  *output = reinterpret_cast<uintptr_t>(function);
  return AIM_ELF_RESOLVE_FOUND;
}

template <typename Function>
Function Lookup(AimElfHandle* image, const char* name) {
  uintptr_t address = 0;
  char message[256]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_lookup(image, name, &address, &error) ==
            AIM_ELF_OK &&
            address != 0,
        name);
  return reinterpret_cast<Function>(address);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  const char* symbols[] = {
      "accept4",     "bind",       "close",      "connect",
      "getpeername", "getsockname", "getsockopt", "listen",
      "poll",        "ppoll",      "recv",       "recvfrom",
      "recvmsg",     "send",       "sendmsg",    "sendto",
      "setsockopt",  "shutdown",   "socket",     "socketpair",
  };
  for (const char* symbol : symbols) {
    Check(aim_bionic_socket_resolve("libc.so", symbol, "LIBC") !=
              nullptr,
          "exact resolver entry");
  }
  Check(aim_bionic_socket_resolve("libSystem.B.dylib", "socket",
                                         "LIBC") == nullptr &&
            aim_bionic_socket_resolve("libc.so", "socket", nullptr) ==
                nullptr &&
            aim_bionic_socket_resolve("libc.so", "select", "LIBC") ==
                nullptr,
        "closed resolver");

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load actual Android ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "run initializers");

  using Vertical = int (*)(int32_t*);
  using Transfer = intptr_t (*)(int, void*, size_t, int);
  using Send = intptr_t (*)(int, const void*, size_t, int);
  using Unary = int (*)(int);
  using Shutdown = int (*)(int, int);
  Vertical tcp = Lookup<Vertical>(image, "SocketFixtureTcp");
  Vertical udp = Lookup<Vertical>(image, "SocketFixtureUdp");
  Vertical udp6 = Lookup<Vertical>(image, "SocketFixtureUdp6");
  Vertical nonblock = Lookup<Vertical>(image, "SocketFixtureNonblock");
  Vertical pair = Lookup<Vertical>(image, "SocketFixturePair");
  Send send = Lookup<Send>(image, "SocketFixtureSend");
  Transfer recv = Lookup<Transfer>(image, "SocketFixtureRecv");
  Unary close = Lookup<Unary>(image, "SocketFixtureClose");
  Shutdown shutdown = Lookup<Shutdown>(image, "SocketFixtureShutdown");
  Unary bad_option = Lookup<Unary>(image, "SocketFixtureUnsupportedOption");
  Unary bad_flags = Lookup<Unary>(image, "SocketFixtureUnsupportedFlags");
  using PollOne = int (*)(int, int, int16_t*);
  using PpollOne = int (*)(int, int64_t, int16_t*);
  PollOne poll_one = Lookup<PollOne>(image, "SocketFixturePollOne");
  PpollOne ppoll_one = Lookup<PpollOne>(image, "SocketFixturePpollOne");
  Unary poll_timeout = Lookup<Unary>(image, "SocketFixturePollTimeout");
  Unary ppoll_mask = Lookup<Unary>(image, "SocketFixturePpollMaskRejected");
  Vertical scatter4 = Lookup<Vertical>(image, "SocketFixtureScatterUdp4");
  Vertical scatter6 = Lookup<Vertical>(image, "SocketFixtureScatterUdp6");
  Unary scm_rights = Lookup<Unary>(image, "SocketFixtureScmRightsRejected");
  Unary recv_control = Lookup<Unary>(image, "SocketFixtureRecvControlRejected");

  errno = EDOM;
  aim_bionic_errno_store(77);
  int32_t tcp_fds[3]{-1, -1, -1};
  Check(tcp(tcp_fds) == 0 && errno == EDOM &&
            aim_bionic_errno_load() == 77,
        "TCP loopback/address/options/nonblock");
  Check(shutdown(tcp_fds[1], 1) == 0, "shutdown translation");
  for (int fd : tcp_fds) Check(close(fd) == 0, "close TCP token");

  int32_t nonblock_fds[3]{-1, -1, -1};
  Check(nonblock(nonblock_fds) == 0, "accept4 nonblocking setup");
  char empty = 0;
  aim_bionic_errno_store(0);
  Check(recv(nonblock_fds[2], &empty, 1, 0) == -1 &&
            aim_bionic_errno_load() == 11,
        "accept4 SOCK_NONBLOCK becomes EAGAIN");
  for (int fd : nonblock_fds) Check(close(fd) == 0, "close nonblock token");

  int32_t udp_fds[2]{-1, -1};
  Check(udp(udp_fds) == 0, "UDP loopback/sendto/recvfrom");
  for (int fd : udp_fds) Check(close(fd) == 0, "close UDP token");

  int32_t udp6_fds[2]{-1, -1};
  Check(udp6(udp6_fds) == 0, "IPv6 UDP loopback/sockaddr translation");
  for (int fd : udp6_fds) Check(close(fd) == 0, "close IPv6 UDP token");

  int32_t scatter_fds[2]{-1, -1};
  Check(scatter4(scatter_fds) == 0, "IPv4 UDP sendmsg/recvmsg scatter-gather");
  for (int fd : scatter_fds) Check(close(fd) == 0, "close IPv4 scatter token");
  Check(scatter6(scatter_fds) == 0, "IPv6 UDP sendmsg/recvmsg scatter-gather");
  for (int fd : scatter_fds) Check(close(fd) == 0, "close IPv6 scatter token");

  const auto poll_start = std::chrono::steady_clock::now();
  Check(poll_timeout(0) == 0, "poll timeout result");
  const auto poll_elapsed = std::chrono::steady_clock::now() - poll_start;
  Check(poll_elapsed >= std::chrono::milliseconds(2) &&
            poll_elapsed < std::chrono::seconds(1),
        "poll timeout duration");
  const auto ppoll_start = std::chrono::steady_clock::now();
  Check(poll_timeout(1) == 0, "ppoll timeout result");
  const auto ppoll_elapsed = std::chrono::steady_clock::now() - ppoll_start;
  Check(ppoll_elapsed >= std::chrono::milliseconds(2) &&
            ppoll_elapsed < std::chrono::seconds(1),
        "ppoll rounded timeout duration");

  int32_t pair_fds[2]{-1, -1};
  Check(pair(pair_fds) == 0, "socketpair");
  aim_bionic_errno_store(0);
  Check(scm_rights(pair_fds[0]) == -1 &&
            aim_bionic_errno_load() == 95,
        "sendmsg SCM_RIGHTS fail closed");
  aim_bionic_errno_store(0);
  Check(recv_control(pair_fds[0]) == -1 &&
            aim_bionic_errno_load() == 95,
        "recvmsg control buffer fail closed before I/O");
  aim_bionic_errno_store(0);
  Check(ppoll_mask(pair_fds[0]) == -1 &&
            aim_bionic_errno_load() == 95,
        "ppoll non-null Android sigset fail closed");
  aim_bionic_errno_store(0);
  Check(bad_option(pair_fds[0]) == -1 &&
            aim_bionic_errno_load() == 92,
        "unknown option ENOPROTOOPT");
  aim_bionic_errno_store(0);
  Check(bad_flags(pair_fds[0]) == -1 &&
            aim_bionic_errno_load() == 95,
        "unknown flags EOPNOTSUPP");

  char received = 0;
  std::atomic<intptr_t> receive_result{-2};
  std::thread blocked([&] {
    receive_result.store(recv(pair_fds[0], &received, 1, 0),
                         std::memory_order_release);
  });
  while (aim_bionic_socket_borrowed_descriptors_for_test() == 0) {
    std::this_thread::yield();
  }
  Check(close(pair_fds[0]) == 0, "concurrent close removes token");
  const char sent = 'x';
  Check(send(pair_fds[1], &sent, 1, 0) == 1, "peer unblocks borrowed recv");
  blocked.join();
  Check(receive_result.load(std::memory_order_acquire) == 1 && received == 'x',
        "admitted recv survives close");
  aim_bionic_errno_store(0);
  Check(recv(pair_fds[0], &received, 1, 0) == -1 &&
            aim_bionic_errno_load() == 9,
        "closed token EBADF");
  Check(close(pair_fds[1]) == 0, "close pair peer");
  int32_t replacement[2]{-1, -1};
  Check(pair(replacement) == 0 && replacement[0] != pair_fds[0],
        "generation-tagged token reuse");
  aim_bionic_errno_store(0);
  Check(recv(pair_fds[0], &received, 1, 0) == -1 &&
            aim_bionic_errno_load() == 9,
        "stale token cannot alias replacement");
  Check(close(replacement[0]) == 0 && close(replacement[1]) == 0,
        "close replacement pair");

  int32_t poll_pair[2]{-1, -1};
  Check(pair(poll_pair) == 0, "poll close/use pair");
  int16_t poll_revents = 0;
  std::atomic<int> poll_result{-2};
  std::thread poll_thread([&] {
    poll_result.store(poll_one(poll_pair[0], 5000, &poll_revents),
                      std::memory_order_release);
  });
  while (aim_bionic_socket_borrowed_descriptors_for_test() == 0) {
    std::this_thread::yield();
  }
  Check(close(poll_pair[0]) == 0, "close token during poll");
  Check(send(poll_pair[1], &sent, 1, 0) == 1, "wake borrowed poll descriptor");
  poll_thread.join();
  Check(poll_result.load(std::memory_order_acquire) == 1 &&
            (poll_revents & 0x001) != 0,
        "poll admitted borrow survives close");
  aim_bionic_errno_store(71);
  poll_revents = 0;
  Check(poll_one(poll_pair[0], 1000, &poll_revents) == 1 &&
            poll_revents == 0x020 && aim_bionic_errno_load() == 71,
        "invalid poll token POLLNVAL without errno mutation");
  Check(close(poll_pair[1]) == 0, "close poll peer");

  int32_t interrupt_pair[2]{-1, -1};
  Check(pair(interrupt_pair) == 0, "ppoll EINTR pair");
  struct sigaction action {};
  struct sigaction previous {};
  action.sa_handler = InterruptHandler;
  sigemptyset(&action.sa_mask);
  Check(sigaction(SIGUSR1, &action, &previous) == 0, "install interrupt handler");
  int16_t interrupt_revents = 0;
  std::atomic<int> interrupt_result{-2};
  std::atomic<int> interrupt_errno{-1};
  std::thread interrupted([&] {
    interrupt_result.store(ppoll_one(interrupt_pair[0], 5, &interrupt_revents),
                           std::memory_order_release);
    interrupt_errno.store(aim_bionic_errno_load(),
                          std::memory_order_release);
  });
  while (aim_bionic_socket_borrowed_descriptors_for_test() == 0) {
    std::this_thread::yield();
  }
  Check(pthread_kill(interrupted.native_handle(), SIGUSR1) == 0,
        "deliver ppoll interrupt");
  interrupted.join();
  Check(interrupt_result.load(std::memory_order_acquire) == -1 &&
            interrupt_errno.load(std::memory_order_acquire) == 4 &&
            interrupt_revents == 0,
        "ppoll EINTR maps to Bionic errno");
  Check(sigaction(SIGUSR1, &previous, nullptr) == 0, "restore interrupt handler");
  Check(close(interrupt_pair[0]) == 0 && close(interrupt_pair[1]) == 0,
        "close interrupt pair");
  Check(aim_bionic_socket_live_tokens_for_test() == 0 &&
            aim_bionic_socket_borrowed_descriptors_for_test() == 0,
        "no descriptor leaks");
  Check(errno == EDOM, "host errno preserved");

  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android ELF");
  std::fprintf(stderr,
               "bionic-socket-facade: PASS Android-ELF=yes TCP=yes UDP=yes "
               "fd=virtual+borrowed-close-safe constants=translated "
               "errno=Bionic\n");
  return 0;
}
