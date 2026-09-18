#include "fd-inheritance-fixture.h"
#include "tools/bionic-socket-broker-adapter/src/ancillary_intake.h"

#include <array>
#include <cassert>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <sys/socket.h>
#include <unistd.h>

using darwin_art::bionic::ancillary::AncillaryRecord;
using darwin_art::bionic::ancillary::DecodeAncillary;
using darwin_art::bionic::ancillary::Recvmsg;
using darwin_art::bionic::ancillary::RecvRequest;
using darwin_art::bionic::ancillary::RecvResult;

namespace {

void SendRights(int socket, const int *descriptors, std::size_t count,
                char payload = 'x') {
  std::array<unsigned char, CMSG_SPACE(254 * sizeof(int))> control{};
  iovec vector{&payload, sizeof(payload)};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control.data();
  message.msg_controllen = CMSG_SPACE(count * sizeof(int));
  cmsghdr *header = CMSG_FIRSTHDR(&message);
  assert(header != nullptr);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(count * sizeof(int));
  std::memcpy(CMSG_DATA(header), descriptors, count * sizeof(int));
  assert(sendmsg(socket, &message, 0) == 1);
}

RecvRequest Request(int socket, iovec *vector, int flags = 0) {
  RecvRequest request;
  request.socket = socket;
  request.vectors = vector;
  request.vector_count = 1;
  request.flags = flags;
  return request;
}

int OpenCount() {
  int count = 0;
  for (int fd = 0; fd < 8192; ++fd) {
    if (fcntl(fd, F_GETFD) >= 0 || errno != EBADF)
      ++count;
  }
  return count;
}

void AssertCloseOnExec(int descriptor) {
  const int flags = fcntl(descriptor, F_GETFD);
  assert(flags >= 0);
  assert((flags & FD_CLOEXEC) != 0);
}

void TestTakeAndDiscard() {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  int pipe_fds[2];
  assert(pipe(pipe_fds) == 0);

  int sent[3] = {pipe_fds[0], pipe_fds[0], pipe_fds[0]};
  SendRights(pair[0], sent, 3);
  char byte = 0;
  iovec vector{&byte, sizeof(byte)};
  RecvResult result;
  assert(Recvmsg(Request(pair[1], &vector), &result) == 1);
  assert(byte == 'x');
  assert(result.rights().size() == 3);
  int first = -1;
  assert(result.rights().Take(0, &first));
  assert(first >= 0);
  AssertCloseOnExec(first);
  assert(!result.rights().Take(0, &first));
  int second = -1;
  int third = -1;
  assert(result.rights().Take(1, &second));
  assert(result.rights().Take(2, &third));
  AssertCloseOnExec(second);
  AssertCloseOnExec(third);
  close(first);
  close(second);
  close(third);

  const int before = OpenCount();
  int sent_again[2] = {pipe_fds[1], pipe_fds[1]};
  SendRights(pair[0], sent_again, 2);
  {
    RecvResult discarded;
    assert(Recvmsg(Request(pair[1], &vector), &discarded) == 1);
    assert(discarded.rights().size() == 2);
  }
  assert(OpenCount() == before);
  close(pipe_fds[0]);
  close(pipe_fds[1]);
  close(pair[0]);
  close(pair[1]);
}

void TestPeekAndQueuePreservation() {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  int pipe_fds[2];
  assert(pipe(pipe_fds) == 0);
  SendRights(pair[0], &pipe_fds[0], 1, 'p');
  char byte = 0;
  iovec vector{&byte, sizeof(byte)};
  RecvResult peek;
  errno = 0;
  assert(Recvmsg(Request(pair[1], &vector, MSG_PEEK), &peek) == -1);
  assert(errno == EOPNOTSUPP);
  assert(peek.rights().size() == 0);

  RecvResult consumed;
  assert(Recvmsg(Request(pair[1], &vector), &consumed) == 1);
  assert(byte == 'p' && consumed.rights().size() == 1);
  int received = -1;
  assert(consumed.rights().Take(0, &received));
  close(received);
  close(pipe_fds[0]);
  close(pipe_fds[1]);
  close(pair[0]);
  close(pair[1]);
}

void TestErrnoAndFlags() {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  int status = fcntl(pair[1], F_GETFL, 0);
  assert(status >= 0 && fcntl(pair[1], F_SETFL, status | O_NONBLOCK) == 0);
  char byte = 0;
  iovec vector{&byte, sizeof(byte)};
  RecvResult empty;
  errno = 0;
  assert(Recvmsg(Request(pair[1], &vector, MSG_DONTWAIT), &empty) == -1);
  assert(errno == EAGAIN || errno == EWOULDBLOCK);

  assert(write(pair[0], "f", 1) == 1);
  RecvResult invalid;
  errno = 0;
  assert(Recvmsg(Request(pair[1], &vector, 0x40000000), &invalid) == -1);
  assert(errno == EINVAL);
  RecvResult valid;
  assert(Recvmsg(Request(pair[1], &vector, MSG_DONTWAIT), &valid) == 1);
  assert(byte == 'f');
  close(pair[0]);
  close(pair[1]);
}

void TestSupportedNativeBatch() {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  std::array<int, 254> sent{};
  for (int &descriptor : sent) {
    descriptor = open("/dev/null", O_RDONLY);
    assert(descriptor >= 0);
  }
  SendRights(pair[0], sent.data(), sent.size(), 'b');
  char byte = 0;
  iovec vector{&byte, sizeof(byte)};
  RecvResult result;
  assert(Recvmsg(Request(pair[1], &vector), &result) == 1);
  assert(byte == 'b' && result.rights().size() == sent.size());
  for (std::size_t index = 0; index < sent.size(); ++index) {
    int descriptor = -1;
    assert(result.rights().Take(index, &descriptor));
    assert(descriptor >= 0);
    AssertCloseOnExec(descriptor);
    close(descriptor);
    close(sent[index]);
  }
  close(pair[0]);
  close(pair[1]);
}

void TestAdjacentMaximumNativeBatches() {
  const int baseline = OpenCount();
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  std::array<int, 254> sent{};
  for (int &descriptor : sent) {
    descriptor = open("/dev/null", O_RDONLY);
    assert(descriptor >= 0);
  }
  SendRights(pair[0], sent.data(), sent.size(), '1');
  SendRights(pair[0], sent.data(), sent.size(), '2');

  std::array<char, 2> payload{};
  std::size_t bytes = 0;
  std::size_t rights = 0;
  while (bytes < payload.size()) {
    iovec vector{payload.data() + bytes, payload.size() - bytes};
    RecvResult result;
    const ssize_t received =
        Recvmsg(Request(pair[1], &vector, MSG_WAITALL), &result);
    assert(received > 0);
    bytes += static_cast<std::size_t>(received);
    rights += result.rights().size();
    for (std::size_t index = 0; index < result.rights().size(); ++index) {
      int descriptor = -1;
      assert(result.rights().Take(index, &descriptor));
      assert(descriptor >= 0);
      AssertCloseOnExec(descriptor);
      close(descriptor);
    }
  }
  assert(bytes == 2);
  assert(payload[0] == '1' && payload[1] == '2');
  assert(rights == 508);
  for (const int descriptor : sent)
    close(descriptor);
  close(pair[0]);
  close(pair[1]);
  assert(OpenCount() == baseline);
}

void TestUnalignedDecoder() {
  std::array<unsigned char, CMSG_SPACE(sizeof(int)) + 1> storage{};
  unsigned char *bytes = storage.data() + 1;
  const socklen_t length = CMSG_LEN(sizeof(int));
  const int level = SOL_SOCKET;
  const int type = SCM_RIGHTS;
  std::memcpy(bytes, &length, sizeof(length));
  std::memcpy(bytes + sizeof(length), &level, sizeof(level));
  std::memcpy(bytes + sizeof(length) + sizeof(level), &type, sizeof(type));
  int fake = 123;
  std::memcpy(bytes + sizeof(cmsghdr), &fake, sizeof(fake));
  AncillaryRecord record;
  std::size_t count = 0;
  assert(DecodeAncillary(bytes, CMSG_SPACE(sizeof(int)), &record, 1, &count));
  assert(count == 1 && record.level == SOL_SOCKET &&
         record.type == SCM_RIGHTS && record.data_length == sizeof(int));

  const socklen_t malformed = sizeof(cmsghdr) - 1;
  std::memcpy(bytes, &malformed, sizeof(malformed));
  errno = 0;
  assert(!DecodeAncillary(bytes, sizeof(cmsghdr), &record, 1, &count));
  assert(errno == EPROTO);
}

void TestQueuedGroupsWaitAll() {
  int pair[2];
  int pipe_fds[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  assert(pipe(pipe_fds) == 0);
  const int baseline = OpenCount();
  constexpr std::size_t groups = 15;
  std::array<int, 18> descriptors{};
  descriptors.fill(pipe_fds[0]);
  for (std::size_t index = 0; index < groups; ++index) {
    SendRights(pair[0], descriptors.data(), descriptors.size());
  }
  // Terminate the write direction so an incorrect WAITALL implementation
  // cannot hang waiting for more bytes during this bounded native test.
  assert(shutdown(pair[0], SHUT_WR) == 0);
  std::size_t bytes = 0;
  std::size_t rights = 0;
  while (bytes < groups) {
    std::array<char, groups> payload{};
    iovec vector{payload.data(), groups - bytes};
    RecvResult result;
    const ssize_t received =
        Recvmsg(Request(pair[1], &vector, MSG_WAITALL), &result);
    assert(received > 0);
    bytes += received;
    rights += result.rights().size();
  }
  assert(bytes == groups && rights == groups * descriptors.size());
  assert(OpenCount() == baseline);
  close(pair[0]);
  close(pair[1]);
  close(pipe_fds[0]);
  close(pipe_fds[1]);
}

} // namespace

int main() {
  assert(darwin_art::test::InstallNoSpawnFdFixture() == 0);
  TestTakeAndDiscard();
  TestPeekAndQueuePreservation();
  TestErrnoAndFlags();
  TestSupportedNativeBatch();
  TestAdjacentMaximumNativeBatches();
  TestUnalignedDecoder();
  TestQueuedGroupsWaitAll();
  std::cout
      << "SCM ancillary native intake: ownership/peek/errno/decoder PASS\n";
}
