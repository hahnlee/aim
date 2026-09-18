#include "service_ingress.h"

#include "composition_protocol.h"

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

namespace darwin_art::surfaceflinger {

ServiceIngress::~ServiceIngress() {
  while (!peers_.empty()) Close(peers_.begin()->first);
}

void ServiceIngress::Close(int descriptor) {
  const auto peer = peers_.find(descriptor);
  if (peer == peers_.end()) return;
  drop_(descriptor);
  close(descriptor);
  peers_.erase(peer);
}

bool ServiceIngress::Add(int descriptor) {
  if (descriptor < 0) return false;
  const int flags = fcntl(descriptor, F_GETFL);
  bool configured = flags >= 0 &&
      fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) == 0 &&
      fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0;
#ifdef F_SETNOSIGPIPE
  configured = configured && fcntl(descriptor, F_SETNOSIGPIPE, 1) == 0;
#endif
  if (!configured || peers_.size() >= 256) {
    close(descriptor);
    return false;
  }
  try {
    auto [peer, inserted] = peers_.try_emplace(descriptor);
    if (!inserted) return false;  // caller cannot own an already-owned FD
    peer->second.deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  } catch (...) {
    close(descriptor);
    throw;
  }
  return true;
}

void ServiceIngress::AppendPoll(std::vector<pollfd>* waiters) const {
  for (const auto& [descriptor, peer] : peers_) {
    waiters->push_back({descriptor,
                       static_cast<short>(peer.replying ? POLLOUT : POLLIN), 0});
  }
}

void ServiceIngress::ExpirePending() {
  const auto now = std::chrono::steady_clock::now();
  for (auto peer = peers_.begin(); peer != peers_.end();) {
    const int descriptor = peer->first;
    const bool expired = !peer->second.registered && now >= peer->second.deadline;
    ++peer;
    if (expired) Close(descriptor);
  }
}

void ServiceIngress::Ready(int descriptor, short events) {
  auto found = peers_.find(descriptor);
  if (found == peers_.end()) return;
  if ((events & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
    Close(descriptor);
    return;
  }
  auto& peer = found->second;
  if (peer.replying) {
    if ((events & POLLOUT) == 0) return;
    const ssize_t count = send(descriptor, peer.output.data() + peer.sent,
                               peer.output.size() - peer.sent, 0);
    if (count < 0 && (errno == EAGAIN || errno == EINTR)) return;
    if (count <= 0) { Close(descriptor); return; }
    peer.sent += static_cast<size_t>(count);
    if (peer.sent == peer.output.size()) {
      peer.replying = false;
      peer.sent = 0;
    }
    return;
  }
  if ((events & POLLIN) == 0) return;
  const size_t needed = peer.classified ? sizeof(OutputRequest) : kOutputMagic.size();
  const ssize_t count = recv(descriptor, peer.input.data() + peer.received,
                             needed - peer.received, 0);
  if (count < 0 && (errno == EAGAIN || errno == EINTR)) return;
  if (count <= 0) { Close(descriptor); return; }
  peer.received += static_cast<size_t>(count);
  if (peer.received != needed) return;
  try {
    if (!peer.classified) {
      std::array<char, 8> magic{};
      std::memcpy(magic.data(), peer.input.data(), magic.size());
      if (magic == kRequestMagic) {
        compose_(descriptor, magic);
        Close(descriptor);
        return;
      }
      if (magic != kOutputMagic) { Close(descriptor); return; }
      peer.classified = true;
      return;
    }
    OutputRequest request{};
    std::memcpy(&request, peer.input.data(), sizeof(request));
    const auto response = apply_(descriptor, request);
    peer.registered = peer.registered ||
        (request.operation == OutputOperation::Register && response.status == 0);
    std::memcpy(peer.output.data(), &response, sizeof(response));
    peer.received = 0;
    peer.sent = 0;
    peer.replying = true;
  } catch (...) {
    std::fprintf(stderr, "ART SurfaceFlinger: ingress callback failed fd=%d\n", descriptor);
    Close(descriptor);
  }
}

}  // namespace darwin_art::surfaceflinger
