#include "output_owner.h"

#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace darwin_art::surfaceflinger {
namespace {

// Absolute control deadline bounds trickled fragments as well as EAGAIN;
// unlike producer fences this operation is only an IPC ownership handshake.
bool Transfer(int fd, void* data, size_t size, bool sending,
              std::chrono::steady_clock::time_point deadline) {
  auto* bytes = static_cast<unsigned char*>(data);
  while (size != 0) {
    const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
        deadline - std::chrono::steady_clock::now()).count();
    if (remaining <= 0) return false;
    pollfd waiter{fd, static_cast<short>(sending ? POLLOUT : POLLIN), 0};
    const int ready = poll(&waiter, 1, static_cast<int>(remaining));
    if (ready < 0 && errno == EINTR) continue;
    if (ready <= 0 || (waiter.revents & (POLLERR | POLLNVAL)) != 0) return false;
    const ssize_t count = sending ? send(fd, bytes, size, 0)
                                  : recv(fd, bytes, size, 0);
    if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
    if (count <= 0) return false;
    bytes += count;
    size -= static_cast<size_t>(count);
  }
  return true;
}

}  // namespace

std::unique_ptr<OutputOwner> OutputOwner::Register(const char* endpoint,
                                                  OutputRequest backing) {
  if (endpoint == nullptr || endpoint[0] == '\0') return {};
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  if (std::strlen(endpoint) >= sizeof(address.sun_path)) return {};
  std::memcpy(address.sun_path, endpoint, std::strlen(endpoint) + 1);
  auto owner = std::unique_ptr<OutputOwner>(new OutputOwner(-1));
  const int descriptor = socket(AF_UNIX, SOCK_STREAM, 0);
  if (descriptor < 0) return {};
  owner->descriptor_ = descriptor;
  if (fcntl(descriptor, F_SETFD, FD_CLOEXEC) < 0) return {};
#ifdef F_SETNOSIGPIPE
  if (fcntl(descriptor, F_SETNOSIGPIPE, 1) < 0) return {};
#endif
  const int flags = fcntl(descriptor, F_GETFL);
  if (flags < 0 || fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) < 0) return {};
  if (connect(descriptor, reinterpret_cast<sockaddr*>(&address), sizeof(address)) < 0) {
    if (errno != EINPROGRESS) return {};
    pollfd waiter{descriptor, POLLOUT, 0};
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    for (;;) {
      const auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
          deadline - std::chrono::steady_clock::now()).count();
      if (remaining <= 0) return {};
      const int ready = poll(&waiter, 1, static_cast<int>(remaining));
      if (ready < 0 && errno == EINTR) continue;
      int error = 0;
      socklen_t size = sizeof(error);
      if (ready <= 0 || getsockopt(descriptor, SOL_SOCKET, SO_ERROR, &error, &size) < 0 ||
          error != 0) return {};
      break;
    }
  }
  backing.operation = OutputOperation::Register;
  backing.token = {};
  backing.generation = 0;
  if (!owner->Exchange(backing)) return {};
  return owner;
}

bool OutputOwner::Exchange(OutputRequest request) {
  if (descriptor_ < 0) return false;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  OutputResponse response{};
  if (!Transfer(descriptor_, &request, sizeof(request), true, deadline) ||
      !Transfer(descriptor_, &response, sizeof(response), false, deadline) ||
      response.magic != kOutputMagic || response.version != kOutputProtocolVersion) {
    // A missing reply makes server commit ambiguous. Retiring the connection
    // prevents publishing old local storage against an uncertain generation.
    Retire();
    return false;
  }
  // An explicit rejection did not commit a replacement. Keep the previous
  // output/generation live; only transport ambiguity requires FD retirement.
  if (response.status != 0) return false;
  if (response.token.server_instance == 0 || response.token.serial == 0 ||
      response.generation == 0) {
    Retire();
    return false;
  }
  const bool identity_valid = request.operation == OutputOperation::Register
      ? response.generation == 1
      : response.token == token_ && response.generation == generation_ + 1;
  if (!identity_valid) {
    Retire();
    return false;
  }
  token_ = response.token;
  generation_ = response.generation;
  return true;
}

bool OutputOwner::Replace(OutputRequest backing) {
  backing.operation = OutputOperation::Replace;
  backing.token = token_;
  backing.generation = generation_;
  return Exchange(backing);
}

void OutputOwner::Retire() {
  if (descriptor_ >= 0) close(descriptor_);
  descriptor_ = -1;
  token_ = {};
  generation_ = 0;
}

OutputOwner::~OutputOwner() { Retire(); }

}  // namespace darwin_art::surfaceflinger
