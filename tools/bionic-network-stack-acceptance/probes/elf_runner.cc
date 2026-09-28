#include "aim_bionic_dns.h"
#include "aim_bionic_errno.h"
#include "aim_bionic_socket.h"
#include "aim_elf_loader.h"

#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <pthread.h>
#include <signal.h>
#include <sys/socket.h>
#include <unistd.h>

#include <array>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

struct HttpFixtureResult {
  int32_t connected_family;
  int32_t send_calls;
  int32_t recv_calls;
  int32_t eintr_retries;
  int32_t last_android_errno;
  uint32_t response_length;
  char response[256];
};

void Check(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-network-stack-acceptance: FAIL %s\n", message);
    std::abort();
  }
}

void InterruptHandler(int) {}

bool IsLoopbackPeer(const sockaddr_storage& storage) {
  if (storage.ss_family == AF_INET) {
    const auto* address = reinterpret_cast<const sockaddr_in*>(&storage);
    return ntohl(address->sin_addr.s_addr) == INADDR_LOOPBACK;
  }
  if (storage.ss_family == AF_INET6) {
    const auto* address = reinterpret_cast<const sockaddr_in6*>(&storage);
    return IN6_IS_ADDR_LOOPBACK(&address->sin6_addr);
  }
  return false;
}

class LoopbackServer {
 public:
  LoopbackServer(int family, int expected_clients, bool gate_first)
      : family_(family), expected_clients_(expected_clients),
        gate_first_(gate_first) {
    listener_ = socket(family_, SOCK_STREAM, 0);
    Check(listener_ != -1, "create host listener");
    int one = 1;
    Check(setsockopt(listener_, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)) == 0,
          "host listener SO_REUSEADDR");
    if (family_ == AF_INET6) {
      Check(setsockopt(listener_, IPPROTO_IPV6, IPV6_V6ONLY, &one, sizeof(one)) == 0,
            "host listener IPV6_V6ONLY");
      sockaddr_in6 address{};
      address.sin6_len = sizeof(address);
      address.sin6_family = AF_INET6;
      address.sin6_addr = in6addr_loopback;
      Check(bind(listener_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
            "bind IPv6 loopback listener");
      socklen_t length = sizeof(address);
      Check(getsockname(listener_, reinterpret_cast<sockaddr*>(&address), &length) == 0,
            "IPv6 listener port");
      port_ = ntohs(address.sin6_port);
    } else {
      sockaddr_in address{};
      address.sin_len = sizeof(address);
      address.sin_family = AF_INET;
      address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
      Check(bind(listener_, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
            "bind IPv4 loopback listener");
      socklen_t length = sizeof(address);
      Check(getsockname(listener_, reinterpret_cast<sockaddr*>(&address), &length) == 0,
            "IPv4 listener port");
      port_ = ntohs(address.sin_port);
    }
    Check(port_ != 0 && listen(listener_, expected_clients_) == 0,
          "listen on host loopback");
    worker_ = std::thread([this] { Serve(); });
  }

  ~LoopbackServer() {
    if (worker_.joinable()) {
      ReleaseFirstResponse();
      (void)shutdown(listener_, SHUT_RDWR);
      (void)close(listener_);
      worker_.join();
    }
  }

  std::string port_string() const {
    char value[16]{};
    std::snprintf(value, sizeof(value), "%u", static_cast<unsigned>(port_));
    return value;
  }

  void WaitForFirstRequest() {
    std::unique_lock<std::mutex> lock(mutex_);
    condition_.wait(lock, [this] { return first_request_ready_ || failed_; });
    Check(!failed_, "server request read");
  }

  void ReleaseFirstResponse() {
    std::lock_guard<std::mutex> lock(mutex_);
    release_first_response_ = true;
    condition_.notify_all();
  }

  void Join() {
    if (worker_.joinable()) worker_.join();
    Check(!failed_, "server completed without protocol error");
    Check(served_.load(std::memory_order_acquire) == expected_clients_,
          "server served expected clients");
  }

 private:
  void FailServer() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      failed_ = true;
      condition_.notify_all();
    }
  }

  bool ReceiveRequest(int client) {
    std::array<char, 512> request{};
    size_t used = 0;
    while (used < request.size()) {
      const size_t capacity = std::min<size_t>(4, request.size() - used);
      const ssize_t count = recv(client, request.data() + used, capacity, 0);
      if (count < 0 && errno == EINTR) continue;
      if (count <= 0) return false;
      used += static_cast<size_t>(count);
      if (used >= 4 && request[used - 4] == '\r' && request[used - 3] == '\n' &&
          request[used - 2] == '\r' && request[used - 1] == '\n') {
        break;
      }
    }
    static constexpr char kPrefix[] = "GET /acceptance HTTP/1.0\r\n";
    if (used < sizeof(kPrefix) - 1) return false;
    for (size_t index = 0; index < sizeof(kPrefix) - 1; ++index) {
      if (request[index] != kPrefix[index]) return false;
    }
    return true;
  }

  bool SendResponse(int client) {
    static constexpr char kResponse[] =
        "HTTP/1.0 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nHELLO";
    size_t sent = 0;
    while (sent < sizeof(kResponse) - 1) {
      const size_t length = std::min<size_t>(2, sizeof(kResponse) - 1 - sent);
      const ssize_t count = send(client, kResponse + sent, length, 0);
      if (count < 0 && errno == EINTR) continue;
      if (count <= 0) return false;
      sent += static_cast<size_t>(count);
    }
    return shutdown(client, SHUT_WR) == 0;
  }

  void Serve() {
    for (int index = 0; index < expected_clients_; ++index) {
      sockaddr_storage peer{};
      socklen_t peer_length = sizeof(peer);
      const int client = accept(listener_, reinterpret_cast<sockaddr*>(&peer),
                                &peer_length);
      if (client == -1 || !IsLoopbackPeer(peer) || !ReceiveRequest(client)) {
        if (client != -1) (void)close(client);
        FailServer();
        break;
      }
      if (gate_first_ && index == 0) {
        std::unique_lock<std::mutex> lock(mutex_);
        first_request_ready_ = true;
        condition_.notify_all();
        condition_.wait(lock, [this] { return release_first_response_; });
      }
      if (!SendResponse(client)) {
        (void)close(client);
        FailServer();
        break;
      }
      (void)close(client);
      served_.fetch_add(1, std::memory_order_release);
    }
    const int descriptor = listener_;
    listener_ = -1;
    if (descriptor != -1) (void)close(descriptor);
  }

  int family_;
  int expected_clients_;
  bool gate_first_;
  int listener_ = -1;
  uint16_t port_ = 0;
  std::thread worker_;
  std::atomic<int> served_{0};
  std::mutex mutex_;
  std::condition_variable condition_;
  bool first_request_ready_ = false;
  bool release_first_response_ = false;
  bool failed_ = false;
};

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr || request->symbol == nullptr ||
      request->version_soname == nullptr || request->version_name == nullptr ||
      std::strcmp(request->version_soname, "libc.so") != 0 ||
      std::strcmp(request->version_name, "LIBC") != 0) {
    return AIM_ELF_RESOLVE_NOT_FOUND;
  }
  if (std::strcmp(request->symbol, "__errno") == 0) {
    const auto function = aim_bionic_errno_resolve(request->symbol);
    if (function != nullptr) {
      *output = reinterpret_cast<uintptr_t>(function);
      return AIM_ELF_RESOLVE_FOUND;
    }
  }
  const auto dns = aim_bionic_dns_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (dns != nullptr) {
    *output = reinterpret_cast<uintptr_t>(dns);
    return AIM_ELF_RESOLVE_FOUND;
  }
  const auto socket_function = aim_bionic_socket_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (socket_function != nullptr) {
    *output = reinterpret_cast<uintptr_t>(socket_function);
    return AIM_ELF_RESOLVE_FOUND;
  }
  return AIM_ELF_RESOLVE_NOT_FOUND;
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

