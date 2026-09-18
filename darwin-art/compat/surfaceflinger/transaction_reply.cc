#include "transaction_reply.h"

#include "composition_protocol.h"

#include <array>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <utility>

namespace darwin_art::surfaceflinger {
namespace {
int Duplicate(int descriptor) {
  return fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
}

bool Writable(int descriptor, std::chrono::steady_clock::time_point deadline) {
  for (;;) {
    auto remaining = deadline - std::chrono::steady_clock::now();
    if (remaining <= decltype(remaining)::zero()) return false;
    pollfd waiter{descriptor, POLLOUT, 0};
    int result = poll(&waiter, 1, static_cast<int>(
        std::chrono::duration_cast<std::chrono::milliseconds>(remaining).count() + 1));
    if (result < 0 && errno == EINTR) continue;
    return result > 0 && (waiter.revents & POLLOUT) != 0 &&
           (waiter.revents & (POLLERR | POLLHUP | POLLNVAL)) == 0;
  }
}
}  // namespace

std::shared_ptr<TransactionReply> TransactionReply::Create(int client, int completion) {
  const int owned_client = Duplicate(client);
  if (owned_client < 0) return {};
  const int flags = fcntl(owned_client, F_GETFL);
  const int no_sigpipe = 1;
  if (flags < 0 || fcntl(owned_client, F_SETFL, flags | O_NONBLOCK) != 0 ||
      setsockopt(owned_client, SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe,
                 sizeof(no_sigpipe)) != 0) {
    close(owned_client);
    return {};
  }
  const int owned_completion = Duplicate(completion);
  if (owned_completion < 0) { close(owned_client); return {}; }
  // Separate object allocation from shared_ptr control-block allocation:
  // shared_ptr deletes the object on its own allocation failure.
  TransactionReply* reply;
  try { reply = new TransactionReply(owned_client, owned_completion); }
  catch (...) { close(owned_client); close(owned_completion); return {}; }
  try { return std::shared_ptr<TransactionReply>(reply); }
  catch (...) { return {}; } // shared_ptr already deleted reply.
}

TransactionReply::~TransactionReply() {
  if (client_ >= 0) close(client_);
  if (completion_ >= 0) close(completion_);
}

bool TransactionReply::Committed() noexcept { return Send(true, 0); }
bool TransactionReply::Rejected(int status) noexcept {
  return status != 0 && Send(false, status);
}

bool TransactionReply::Send(bool committed, int status) noexcept {
  if (client_ < 0) return false;
  const int client = std::exchange(client_, -1);
  const int completion = std::exchange(completion_, -1);
  ResponseHeader response{};
  std::memcpy(response.magic, kResponseMagic.data(), kResponseMagic.size());
  response.version = kProtocolVersion;
  response.status = status;
  response.commit = committed ? CommitDisposition::Committed
                              : CommitDisposition::RejectedBeforeCommit;
  response.has_completion_fence = committed ? 1u : 0u;
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(250);
  const auto* bytes = reinterpret_cast<const unsigned char*>(&response);
  size_t remaining = sizeof(response);
  bool sent = true;
  while (remaining != 0) {
    if (!Writable(client, deadline)) { sent = false; break; }
    const ssize_t count = send(client, bytes, remaining, 0);
    if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
    if (count <= 0) { sent = false; break; }
    bytes += count;
    remaining -= static_cast<size_t>(count);
  }
  if (sent) {
    char marker = committed ? 1 : 0;
    iovec vector{&marker, sizeof(marker)};
    std::array<char, CMSG_SPACE(sizeof(int))> control{};
    msghdr message{};
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    if (committed) {
      message.msg_control = control.data();
      message.msg_controllen = control.size();
      auto* header = CMSG_FIRSTHDR(&message);
      header->cmsg_level = SOL_SOCKET;
      header->cmsg_type = SCM_RIGHTS;
      header->cmsg_len = CMSG_LEN(sizeof(int));
      std::memcpy(CMSG_DATA(header), &completion, sizeof(completion));
    }
    for (;;) {
      if (!Writable(client, deadline)) { sent = false; break; }
      const ssize_t count = sendmsg(client, &message, 0);
      if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
      sent = count == 1;
      break;
    }
  }
  close(client);
  close(completion);
  return sent;
}
}  // namespace darwin_art::surfaceflinger
