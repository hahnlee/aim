// Private fixture for the standalone SCM trace interposer. Not production.
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <sys/socket.h>
#include <thread>
#include <unistd.h>

namespace {

bool ValidRightsControl(const unsigned char *bytes, size_t length) {
  if (bytes == nullptr || length < sizeof(cmsghdr)) return false;
  cmsghdr header{};
  std::memcpy(&header, bytes, sizeof(header));
  return header.cmsg_len >= sizeof(cmsghdr) &&
         header.cmsg_len <= length &&
         header.cmsg_level == SOL_SOCKET && header.cmsg_type == SCM_RIGHTS &&
         (header.cmsg_len - sizeof(cmsghdr)) % sizeof(int) == 0;
}

}  // namespace

void RunFixture() {
  int pair[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  int transferred = dup(pair[0]);
  assert(transferred >= 0);

  char byte = 'S';
  iovec send_iov{&byte, 1};
  alignas(cmsghdr) unsigned char send_control[CMSG_SPACE(sizeof(int))]{};
  msghdr send_message{};
  send_message.msg_iov = &send_iov;
  send_message.msg_iovlen = 1;
  send_message.msg_control = send_control;
  send_message.msg_controllen = sizeof(send_control);
  cmsghdr *send_header = CMSG_FIRSTHDR(&send_message);
  assert(send_header != nullptr);
  send_header->cmsg_level = SOL_SOCKET;
  send_header->cmsg_type = SCM_RIGHTS;
  send_header->cmsg_len = CMSG_LEN(sizeof(int));
  std::memcpy(CMSG_DATA(send_header), &transferred, sizeof(transferred));
  errno = EDOM;
  assert(sendmsg(pair[0], &send_message, 0) == 1);
  assert(errno == EDOM);

  char received_byte = 0;
  iovec recv_iov{&received_byte, 1};
  alignas(cmsghdr) unsigned char recv_control[CMSG_SPACE(sizeof(int))]{};
  msghdr recv_message{};
  recv_message.msg_iov = &recv_iov;
  recv_message.msg_iovlen = 1;
  recv_message.msg_control = recv_control;
  recv_message.msg_controllen = sizeof(recv_control);
  errno = ERANGE;
  assert(recvmsg(pair[1], &recv_message, 0) == 1);
  assert(errno == ERANGE);
  assert(received_byte == 'S');
  cmsghdr *recv_header = CMSG_FIRSTHDR(&recv_message);
  assert(recv_header != nullptr && recv_header->cmsg_level == SOL_SOCKET &&
         recv_header->cmsg_type == SCM_RIGHTS);
  int received_fd = -1;
  std::memcpy(&received_fd, CMSG_DATA(recv_header), sizeof(received_fd));
  assert(received_fd >= 0);
  assert(send(pair[1], "x", 1, 0) == 1);
  char response = 0;
  assert(recv(received_fd, &response, 1, 0) == 1 && response == 'x');
  close(received_fd);

  // Exactly-once forwarding: no second payload/right remains queued.
  recv_message.msg_controllen = sizeof(recv_control);
  assert(recvmsg(pair[1], &recv_message, MSG_DONTWAIT) == -1);
  assert(errno == EAGAIN);
  assert(sendmsg(-1, &send_message, 0) == -1 && errno == EBADF);
  assert(recvmsg(-1, &recv_message, MSG_DONTWAIT) == -1 && errno == EBADF);
  const auto *invalid_send = reinterpret_cast<const msghdr *>(uintptr_t{1});
  auto *invalid_receive = reinterpret_cast<msghdr *>(uintptr_t{1});
  assert(sendmsg(pair[0], invalid_send, MSG_DONTWAIT) == -1 && errno == EFAULT);
  assert(recvmsg(pair[1], invalid_receive, MSG_DONTWAIT) == -1 && errno == EFAULT);

  // Bounds validation is local and non-mutating; no malformed control data is
  // sent to the kernel or used to fabricate an acknowledgement.
  unsigned char malformed[sizeof(cmsghdr)]{};
  cmsghdr malformed_header{};
  malformed_header.cmsg_len = sizeof(cmsghdr) - 1;
  std::memcpy(malformed, &malformed_header, sizeof(malformed_header));
  unsigned char before[sizeof(malformed)]{};
  std::memcpy(before, malformed, sizeof(before));
  assert(!ValidRightsControl(malformed, sizeof(malformed)));
  assert(std::memcmp(before, malformed, sizeof(before)) == 0);

  close(transferred);
  close(pair[0]);
  close(pair[1]);
}

void RunTruncatedFixture() {
  int pair[2] = {-1, -1};
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  char byte = 'T';
  iovec vector{&byte, 1};
  alignas(cmsghdr) unsigned char control[CMSG_SPACE(sizeof(int))]{};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = sizeof(control);
  cmsghdr* header = CMSG_FIRSTHDR(&message);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  std::memcpy(CMSG_DATA(header), &pair[0], sizeof(int));
  assert(sendmsg(pair[0], &message, 0) == 1);
  // Supply nonzero control space smaller than a cmsghdr. Darwin silently
  // discards rights with zero space; nonzero undersized space reports CTRUNC.
  msghdr receive{};
  receive.msg_iov = &vector;
  receive.msg_iovlen = 1;
  unsigned char tiny_control[8]{};
  receive.msg_control = tiny_control;
  receive.msg_controllen = sizeof(tiny_control);
  assert(recvmsg(pair[1], &receive, 0) == 1);
  assert(byte == 'T' && (receive.msg_flags & MSG_CTRUNC) != 0);
  assert(receive.msg_controllen <= sizeof(tiny_control));
  close(pair[0]);
  close(pair[1]);
}

void RunPeekFixture() {
  int pair[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair) == 0);
  char byte = 0;
  iovec vector{&byte, 1};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  assert(recvmsg(pair[1], &message, MSG_PEEK | MSG_DONTWAIT) == -1);
  assert(errno == EAGAIN);
  assert(recv(pair[1], &byte, 1, MSG_PEEK | MSG_DONTWAIT) == -1);
  assert(errno == EAGAIN);
  assert(recvfrom(pair[1], &byte, 1, MSG_PEEK | MSG_DONTWAIT, nullptr,
                  nullptr) == -1);
  assert(errno == EAGAIN);
  assert(send(pair[0], "P", 1, 0) == 1);
  errno = EDOM;
  assert(recvmsg(pair[1], &message, MSG_PEEK) == 1);
  assert(byte == 'P' && errno == EDOM);
  errno = ERANGE;
  assert(recv(pair[1], &byte, 1, MSG_PEEK) == 1 && errno == ERANGE);
  errno = EDOM;
  assert(recvfrom(pair[1], &byte, 1, MSG_PEEK, nullptr, nullptr) == 1);
  assert(byte == 'P' && errno == EDOM);
  byte = 0;
  assert(recvmsg(pair[1], &message, 0) == 1 && byte == 'P');
  close(pair[0]);
  close(pair[1]);
}

int main(int argc, char** argv) {
  if (argc > 2) return 64;
  if (argc == 2 && std::strcmp(argv[1], "--peek") == 0) {
    RunPeekFixture();
    return 0;
  }
  if (argc == 2 && std::strcmp(argv[1], "--truncated") == 0) {
    RunTruncatedFixture();
    return 0;
  }
  if (argc == 2 && std::strcmp(argv[1], "--concurrent") == 0) {
    auto work = [] { for (int trial = 0; trial < 150; ++trial) RunFixture(); };
    std::thread first(work), second(work);
    first.join();
    second.join();
    return 0;
  }
  if (argc == 2 && std::strcmp(argv[1], "--stress") != 0) return 64;
  const int trials = argc == 2 ? 300 : 1;
  for (int trial = 0; trial < trials; ++trial) RunFixture();
  return 0;
}
