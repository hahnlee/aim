#include "runtime_network_probe.h"

#include <arpa/inet.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <array>
#include <cstring>

BoundedLoopbackHttpServer::~BoundedLoopbackHttpServer() { Stop(); }

bool BoundedLoopbackHttpServer::Start() {
  listener_ = socket(AF_INET, SOCK_STREAM, 0);
  if (listener_ < 0) return false;
  int enabled = 1;
  (void)setsockopt(listener_, SOL_SOCKET, SO_REUSEADDR, &enabled,
                   sizeof(enabled));
  sockaddr_in address{};
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  address.sin_port = 0;
  if (bind(listener_, reinterpret_cast<const sockaddr*>(&address),
           sizeof(address)) != 0 ||
      listen(listener_, 1) != 0) {
    close(listener_);
    listener_ = -1;
    return false;
  }
  socklen_t length = sizeof(address);
  if (getsockname(listener_, reinterpret_cast<sockaddr*>(&address), &length) !=
      0) {
    close(listener_);
    listener_ = -1;
    return false;
  }
  port_ = ntohs(address.sin_port);
  try {
    thread_ = std::thread([this] { Serve(); });
  } catch (...) {
    close(listener_);
    listener_ = -1;
    port_ = 0;
    return false;
  }
  return true;
}

int BoundedLoopbackHttpServer::port() const { return port_; }

bool BoundedLoopbackHttpServer::Stop() {
  if (joined_) return success_.load(std::memory_order_acquire);
  cancelled_.store(true, std::memory_order_release);
  {
    std::lock_guard<std::mutex> lock(listener_mutex_);
    if (listener_ >= 0) (void)shutdown(listener_, SHUT_RDWR);
  }
  if (thread_.joinable()) thread_.join();
  joined_ = true;
  return success_.load(std::memory_order_acquire);
}

void BoundedLoopbackHttpServer::Serve() {
  pollfd pending{listener_, POLLIN, 0};
  for (int attempt = 0; attempt < 50; ++attempt) {
    if (cancelled_.load(std::memory_order_acquire)) break;
    const int ready = poll(&pending, 1, 100);
    if (ready < 0 && errno == EINTR) continue;
    if (ready <= 0 || (pending.revents & POLLIN) == 0) continue;
    const int client = accept(listener_, nullptr, nullptr);
    if (client < 0) break;
    timeval timeout{2, 0};
    (void)setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    (void)setsockopt(client, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    std::array<char, 512> request{};
    size_t used = 0;
    while (used + 1 < request.size()) {
      const ssize_t count =
          recv(client, request.data() + used, request.size() - 1 - used, 0);
      if (count < 0 && errno == EINTR) continue;
      if (count <= 0) break;
      used += static_cast<size_t>(count);
      request[used] = '\0';
      if (std::strstr(request.data(), "\r\n\r\n") != nullptr) break;
    }
    static constexpr char kRequest[] =
        "GET /runtime HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    static constexpr char kResponse[] =
        "HTTP/1.0 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";
    bool valid = used == sizeof(kRequest) - 1 &&
                 std::memcmp(request.data(), kRequest, sizeof(kRequest) - 1) ==
                     0;
    size_t sent = 0;
    while (valid && sent < sizeof(kResponse) - 1) {
      const ssize_t count =
          send(client, kResponse + sent, sizeof(kResponse) - 1 - sent, 0);
      if (count < 0 && errno == EINTR) continue;
      if (count <= 0) {
        valid = false;
        break;
      }
      sent += static_cast<size_t>(count);
    }
    (void)shutdown(client, SHUT_WR);
    close(client);
    success_.store(valid && sent == sizeof(kResponse) - 1,
                   std::memory_order_release);
    break;
  }
  {
    std::lock_guard<std::mutex> lock(listener_mutex_);
    close(listener_);
    listener_ = -1;
  }
}
