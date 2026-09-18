#include "../../compat/surfaceflinger/transaction_reply.h"
#include "../../compat/surfaceflinger/composition_protocol.h"
#include "../../compat/surfaceflinger/service_ingress.h"

#include <cassert>
#include <array>
#include <chrono>
#include <cstdio>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <cstring>

using namespace darwin_art::surfaceflinger;

struct Connection {
  int socket[2];
  int fence[2];
  Connection() {
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, socket) == 0);
    assert(pipe(fence) == 0);
  }
  ~Connection() {
    for (int fd : socket) if (fd >= 0) close(fd);
    for (int fd : fence) if (fd >= 0) close(fd);
  }
  void DetachIngress() { close(socket[0]); socket[0] = -1; }
};

int Descriptor(int fd, bool expected) {
  char marker = -1;
  iovec vector{&marker, 1};
  std::array<char, CMSG_SPACE(sizeof(int))> control{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control.data();
  message.msg_controllen = control.size();
  assert(recvmsg(fd, &message, 0) == 1);
  assert(marker == (expected ? 1 : 0));
  auto* header = CMSG_FIRSTHDR(&message);
  if (!expected) { assert(header == nullptr); return -1; }
  assert(header && header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS);
  int received = -1;
  std::memcpy(&received, CMSG_DATA(header), sizeof(received));
  assert(received >= 0);
  return received;
}

namespace {
std::shared_ptr<TransactionReply> ingress_reply;
int ingress_completion = -1;
int output_drops = 0;
int registered_output = -1;
OutputResponse RegisterOutput(int descriptor, const OutputRequest&) {
  registered_output = descriptor;
  return {};
}
void DropOutput(int descriptor) noexcept {
  if (descriptor == registered_output) ++output_drops;
}
void RetainCompositionReply(int descriptor, const std::array<char, 8>& magic) {
  assert(magic == kRequestMagic);
  ingress_reply = TransactionReply::Create(descriptor, ingress_completion);
  assert(ingress_reply);
}
void TestOutputEofDuringDeferredReply() {
  Connection composition;
  Connection output;
  ingress_completion = composition.fence[0];
  output_drops = 0;
  {
    ServiceIngress ingress(RegisterOutput, DropOutput, RetainCompositionReply);
    assert(ingress.Add(composition.socket[0]));
    composition.socket[0] = -1; // ingress owns original, reply owns duplicate.
    assert(ingress.Add(output.socket[0]));
    const int output_fd = output.socket[0];
    output.socket[0] = -1;
    assert(write(composition.socket[1], kRequestMagic.data(), kRequestMagic.size()) == 8);
    std::vector<pollfd> pending;
    ingress.AppendPoll(&pending);
    for (auto waiter : pending) if (waiter.fd != output_fd) ingress.Ready(waiter.fd, POLLIN);
    pollfd unresolved{composition.socket[1], POLLIN, 0};
    assert(poll(&unresolved, 1, 0) == 0); // no ACK and no EOF after ingress detach.
    OutputRequest request{};
    request.magic = kOutputMagic;
    request.version = kOutputProtocolVersion;
    request.operation = OutputOperation::Register;
    assert(write(output.socket[1], &request, sizeof(request)) == sizeof(request));
    ingress.Ready(output_fd, POLLIN);
    ingress.Ready(output_fd, POLLIN);
    ingress.Ready(output_fd, POLLOUT);
    OutputResponse registration{};
    assert(read(output.socket[1], &registration, sizeof(registration)) == sizeof(registration));
    close(output.socket[1]); output.socket[1] = -1;
    ingress.Ready(output_fd, POLLHUP);
    assert(output_drops == 1); // output retirement is not waiting for a commit reply.
    assert(ingress_reply->Committed());
    ResponseHeader response{};
    assert(read(composition.socket[1], &response, sizeof(response)) == sizeof(response));
    assert(response.commit == CommitDisposition::Committed);
    close(Descriptor(composition.socket[1], true));
    ingress_reply.reset();
  }
}
}  // namespace

int main() {
  {
    Connection c;
    auto reply = TransactionReply::Create(c.socket[0], c.fence[0]);
    assert(reply);
    c.DetachIngress();
    pollfd waiting{c.socket[1], POLLIN, 0};
    assert(poll(&waiting, 1, 0) == 0); // ownership/queue admission is not commit.
    assert(reply->Committed());
    assert(!reply->Committed());
    assert(!reply->Rejected(EAGAIN));
    ResponseHeader response{};
    assert(read(c.socket[1], &response, sizeof(response)) == sizeof(response));
    assert(response.version == kProtocolVersion && response.status == 0);
    assert(response.commit == CommitDisposition::Committed);
    assert(response.has_completion_fence == 1);
    int fence = Descriptor(c.socket[1], true);
    pollfd pending{fence, POLLIN, 0};
    assert(poll(&pending, 1, 0) == 0); // receipt did not signal completion.
    const uint64_t signal = 42;
    assert(write(c.fence[1], &signal, sizeof(signal)) == sizeof(signal));
    uint64_t observed = 0;
    assert(read(fence, &observed, sizeof(observed)) == sizeof(observed));
    assert(observed == signal);
    close(fence);
  }
  {
    Connection c;
    auto reply = TransactionReply::Create(c.socket[0], c.fence[0]);
    assert(reply);
    c.DetachIngress();
    assert(reply->Rejected(EAGAIN));
    ResponseHeader response{};
    assert(read(c.socket[1], &response, sizeof(response)) == sizeof(response));
    assert(response.status == EAGAIN);
    assert(response.commit == CommitDisposition::RejectedBeforeCommit);
    assert(response.has_completion_fence == 0);
    assert(Descriptor(c.socket[1], false) == -1);
  }
  {
    Connection c;
    auto reply = TransactionReply::Create(c.socket[0], c.fence[0]);
    assert(reply);
    c.DetachIngress();
    reply.reset(); // abort/unknown: close-only, no socket I/O or safe rejection.
    char byte;
    assert(read(c.socket[1], &byte, 1) == 0);
  }
  {
    Connection c;
    auto reply = TransactionReply::Create(c.socket[0], c.fence[0]);
    assert(reply);
    close(c.socket[1]); c.socket[1] = -1;
    assert(!reply->Committed()); // no SIGPIPE or second response after loss.
    assert(!reply->Committed());
  }
  {
    Connection c;
    auto reply = TransactionReply::Create(c.socket[0], c.fence[0]);
    assert(reply);
    std::array<char, 8192> fill{};
    while (send(c.socket[0], fill.data(), fill.size(), 0) > 0) {}
    assert(errno == EAGAIN || errno == EWOULDBLOCK);
    auto start = std::chrono::steady_clock::now();
    assert(!reply->Committed());
    assert(std::chrono::steady_clock::now() - start < std::chrono::seconds(1));
  }
  TestOutputEofDuringDeferredReply();
  std::puts("transaction reply: actual transport lifetime/one-shot/unknown/bounded-write/fence independence/output EOF PASS");
}