bool ValidHttpResult(const HttpFixtureResult& result, int expected_family,
                     bool require_interrupt) {
  return result.connected_family == expected_family && result.send_calls >= 10 &&
         result.recv_calls >= 10 &&
         (!require_interrupt || result.eintr_retries >= 1) &&
         result.last_android_errno == 0 && result.response_length > 60 &&
         std::strstr(result.response, "HTTP/1.0 200 OK\r\n") == result.response &&
         std::strstr(result.response, "\r\n\r\nHELLO") != nullptr;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  const char* exact_symbols[] = {"__errno", "close", "connect", "freeaddrinfo",
                                 "getaddrinfo", "recv", "send", "socket"};
  for (const char* symbol : exact_symbols) {
    AimElfSymbolRequest request{AIM_ELF_ABI_VERSION, symbol,
                                      "libc.so", "LIBC", 0, 0, 0, nullptr, 0};
    uintptr_t address = 0;
    Check(Resolve(nullptr, &request, &address, nullptr) ==
              AIM_ELF_RESOLVE_FOUND &&
              address != 0,
          "combined exact resolver");
  }
  AimElfSymbolRequest rejected{AIM_ELF_ABI_VERSION, "gethostbyname",
                                     "libc.so", "LIBC", 0, 0, 0, nullptr, 0};
  uintptr_t rejected_address = 0;
  Check(Resolve(nullptr, &rejected, &rejected_address, nullptr) ==
            AIM_ELF_RESOLVE_NOT_FOUND,
        "combined resolver closed");

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load Android HTTP ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "run HTTP ELF initializers");
  using Http = int (*)(const char*, const char*, int, int, HttpFixtureResult*);
  Http http = Lookup<Http>(image, "NetworkFixtureHttp");

  struct sigaction action {};
  struct sigaction previous {};
  action.sa_handler = InterruptHandler;
  sigemptyset(&action.sa_mask);
  Check(sigaction(SIGUSR1, &action, &previous) == 0,
        "install HTTP interrupt handler");

  LoopbackServer interrupted_server(AF_INET, 1, true);
  const std::string interrupted_port = interrupted_server.port_string();
  HttpFixtureResult interrupted_result{};
  std::atomic<int> interrupted_status{-1};
  std::atomic<bool> interrupted_errno_ok{false};
  std::thread interrupted_client([&] {
    errno = EDOM;
    interrupted_status.store(
        http("127.0.0.1", interrupted_port.c_str(), 2, 1,
             &interrupted_result),
        std::memory_order_release);
    interrupted_errno_ok.store(errno == EDOM, std::memory_order_release);
  });
  interrupted_server.WaitForFirstRequest();
  while (aim_bionic_socket_borrowed_descriptors_for_test() == 0) {
    std::this_thread::yield();
  }
  Check(pthread_kill(interrupted_client.native_handle(), SIGUSR1) == 0,
        "interrupt Android recv");
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  interrupted_server.ReleaseFirstResponse();
  interrupted_client.join();
  interrupted_server.Join();
  Check(interrupted_status.load(std::memory_order_acquire) == 0 &&
            interrupted_errno_ok.load(std::memory_order_acquire) &&
            ValidHttpResult(interrupted_result, 2, true),
        "IPv4 numeric HTTP with EINTR and short I/O");

  LoopbackServer ipv6_server(AF_INET6, 1, false);
  const std::string ipv6_port = ipv6_server.port_string();
  HttpFixtureResult ipv6_result{};
  errno = EDOM;
  Check(http("::1", ipv6_port.c_str(), 10, 1, &ipv6_result) == 0 &&
            errno == EDOM && ValidHttpResult(ipv6_result, 10, false),
        "IPv6 numeric HTTP");
  ipv6_server.Join();

  LoopbackServer localhost_server(AF_INET, 1, false);
  const std::string localhost_port = localhost_server.port_string();
  HttpFixtureResult localhost_result{};
  errno = EDOM;
  const int localhost_status =
      http("localhost", localhost_port.c_str(), 0, 0, &localhost_result);
  if (localhost_status != 0 || !ValidHttpResult(localhost_result, 2, false)) {
    std::fprintf(stderr,
                 "localhost diagnostic status=%d family=%d send=%d recv=%d "
                 "retry=%d android-errno=%d response=%u\n",
                 localhost_status, localhost_result.connected_family,
                 localhost_result.send_calls, localhost_result.recv_calls,
                 localhost_result.eintr_retries,
                 localhost_result.last_android_errno,
                 localhost_result.response_length);
  }
  Check(localhost_status == 0 &&
            errno == EDOM && ValidHttpResult(localhost_result, 2, false),
        "localhost DNS to IPv4 HTTP fallback");
  localhost_server.Join();

  constexpr size_t kConcurrentClients = 4;
  LoopbackServer concurrent_server(AF_INET, kConcurrentClients, false);
  const std::string concurrent_port = concurrent_server.port_string();
  std::array<HttpFixtureResult, kConcurrentClients> concurrent_results{};
  std::array<std::atomic<int>, kConcurrentClients> concurrent_status{};
  std::array<std::atomic<bool>, kConcurrentClients> concurrent_errno{};
  std::array<std::thread, kConcurrentClients> clients;
  for (size_t index = 0; index < kConcurrentClients; ++index) {
    concurrent_status[index].store(-1);
    concurrent_errno[index].store(false);
    clients[index] = std::thread([&, index] {
      errno = EDOM;
      concurrent_status[index].store(
          http("127.0.0.1", concurrent_port.c_str(), 2, 1,
               &concurrent_results[index]),
          std::memory_order_release);
      concurrent_errno[index].store(errno == EDOM, std::memory_order_release);
    });
  }
  for (std::thread& client : clients) client.join();
  concurrent_server.Join();
  for (size_t index = 0; index < kConcurrentClients; ++index) {
    Check(concurrent_status[index].load(std::memory_order_acquire) == 0 &&
              concurrent_errno[index].load(std::memory_order_acquire) &&
              ValidHttpResult(concurrent_results[index], 2, false),
          "concurrent IPv4 HTTP client");
  }

  Check(sigaction(SIGUSR1, &previous, nullptr) == 0,
        "restore HTTP interrupt handler");
  Check(aim_bionic_socket_live_tokens_for_test() == 0 &&
            aim_bionic_socket_borrowed_descriptors_for_test() == 0,
        "all guest socket tokens closed");
  Check(aim_bionic_dns_live_results_for_test() == 0 &&
            aim_bionic_dns_retired_results_for_test() == 7,
        "every DNS result freed and quarantined");
  aim_bionic_dns_reset_for_test();
  aim_bionic_socket_reset_for_test();
  Check(aim_bionic_dns_retired_results_for_test() == 0,
        "DNS ownership reclaimed after clients quiesce");

  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android HTTP ELF");
  std::fprintf(stderr,
               "bionic-network-stack-acceptance: PASS Android-ELF=yes "
               "HTTP/1.0=IPv4+IPv6+localhost concurrent=4 EINTR=yes "
               "short-IO=3 DNS-free=yes server-teardown=yes Internet=no\n");
  return 0;
}
