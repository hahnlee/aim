#include "compat/surfaceflinger/socket_transport.h"
#include <array>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

using namespace darwin_art::surfaceflinger;

static int OpenDescriptors() {
  int count = 0;
  for (int fd = 0; fd < 4096; ++fd) if (fcntl(fd, F_GETFD) >= 0) ++count;
  return count;
}

static void SendRights(int socket, char marker, const int* fds, size_t count) {
  std::array<char, CMSG_SPACE(6 * sizeof(int))> control{};
  iovec vector{&marker, 1};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control.data();
  message.msg_controllen = CMSG_SPACE(count * sizeof(int));
  auto* header = CMSG_FIRSTHDR(&message);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(count * sizeof(int));
  std::memcpy(CMSG_DATA(header), fds, count * sizeof(int));
  assert(sendmsg(socket, &message, 0) == 1);
}

int main(int argc, char** argv) {
  assert(argc == 2);
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  int pipe_fds[2];
  assert(pipe(pipe_fds) == 0);
  assert(SendDescriptor(pair[0], pipe_fds[0]));
  int received = -1;
  assert(ReceiveDescriptor(pair[1], true, &received));
  assert((fcntl(received, F_GETFD) & FD_CLOEXEC) != 0);
  close(received);
  assert(SendDescriptor(pair[0], -1));
  assert(ReceiveDescriptor(pair[1], false, &received) && received == -1);
  const int before = OpenDescriptors();
  std::array<int, 6> rights{};
  rights.fill(pipe_fds[0]);
  for (size_t count : {size_t{1}, size_t{2}, size_t{6}}) {
    SendRights(pair[0], 2, rights.data(), count);
    assert(!ReceiveDescriptor(pair[1], true, &received) && received == -1);
    assert(OpenDescriptors() == before);
  }
  SendRights(pair[0], 1, rights.data(), 1);
  assert(!ReceiveDescriptor(pair[1], false, &received));
  assert(OpenDescriptors() == before);
  SendRights(pair[0], 1, rights.data(), 6);
  assert(!ReceiveDescriptor(pair[1], true, &received));
  assert(OpenDescriptors() == before);
  const std::array<char, 4> bytes{'a', 'b', 'c', 'd'};
  std::array<char, 4> copied{};
  assert(WriteAll(pair[0], bytes.data(), 1));
  assert(WriteAll(pair[0], bytes.data() + 1, 3));
  assert(ReadAll(pair[1], copied.data(), copied.size()) && copied == bytes);
  close(pair[0]); close(pair[1]); close(pipe_fds[0]); close(pipe_fds[1]);

  const int listener = socket(AF_UNIX, SOCK_STREAM, 0);
  assert(listener >= 0);
  sockaddr_un address{};
  address.sun_family = AF_UNIX;
  assert(std::strlen(argv[1]) < sizeof(address.sun_path));
  std::strcpy(address.sun_path, argv[1]);
  assert(bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
  assert(listen(listener, 1) == 0);
  const int client = Connect(argv[1]);
  assert(client >= 0 && (fcntl(client, F_GETFD) & FD_CLOEXEC) != 0);
  const int peer = accept(listener, nullptr, nullptr);
  assert(peer >= 0);
  close(peer);
  char byte = 1;
  assert(!WriteAll(client, &byte, 1));  // No SIGPIPE termination.
  close(client); close(listener); unlink(argv[1]);
  std::puts("SurfaceFlinger socket transport: SCM ownership/invalid batches/CLOEXEC/SIGPIPE/bytes PASS");
}
