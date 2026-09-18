#include "darwin_art_bionic_dns.h"
#include "darwin_art_bionic_binder_fd.h"
#include "darwin_art_bionic_errno.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_socket_broker.h"
#include "darwin_art_elf_loader.h"
#include "../../tests/fd-inheritance-fixture.h"
#include "../../tests/binder-retained-provider-fixture.h"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netdb.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <array>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>
#include <thread>
#include <vector>

namespace {

std::atomic<int> g_filesystem_closes{0};
std::atomic<int> g_binder_closes{0};
DarwinArtBionicSpecialDeviceOpen g_binder_open = nullptr;

extern "C" int darwin_art_bionic_fs_bind_binder_device_open(
    DarwinArtBionicSpecialDeviceOpen callback) {
  g_binder_open = callback;
  return 0;
}

int BinderIoctl(void *, uint64_t object, uint64_t request, void *, int *error) {
  if (object != 42 || request != UINT64_C(0xc0046209)) {
    *error = 22;
    return -1;
  }
  *error = 0;
  return 17;
}

int BinderClose(void *, uint64_t object, int *error) {
  if (object != 42) {
    *error = 22;
    return -1;
  }
  g_binder_closes.fetch_add(1, std::memory_order_relaxed);
  *error = 0;
  return 0;
}

struct HttpFixtureResult {
  int32_t connected_family;
  int32_t send_calls;
  int32_t recv_calls;
  int32_t eintr_retries;
  int32_t last_android_errno;
  uint32_t response_length;
  char response[256];
};

struct PipeFixtureResult {
  int32_t empty_ready;
  int32_t readable_ready;
  int32_t read_revents;
  int32_t last_android_errno;
  intptr_t write_count;
  intptr_t read_count;
  uint8_t value;
};

struct AndroidEpollEvent {
  uint32_t events;
  uint64_t data;
};

static_assert(offsetof(AndroidEpollEvent, data) == 8);
static_assert(sizeof(AndroidEpollEvent) == 16);

void Check(bool condition, const char *message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-socket-broker-adapter: FAIL %s\n", message);
    std::abort();
  }
}

DarwinArtElfResolveStatus Resolve(void *,
                                  const DarwinArtElfSymbolRequest *request,
                                  uintptr_t *output,
                                  DarwinArtElfErrorBuffer *) {
  if (request == nullptr || output == nullptr || request->symbol == nullptr ||
      request->version_soname == nullptr || request->version_name == nullptr ||
      std::strcmp(request->version_soname, "libc.so") != 0 ||
      std::strcmp(request->version_name, "LIBC") != 0) {
    return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
  }
  if (std::strcmp(request->symbol, "__errno") == 0) {
    auto function = darwin_art_bionic_errno_resolve(request->symbol);
    if (function != nullptr) {
      *output = reinterpret_cast<uintptr_t>(function);
      return DARWIN_ART_ELF_RESOLVE_FOUND;
    }
  }
  auto dns = darwin_art_bionic_socket_broker_dns_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (dns != nullptr) {
    *output = reinterpret_cast<uintptr_t>(dns);
    return DARWIN_ART_ELF_RESOLVE_FOUND;
  }
  auto socket = darwin_art_bionic_socket_broker_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (socket != nullptr) {
    *output = reinterpret_cast<uintptr_t>(socket);
    return DARWIN_ART_ELF_RESOLVE_FOUND;
  }
  return DARWIN_ART_ELF_RESOLVE_NOT_FOUND;
}

void Serve(int listener) {
  const int client = accept(listener, nullptr, nullptr);
  Check(client >= 0, "accept loopback client");
  std::array<char, 512> request{};
  size_t used = 0;
  while (used < request.size()) {
    const ssize_t count =
        recv(client, request.data() + used, request.size() - used, 0);
    Check(count > 0, "read HTTP request");
    used += static_cast<size_t>(count);
    if (used >= 4 && request[used - 4] == '\r' && request[used - 3] == '\n' &&
        request[used - 2] == '\r' && request[used - 1] == '\n')
      break;
  }
  Check(std::strstr(request.data(), "GET /acceptance HTTP/1.0\r\n") ==
            request.data(),
        "HTTP request path");
  static constexpr char kResponse[] =
      "HTTP/1.0 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nHELLO";
  size_t sent = 0;
  while (sent < sizeof(kResponse) - 1) {
    const ssize_t count =
        send(client, kResponse + sent, sizeof(kResponse) - 1 - sent, 0);
    Check(count > 0, "write HTTP response");
    sent += static_cast<size_t>(count);
  }
  (void)shutdown(client, SHUT_WR);
  (void)close(client);
  (void)close(listener);
}

} // namespace

extern "C" int darwin_art_bionic_fs_close_core(int) {
  g_filesystem_closes.fetch_add(1, std::memory_order_relaxed);
  darwin_art_bionic_errno_store(9);
  return -1;
}

extern "C" intptr_t darwin_art_bionic_fs_read_core(int, void *, size_t) {
  darwin_art_bionic_errno_store(9);
  return -1;
}

extern "C" intptr_t darwin_art_bionic_fs_write_core(int, const void *, size_t) {
  darwin_art_bionic_errno_store(9);
  return -1;
}

extern "C" int darwin_art_bionic_fs_fcntl_core(int, int, intptr_t) {
  darwin_art_bionic_errno_store(9);
  return -1;
}

extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int, int *) { return 0; }

extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int host_fd) {
  (void)close(host_fd);
  darwin_art_bionic_errno_store(9);
  return -1;
}

struct AndroidSelectTimevalSmoke {
  int64_t seconds;
  int64_t microseconds;
};

struct AndroidSockaddrInSmoke {
  uint16_t family;
  uint16_t port;
  uint32_t address;
  uint8_t zero[8];
};

static_assert(sizeof(AndroidSockaddrInSmoke) == 16);

extern "C" int darwin_art_bionic_socket_broker_select(
    int nfds, void *readfds, void *writefds, void *exceptfds,
    AndroidSelectTimevalSmoke *timeout);

int main(int argc, char **argv) {
  Check(darwin_art::test::InstallNoSpawnFdFixture() == 0,
        "install explicit no-spawn fixture boundary");
  if (argc != 2)
    return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  Check(!bytes.empty() && !input.bad(), "read Android network fixture");
  Check(darwin_art_bionic_socket_broker_close(10000) == -1 &&
            g_filesystem_closes.load(std::memory_order_relaxed) == 1,
        "filesystem close works before network activation");
  Check(darwin_art_bionic_socket_broker_close(0x40000001) == -1 &&
            g_filesystem_closes.load(std::memory_order_relaxed) == 1,
        "stale central token cannot fall through before activation");
  Check(darwin_art_bionic_socket_broker_activate() == 0,
        "activate socket broker owner");
  DarwinArtFdOwnerV1 binder_callbacks{};
  binder_callbacks.abi_version = DARWIN_ART_FD_OWNER_ABI_V7;
  binder_callbacks.struct_size = sizeof(binder_callbacks);
  binder_callbacks.ioctl = &BinderIoctl;
  binder_callbacks.close = &BinderClose;
  DarwinArtFdOwnerHandle binder_owner = 0;
  Check(darwin_art_bionic_binder_fd_install_owner(&binder_callbacks,
                                                   &binder_owner) ==
            DARWIN_ART_FD_BROKER_OK &&
            binder_owner != 0 && g_binder_open != nullptr,
        "install Binder device owner");
  int binder_fd = -1;
  Check(darwin_art_bionic_binder_fd_publish(binder_owner, 42, &binder_fd) ==
            DARWIN_ART_FD_BROKER_OK &&
            binder_fd >= 0,
        "publish Binder guest descriptor");
  int handled = 0;
  int binder_ioctl_result = -1;
  int binder_ioctl_error = -1;
  Check(darwin_art_bionic_fd_broker_ioctl_dispatch(
            binder_fd, UINT32_C(0xc0046209), nullptr, &handled,
            &binder_ioctl_result, &binder_ioctl_error) == 0 &&
            handled == 1 && binder_ioctl_result == 17 &&
            binder_ioctl_error == 0,
        "dispatch Binder ioctl through central FD owner");
  const int binder_duplicate = darwin_art_bionic_socket_broker_dup(binder_fd);
  Check(binder_duplicate >= 0 &&
            darwin_art_bionic_socket_broker_close(binder_fd) == 0 &&
            g_binder_closes.load(std::memory_order_relaxed) == 0 &&
            darwin_art_bionic_socket_broker_close(binder_duplicate) == 0 &&
            g_binder_closes.load(std::memory_order_relaxed) == 1,
        "Binder duplicate shares connection until last close");
  Check(darwin_art_bionic_binder_fd_uninstall_owner(binder_owner) ==
            DARWIN_ART_FD_BROKER_OK && g_binder_open == nullptr,
        "uninstall quiescent Binder device owner");
  Check(darwin_art_bionic_socket_broker_resolve("libc.so", "sendmmsg",
                                                "LIBC") != nullptr,
        "sendmmsg resolver entry");
  using SendMmsg = int (*)(int, DarwinArtAndroidMmsghdr*, uint32_t, int);
  auto sendmmsg = reinterpret_cast<SendMmsg>(
      darwin_art_bionic_socket_broker_resolve("libc.so", "sendmmsg", "LIBC"));
  int32_t mmsg_pair[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, mmsg_pair) == 0,
        "create sendmmsg socketpair");
  char first_payload[] = "first";
  char second_payload[] = "second";
  DarwinArtAndroidIovec first_vector{first_payload, sizeof(first_payload) - 1};
  DarwinArtAndroidIovec second_vector{second_payload, sizeof(second_payload) - 1};
  DarwinArtAndroidMmsghdr messages[2]{};
  messages[0].msg_hdr.vectors = &first_vector;
  messages[0].msg_hdr.vector_count = 1;
  messages[0].msg_len = UINT32_MAX;
  messages[1].msg_hdr.vectors = &second_vector;
  messages[1].msg_hdr.vector_count = 1;
  messages[1].msg_len = UINT32_MAX;
  Check(sendmmsg(mmsg_pair[0], messages, 2, 0) == 2 &&
            messages[0].msg_len == sizeof(first_payload) - 1 &&
            messages[1].msg_len == sizeof(second_payload) - 1,
        "sendmmsg sends each message and reports lengths");
  char received[sizeof(first_payload) + sizeof(second_payload)]{};
  Check(darwin_art_bionic_socket_broker_recv(
            mmsg_pair[1], received, sizeof(first_payload) - 1, 0) ==
            static_cast<intptr_t>(sizeof(first_payload) - 1) &&
            std::memcmp(received, first_payload, sizeof(first_payload) - 1) == 0 &&
            darwin_art_bionic_socket_broker_recv(
                mmsg_pair[1], received + sizeof(first_payload) - 1,
                sizeof(second_payload) - 1, 0) ==
                static_cast<intptr_t>(sizeof(second_payload) - 1) &&
            std::memcmp(received + sizeof(first_payload) - 1, second_payload,
                        sizeof(second_payload) - 1) == 0,
        "sendmmsg preserves stream payload order");
  int32_t short_pair[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, short_pair) == 0 &&
            darwin_art_bionic_socket_broker_fcntl(short_pair[0], 4, 2048) == 0,
        "create nonblocking sendmmsg socketpair");
  std::vector<char> large_payload(4 * 1024 * 1024, 'x');
  DarwinArtAndroidIovec large_vector{large_payload.data(), large_payload.size()};
  DarwinArtAndroidMmsghdr short_messages[2]{};
  short_messages[0].msg_hdr.vectors = &large_vector;
  short_messages[0].msg_hdr.vector_count = 1;
  short_messages[0].msg_len = UINT32_MAX;
  short_messages[1].msg_hdr.vectors = &second_vector;
  short_messages[1].msg_hdr.vector_count = 1;
  short_messages[1].msg_len = UINT32_MAX;
  const int short_count = sendmmsg(short_pair[0], short_messages, 2, 0);
  Check(short_count == 1 && short_messages[0].msg_len > 0 &&
            short_messages[0].msg_len < large_payload.size() &&
            short_messages[1].msg_len == UINT32_MAX,
        "sendmmsg stops after a short nonblocking stream send");
  Check(darwin_art_bionic_socket_broker_close(short_pair[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(short_pair[1]) == 0,
        "close short sendmmsg socketpair");
  DarwinArtAndroidMmsghdr partial[2]{};
  partial[0].msg_hdr.vectors = &first_vector;
  partial[0].msg_hdr.vector_count = 1;
  partial[0].msg_len = UINT32_MAX;
  partial[1].msg_hdr.control = reinterpret_cast<void*>(1);
  partial[1].msg_hdr.control_length = 1;
  partial[1].msg_len = UINT32_MAX;
  darwin_art_bionic_errno_store(77);
  Check(sendmmsg(mmsg_pair[0], partial, 2, 0) == 1 &&
            partial[0].msg_len == sizeof(first_payload) - 1 &&
            partial[1].msg_len == UINT32_MAX &&
            darwin_art_bionic_errno_load() == 77,
        "sendmmsg returns partial count without clobbering errno");
  Check(sendmmsg(mmsg_pair[0], nullptr, 0, 0) == 0,
        "sendmmsg accepts null vector for empty batch");
  Check(sendmmsg(mmsg_pair[0], nullptr, 1, 0) == -1 &&
            darwin_art_bionic_errno_load() == 14,
        "sendmmsg rejects null vector for nonempty batch");
  Check(darwin_art_bionic_socket_broker_close(mmsg_pair[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(mmsg_pair[1]) == 0,
        "close sendmmsg socketpair");
  {
    // The broker's send() translates Android flags to host flags. Verify the
    // socketpair itself also carries SO_NOSIGPIPE: a plain send with no
    // MSG_NOSIGNAL must return EPIPE after its peer closes, rather than
    // terminating this process.
    int32_t broken_pair[2]{-1, -1};
    Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, broken_pair) ==
              0,
          "create broken-peer socketpair");
    Check(darwin_art_bionic_socket_broker_close(broken_pair[1]) == 0,
          "close broken-peer socketpair peer");
    const uint8_t marker = 1;
    darwin_art_bionic_errno_store(7);
    Check(darwin_art_bionic_socket_broker_send(
              broken_pair[0], &marker, sizeof(marker), 0) == -1 &&
              darwin_art_bionic_errno_load() == 32,
          "broken-peer send returns EPIPE without SIGPIPE");
    Check(darwin_art_bionic_socket_broker_close(broken_pair[0]) == 0,
          "close broken-peer socketpair endpoint");
  }
  {
    int udp_receiver = darwin_art_bionic_socket_broker_socket(2, 2, 0);
    int udp_sender = darwin_art_bionic_socket_broker_socket(2, 2, 0);
    Check(udp_receiver >= 0 && udp_sender >= 0,
          "create UDP sendmmsg sockets");
    AndroidSockaddrInSmoke bind_address{2, 0, htonl(INADDR_LOOPBACK), {}};
    Check(darwin_art_bionic_socket_broker_bind(
              udp_receiver, &bind_address, sizeof(bind_address)) == 0,
          "bind UDP sendmmsg receiver");
    AndroidSockaddrInSmoke destination{};
    uint32_t destination_length = sizeof(destination);
    Check(darwin_art_bionic_socket_broker_getsockname(
              udp_receiver, &destination, &destination_length) == 0 &&
              destination_length == sizeof(destination),
          "query UDP sendmmsg destination");
    DarwinArtAndroidMmsghdr datagrams[2]{};
    datagrams[0].msg_hdr.name = &destination;
    datagrams[0].msg_hdr.name_length = destination_length;
    datagrams[0].msg_hdr.vectors = &first_vector;
    datagrams[0].msg_hdr.vector_count = 1;
    datagrams[1].msg_hdr.name = &destination;
    datagrams[1].msg_hdr.name_length = destination_length;
    datagrams[1].msg_hdr.vectors = &second_vector;
    datagrams[1].msg_hdr.vector_count = 1;
    Check(sendmmsg(udp_sender, datagrams, 2, 0) == 2 &&
              datagrams[0].msg_len == sizeof(first_payload) - 1 &&
              datagrams[1].msg_len == sizeof(second_payload) - 1,
          "sendmmsg translates Android UDP destination");
    char datagram[32]{};
    Check(darwin_art_bionic_socket_broker_recvfrom(
              udp_receiver, datagram, sizeof(datagram), 0, nullptr, nullptr) ==
              static_cast<intptr_t>(sizeof(first_payload) - 1) &&
              std::memcmp(datagram, first_payload, sizeof(first_payload) - 1) == 0 &&
              darwin_art_bionic_socket_broker_recvfrom(
                  udp_receiver, datagram, sizeof(datagram), 0, nullptr, nullptr) ==
                  static_cast<intptr_t>(sizeof(second_payload) - 1) &&
              std::memcmp(datagram, second_payload, sizeof(second_payload) - 1) == 0,
          "receive UDP sendmmsg datagrams");
    Check(darwin_art_bionic_socket_broker_close(udp_sender) == 0 &&
              darwin_art_bionic_socket_broker_close(udp_receiver) == 0,
          "close UDP sendmmsg sockets");
  }
  {
    int32_t old_pair[2]{-1, -1};
    int32_t replacement_pair[2]{-1, -1};
    Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, old_pair) == 0 &&
              darwin_art_bionic_socket_broker_socketpair(1, 1, 0,
                                                         replacement_pair) == 0,
          "create sendmmsg replacement pairs");
    const int send_buffer = 4096;
    Check(darwin_art_bionic_socket_broker_setsockopt(
              old_pair[0], 1, 7, &send_buffer, sizeof(send_buffer)) == 0 &&
              darwin_art_bionic_socket_broker_fcntl(old_pair[1], 4,
                                                   2048) == 0,
          "configure sendmmsg replacement timing");
    std::vector<char> replacement_payload(1024 * 1024, 'r');
    DarwinArtAndroidIovec replacement_vector{
        replacement_payload.data(), replacement_payload.size()};
    DarwinArtAndroidMmsghdr replacement_messages[2]{};
    replacement_messages[0].msg_hdr.vectors = &replacement_vector;
    replacement_messages[0].msg_hdr.vector_count = 1;
    replacement_messages[0].msg_len = UINT32_MAX;
    replacement_messages[1].msg_hdr.vectors = &second_vector;
    replacement_messages[1].msg_hdr.vector_count = 1;
    replacement_messages[1].msg_len = UINT32_MAX;
    std::atomic<bool> send_call_started{false};
    std::atomic<bool> replacement_ok{false};
    std::thread replacer([&] {
      while (!send_call_started.load(std::memory_order_acquire))
        std::this_thread::yield();
      std::this_thread::sleep_for(std::chrono::milliseconds(20));
      if (darwin_art_bionic_socket_broker_dup2(replacement_pair[0],
                                                old_pair[0]) != old_pair[0])
        return;
      const size_t expected = replacement_payload.size() +
                              sizeof(second_payload) - 1;
      std::vector<char> received(expected);
      size_t offset = 0;
      const auto deadline = std::chrono::steady_clock::now() +
                            std::chrono::seconds(2);
      while (offset < expected && std::chrono::steady_clock::now() < deadline) {
        const intptr_t result = darwin_art_bionic_socket_broker_recv(
            old_pair[1], received.data() + offset, expected - offset, 0);
        if (result > 0) {
          offset += static_cast<size_t>(result);
        } else if (result < 0 && darwin_art_bionic_errno_load() == 11) {
          std::this_thread::sleep_for(std::chrono::milliseconds(1));
        } else {
          return;
        }
      }
      replacement_ok.store(
          offset == expected &&
              std::memcmp(received.data(), replacement_payload.data(),
                          replacement_payload.size()) == 0 &&
              std::memcmp(received.data() + replacement_payload.size(),
                          second_payload, sizeof(second_payload) - 1) == 0,
          std::memory_order_release);
    });
    send_call_started.store(true, std::memory_order_release);
    Check(sendmmsg(old_pair[0], replacement_messages, 2, 0) == 2 &&
              replacement_messages[0].msg_len == replacement_payload.size() &&
              replacement_messages[1].msg_len == sizeof(second_payload) - 1,
          "sendmmsg completes while descriptor is replaced");
    replacer.join();
    Check(replacement_ok.load(std::memory_order_acquire),
          "sendmmsg retains original descriptor across replacement");
    Check(darwin_art_bionic_socket_broker_close(old_pair[0]) == 0 &&
              darwin_art_bionic_socket_broker_close(old_pair[1]) == 0 &&
              darwin_art_bionic_socket_broker_close(replacement_pair[0]) == 0 &&
              darwin_art_bionic_socket_broker_close(replacement_pair[1]) == 0,
          "close sendmmsg replacement pairs");
  }
  darwin_art_bionic_errno_store(0);
  Check(sendmmsg(-1, nullptr, 0, 0) == -1 &&
            darwin_art_bionic_errno_load() == 9,
        "sendmmsg validates descriptor for empty batch");
  int32_t pipe_descriptors[2]{-1, -1};
  Check(darwin_art_bionic_socket_broker_pipe(pipe_descriptors) == 0,
        "create non-socket sendmmsg descriptor");
  Check(sendmmsg(pipe_descriptors[1], nullptr, 0, 0) == -1 &&
            darwin_art_bionic_errno_load() == 88,
        "sendmmsg rejects non-socket empty batch");
  Check(darwin_art_bionic_socket_broker_close(pipe_descriptors[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(pipe_descriptors[1]) == 0,
        "close non-socket sendmmsg descriptors");
  const int timeout_socket = darwin_art_bionic_socket_broker_socket(2, 1, 0);
  Check(timeout_socket >= 0, "create timeout option socket");
  for (int option : {20, 21}) {
    struct TimeoutValue { int64_t seconds; int64_t micros; };
    const TimeoutValue requested{1, 200000};
    Check(darwin_art_bionic_socket_broker_setsockopt(
              timeout_socket, 1, option, &requested, sizeof(requested)) == 0,
          "set Android 64-bit timeval");
    TimeoutValue received{-1, -1};
    uint32_t received_length = sizeof(received);
    Check(darwin_art_bionic_socket_broker_getsockopt(
              timeout_socket, 1, option, &received, &received_length) == 0 &&
              received_length == sizeof(received) && received.seconds == 1 &&
              received.micros == 200000,
          "Android timeval roundtrip without host padding alias");
    const TimeoutValue invalid{0, 1000000};
    Check(darwin_art_bionic_socket_broker_setsockopt(
              timeout_socket, 1, option, &invalid, sizeof(invalid)) == -1 &&
              darwin_art_bionic_errno_load() == 22,
          "invalid Android timeval rejected");
  }
  Check(darwin_art_bionic_socket_broker_close(timeout_socket) == 0,
        "close timeout option socket");
  Check(darwin_art_bionic_socket_broker_resolve("libc.so", "select",
                                                "LIBC") != nullptr,
        "select resolver entry");

  std::array<uint8_t, 128> select_set{};
  AndroidSelectTimevalSmoke select_timeout{0, 0};
  Check(darwin_art_bionic_socket_broker_select(
                0, &select_set, nullptr, nullptr, &select_timeout) == 0 &&
            select_timeout.seconds == 0 && select_timeout.microseconds == 0,
        "select zero timeout and Android fd_set layout");
  select_timeout = AndroidSelectTimevalSmoke{0, 1'000'000};
  darwin_art_bionic_errno_store(0);
  Check(darwin_art_bionic_socket_broker_select(
                0, nullptr, nullptr, nullptr, &select_timeout) == -1 &&
            darwin_art_bionic_errno_load() == 22,
        "select rejects invalid timeval");
  darwin_art_bionic_errno_store(0);
  Check(darwin_art_bionic_socket_broker_select(
                1025, nullptr, nullptr, nullptr, nullptr) == -1 &&
            darwin_art_bionic_errno_load() == 22,
        "select rejects nfds beyond Android fd_set");
  select_set[0] = 2;
  select_timeout = AndroidSelectTimevalSmoke{0, 0};
  darwin_art_bionic_errno_store(0);
  Check(darwin_art_bionic_socket_broker_select(
                2, &select_set, nullptr, nullptr, &select_timeout) == -1 &&
            darwin_art_bionic_errno_load() == 9,
        "select reports stale descriptor as EBADF");

  int32_t nonblocking_pair[2]{-1, -1};
  int nonblocking = 1;
  int ioctl_handled = 0;
  int ioctl_result = -1;
  int ioctl_errno = 0;
  uint8_t empty_byte = 0;
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, nonblocking_pair) ==
                0 &&
            darwin_art_bionic_socket_broker_ioctl_dispatch(
                nonblocking_pair[0], 0x5421, &nonblocking, &ioctl_handled,
                &ioctl_result, &ioctl_errno) == 0 &&
            ioctl_handled == 1 && ioctl_result == 0 && ioctl_errno == 0 &&
            (darwin_art_bionic_socket_broker_fcntl(nonblocking_pair[0], 3, 0) &
             2048) != 0 &&
            darwin_art_bionic_socket_broker_read(nonblocking_pair[0],
                                                 &empty_byte, 1) == -1 &&
            darwin_art_bionic_errno_load() == 11,
        "FIONBIO makes empty broker socket return EAGAIN");
  nonblocking = 0;
  ioctl_handled = 0;
  ioctl_result = -1;
  ioctl_errno = 0;
  Check(darwin_art_bionic_socket_broker_ioctl_dispatch(
            nonblocking_pair[0], 0x5421, &nonblocking, &ioctl_handled,
            &ioctl_result, &ioctl_errno) == 0 &&
            ioctl_handled == 1 && ioctl_result == 0 && ioctl_errno == 0 &&
            (darwin_art_bionic_socket_broker_fcntl(nonblocking_pair[0], 3, 0) &
             2048) == 0 &&
            darwin_art_bionic_socket_broker_close(nonblocking_pair[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(nonblocking_pair[1]) == 0,
        "FIONBIO clears shared broker socket status");

  int32_t readable_pair[2]{-1, -1};
  const uint32_t readable_value = 0x12345678;
  int available = -1;
  ioctl_handled = 0;
  ioctl_result = -1;
  ioctl_errno = 0;
  Check(darwin_art_bionic_socket_broker_socketpair(1, 1, 0, readable_pair) ==
                0 &&
            darwin_art_bionic_socket_broker_write(
                readable_pair[0], &readable_value, sizeof(readable_value)) ==
                static_cast<intptr_t>(sizeof(readable_value)) &&
            darwin_art_bionic_socket_broker_ioctl_dispatch(
                readable_pair[1], 0x541b, &available, &ioctl_handled,
                &ioctl_result, &ioctl_errno) == 0 &&
            ioctl_handled == 1 && ioctl_result == 0 && ioctl_errno == 0 &&
            available == static_cast<int>(sizeof(readable_value)) &&
            darwin_art_bionic_socket_broker_close(readable_pair[0]) == 0 &&
            darwin_art_bionic_socket_broker_close(readable_pair[1]) == 0,
        "FIONREAD reports bytes queued on broker socket");

  const int listener = socket(AF_INET, SOCK_STREAM, 0);
  Check(listener >= 0, "create host listener");
  sockaddr_in address{};
  address.sin_len = sizeof(address);
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  Check(bind(listener, reinterpret_cast<sockaddr *>(&address),
             sizeof(address)) == 0 &&
            listen(listener, 1) == 0,
        "bind host loopback listener");
  socklen_t address_length = sizeof(address);
  Check(getsockname(listener, reinterpret_cast<sockaddr *>(&address),
                    &address_length) == 0,
        "get host listener port");
  char port[16]{};
  std::snprintf(port, sizeof(port), "%u", ntohs(address.sin_port));
  std::thread server(Serve, listener);

  DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION, &Resolve,
                                  nullptr};
  DarwinArtElfHandle *image = nullptr;
  char message[512]{};
  DarwinArtElfErrorBuffer error{message, sizeof(message), 0};
  Check(darwin_art_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == DARWIN_ART_ELF_OK,
        "load Android HTTP fixture");
  Check(darwin_art_elf_run_initializers(image, &error) == DARWIN_ART_ELF_OK,
        "initialize Android HTTP fixture");
  uintptr_t address_value = 0;
  Check(darwin_art_elf_lookup(image, "NetworkFixtureHttp", &address_value,
                              &error) == DARWIN_ART_ELF_OK &&
            address_value != 0,
        "lookup Android HTTP function");
  using Http =
      int (*)(const char *, const char *, int, int, HttpFixtureResult *);
  auto http = reinterpret_cast<Http>(address_value);
  HttpFixtureResult result{};
  errno = EDOM;
  Check(http("127.0.0.1", port, 2, 1, &result) == 0 && errno == EDOM &&
            result.connected_family == 2 && result.last_android_errno == 0 &&
            std::strstr(result.response, "\r\n\r\nHELLO") != nullptr,
        "Android ELF loopback HTTP through broker");
  server.join();
  address_value = 0;
  Check(darwin_art_elf_lookup(image, "PipeFixtureRoundTrip", &address_value,
                              &error) == DARWIN_ART_ELF_OK &&
            address_value != 0,
        "lookup Android pipe/poll function");
  using PipeRoundTrip = int (*)(PipeFixtureResult *);
  PipeFixtureResult pipe_result{};
  auto pipe_round_trip = reinterpret_cast<PipeRoundTrip>(address_value);
  Check(pipe_round_trip(&pipe_result) == 0 && pipe_result.empty_ready == 0 &&
            pipe_result.readable_ready == 1 &&
            (pipe_result.read_revents & POLLIN) != 0 &&
            pipe_result.write_count == 1 && pipe_result.read_count == 1 &&
            pipe_result.value == 0xa5,
        "Android ELF pipe/read/write/blocking-poll round trip");
  const int event_fd = darwin_art_bionic_socket_broker_eventfd(0, 0x800);
  const int epoll_fd = darwin_art_bionic_socket_broker_epoll_create1(0x80000);
  AndroidEpollEvent registration{1, UINT64_C(0x1122334455667788)};
  AndroidEpollEvent ready{};
  uint64_t event_value = 1;
  Check(event_fd >= 0 && epoll_fd >= 0 &&
            darwin_art_bionic_socket_broker_epoll_ctl(epoll_fd, 1, event_fd,
                                                      &registration) == 0 &&
            darwin_art_bionic_socket_broker_epoll_wait(epoll_fd, &ready, 1,
                                                       0) == 0 &&
            darwin_art_bionic_socket_broker_write(event_fd, &event_value,
                                                  sizeof(event_value)) ==
                sizeof(event_value) &&
            darwin_art_bionic_socket_broker_epoll_wait(epoll_fd, &ready, 1,
                                                       0) == 1 &&
            ready.events == 1 && ready.data == registration.data &&
            darwin_art_bionic_socket_broker_read(event_fd, &event_value,
                                                 sizeof(event_value)) ==
                sizeof(event_value) &&
            darwin_art_bionic_socket_broker_close(event_fd) == 0 &&
            darwin_art_bionic_socket_broker_close(epoll_fd) == 0,
        "eventfd readiness through epoll");
  const int duplicate_source_fd =
      darwin_art_bionic_socket_broker_eventfd(0, 0x800);
  const int duplicate_event_fd = duplicate_source_fd >= 0
                                    ? darwin_art_bionic_socket_broker_dup(
                                          duplicate_source_fd)
                                    : -1;
  uint64_t duplicate_value = 7;
  uint64_t duplicate_read = 0;
  Check(duplicate_source_fd >= 0 && duplicate_event_fd >= 0 &&
            darwin_art_bionic_socket_broker_close(duplicate_source_fd) == 0 &&
            darwin_art_bionic_socket_broker_write(duplicate_event_fd,
                                                  &duplicate_value,
                                                  sizeof(duplicate_value)) ==
                sizeof(duplicate_value) &&
            darwin_art_bionic_socket_broker_read(duplicate_event_fd,
                                                 &duplicate_read,
                                                 sizeof(duplicate_read)) ==
                sizeof(duplicate_read) &&
            duplicate_read == duplicate_value &&
            darwin_art_bionic_socket_broker_close(duplicate_event_fd) == 0,
        "eventfd broker dup survives original close");
  Check(darwin_art_bionic_socket_broker_live_objects() == 0,
        "all central socket objects closed");
  const int counter_fd =
      darwin_art_bionic_socket_broker_eventfd(0, 0x800);
  uint64_t one = 1;
  uint64_t two = 2;
  uint64_t counter_value = 0;
  Check(counter_fd >= 0 &&
            darwin_art_bionic_socket_broker_write(counter_fd, &one,
                                                  sizeof(one)) == sizeof(one) &&
            darwin_art_bionic_socket_broker_write(counter_fd, &two,
                                                  sizeof(two)) == sizeof(two) &&
            darwin_art_bionic_socket_broker_read(counter_fd, &counter_value,
                                                 sizeof(counter_value)) ==
                sizeof(counter_value) &&
            counter_value == 3 &&
            darwin_art_bionic_socket_broker_read(counter_fd, &counter_value,
                                                 sizeof(counter_value)) == -1 &&
            darwin_art_bionic_socket_broker_close(counter_fd) == 0,
        "eventfd counter coalesces writes and drains once");
  const int semaphore_fd =
      darwin_art_bionic_socket_broker_eventfd(2, 0x801);
  Check(semaphore_fd >= 0 &&
            darwin_art_bionic_socket_broker_read(semaphore_fd, &counter_value,
                                                 sizeof(counter_value)) ==
                sizeof(counter_value) &&
            counter_value == 1 &&
            darwin_art_bionic_socket_broker_read(semaphore_fd, &counter_value,
                                                 sizeof(counter_value)) ==
                sizeof(counter_value) &&
            counter_value == 1 &&
            darwin_art_bionic_socket_broker_read(semaphore_fd, &counter_value,
                                                 sizeof(counter_value)) == -1 &&
            darwin_art_bionic_socket_broker_close(semaphore_fd) == 0,
        "eventfd semaphore returns one per read");
  Check(darwin_art_bionic_socket_broker_close(123) == -1,
        "generic close fell back to filesystem owner");
  Check(g_filesystem_closes.load(std::memory_order_relaxed) == 2,
        "filesystem close delegation count");
  Check(darwin_art_bionic_socket_broker_close(0x40000001) == -1 &&
            g_filesystem_closes.load(std::memory_order_relaxed) == 2,
        "stale broker-shaped close cannot alias filesystem token");
  Check(darwin_art_bionic_dns_live_results_for_test() == 0 &&
            darwin_art_bionic_dns_retired_results_for_test() == 1,
        "DNS result retired");
  darwin_art_bionic_dns_reset_for_test();
  Check(darwin_art_elf_unload(&image, &error) == DARWIN_ART_ELF_OK,
        "unload Android HTTP fixture");
  Check(darwin_art_bionic_socket_broker_deactivate() == 0,
        "deactivate quiescent socket owner");
  TestRetainedBinderProvider();
  Check(darwin_art_bionic_socket_broker_close(10001) == -1 &&
            g_filesystem_closes.load(std::memory_order_relaxed) == 3,
        "filesystem close works after network deactivation");
  for (int iteration = 0; iteration < 100; ++iteration) {
    Check(darwin_art_bionic_socket_broker_activate() == 0,
          "reactivate race owner");
    std::atomic<bool> start{false};
    std::thread creator([&] {
      while (!start.load(std::memory_order_acquire))
        std::this_thread::yield();
      const int descriptor = darwin_art_bionic_socket_broker_socket(2, 1, 0);
      if (descriptor >= 0)
        Check(darwin_art_bionic_socket_broker_close(descriptor) == 0,
              "close racing created socket");
    });
    start.store(true, std::memory_order_release);
    const int first_deactivate = darwin_art_bionic_socket_broker_deactivate();
    creator.join();
    if (first_deactivate != 0)
      Check(darwin_art_bionic_socket_broker_deactivate() == 0,
            "retry deactivate after admitted socket");
    Check(darwin_art_bionic_socket_broker_live_objects() == 0,
          "deactivate race leaked object");
  }
  Check(darwin_art_bionic_socket_broker_activate() == 0,
        "activate DNS lifetime owner");
  using GetHostByName = struct hostent *(*)(const char *);
  using GetServByName = struct servent *(*)(const char *, const char *);
  using GetHerrno = int *(*)();
  auto gethostbyname = reinterpret_cast<GetHostByName>(
      darwin_art_bionic_socket_broker_dns_resolve("libc.so", "gethostbyname",
                                                  "LIBC"));
  auto getservbyname = reinterpret_cast<GetServByName>(
      darwin_art_bionic_socket_broker_dns_resolve("libc.so", "getservbyname",
                                                  "LIBC"));
  auto get_h_errno = reinterpret_cast<GetHerrno>(
      darwin_art_bionic_socket_broker_dns_resolve("libc.so", "__get_h_errno",
                                                  "LIBC"));
  Check(gethostbyname != nullptr && getservbyname != nullptr &&
            get_h_errno != nullptr,
        "namespace broker legacy DNS routes");
  struct hostent *host = gethostbyname("127.0.0.1");
  Check(host != nullptr && host->h_addrtype == AF_INET && host->h_length == 4 &&
            host->h_addr_list != nullptr && host->h_addr_list[0] != nullptr,
        "namespace broker numeric gethostbyname");
  Check(gethostbyname("darwin-art-missing-host.invalid") == nullptr &&
            (*get_h_errno() == HOST_NOT_FOUND || *get_h_errno() == NO_DATA),
        "namespace broker gethostbyname h_errno failure");
  struct servent *service = getservbyname("https", "tcp");
  Check(service != nullptr && service->s_port != 0 && service->s_proto != nullptr,
        "namespace broker getservbyname");
  struct hostent *main_host = gethostbyname("localhost");
  struct servent *main_service = getservbyname("https", "tcp");
  Check(main_host != nullptr && main_service != nullptr,
        "namespace broker thread-local setup");
  int *main_h_errno = get_h_errno();
  std::atomic<bool> legacy_dns_thread_local{false};
  std::thread legacy_dns_worker([&] {
    struct hostent *worker_host = gethostbyname("127.0.0.1");
    struct servent *worker_service = getservbyname("https", "tcp");
    int *worker_h_errno = get_h_errno();
    legacy_dns_thread_local.store(
        worker_host != nullptr && worker_service != nullptr &&
            worker_host != main_host && worker_service != main_service &&
            worker_h_errno != main_h_errno,
        std::memory_order_release);
  });
  legacy_dns_worker.join();
  Check(legacy_dns_thread_local.load(std::memory_order_acquire),
        "namespace broker legacy DNS thread-local storage");
  DarwinArtAndroidAddrinfo hints{};
  hints.ai_flags = 0x4 | 0x8;
  hints.ai_family = 2;
  hints.ai_socktype = 1;
  DarwinArtAndroidAddrinfo *dns_result = nullptr;
  using GetAddrInfo =
      int (*)(const char *, const char *, const DarwinArtAndroidAddrinfo *,
              DarwinArtAndroidAddrinfo **);
  using FreeAddrInfo = void (*)(DarwinArtAndroidAddrinfo *);
  auto getaddrinfo =
      reinterpret_cast<GetAddrInfo>(darwin_art_bionic_socket_broker_dns_resolve(
          "libc.so", "getaddrinfo", "LIBC"));
  auto freeaddrinfo = reinterpret_cast<FreeAddrInfo>(
      darwin_art_bionic_socket_broker_dns_resolve("libc.so", "freeaddrinfo",
                                                  "LIBC"));
  Check(getaddrinfo != nullptr && freeaddrinfo != nullptr &&
            getaddrinfo("127.0.0.1", "80", &hints, &dns_result) == 0 &&
            dns_result != nullptr,
        "lease DNS result");
  Check(darwin_art_bionic_socket_broker_deactivate() == -1,
        "DNS result blocks reset and deactivate");
  freeaddrinfo(dns_result);
  Check(darwin_art_bionic_socket_broker_deactivate() == 0 &&
            darwin_art_bionic_dns_live_results_for_test() == 0 &&
            darwin_art_bionic_dns_retired_results_for_test() == 0,
        "DNS free drains before reset and deactivate");
  std::fprintf(stderr, "bionic-socket-broker-adapter: PASS Android-ELF=yes "
                       "HTTP=127.0.0.1 pipe-poll=blocking eventfd-epoll=yes "
                       "central-token=yes binder-fd=ioctl+dup-last-close owner=v7 "
                       "close=generic "
                       "DNS=retired deactivate-race=100 host-errno=preserved "
                       "Internet=no\n");
  return 0;
}
