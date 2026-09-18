// Test-only Darwin provider semantics; never linked into the APK runtime.
#include <libproc.h>
#include <sys/event.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <poll.h>
#include <fcntl.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <cstring>
#include <iostream>
#include <stdexcept>

namespace {
void Check(bool ok, const char* operation) {
  if (!ok) throw std::runtime_error(std::string(operation) + ": " +
                                    std::strerror(errno));
}
struct Pair {
  std::array<int, 2> fd{-1, -1};
  explicit Pair(int type = SOCK_STREAM) {
    Check(socketpair(AF_UNIX, type, 0, fd.data()) == 0, "socketpair");
    for (int value : fd) {
      int flags = fcntl(value, F_GETFL);
      if (flags < 0 || fcntl(value, F_SETFL, flags | O_NONBLOCK) != 0) {
        for (int owned : fd) close(owned);
        Check(false, "set nonblocking");
      }
    }
  }
  ~Pair() { for (int value : fd) if (value >= 0) close(value); }
  void CloseFirst() { Check(close(fd[0]) == 0, "close original"); fd[0] = -1; }
};
struct Owned {
  int fd;
  ~Owned() { if (fd >= 0) close(fd); }
};
void State(const char* name, int fd) {
  socket_fdinfo info{};
  Check(proc_pidfdinfo(getpid(), fd, PROC_PIDFDSOCKETINFO, &info, sizeof(info)) ==
        sizeof(info), "socket info");
  std::cout << name << " fd=" << fd << " state=" << info.psi.soi_state
            << " so=" << info.psi.soi_so << " pcb=" << info.psi.soi_pcb
            << " peer_so=" << info.psi.soi_proto.pri_un.unsi_conn_so
            << " peer_pcb=" << info.psi.soi_proto.pri_un.unsi_conn_pcb << "\n";
}
void Exchange(const char* name, int alias, int peer) {
  int enabled = 1;
  Check(setsockopt(peer, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled)) == 0,
        "SO_NOSIGPIPE");
  State(name, alias);
  Check(send(peer, "x", 1, MSG_DONTWAIT) == 1, "send peer");
  pollfd ready{alias, POLLIN, 0};
  Check(poll(&ready, 1, 1000) == 1, "poll alias");
  char byte = 0;
  const ssize_t result = recv(alias, &byte, 1, MSG_DONTWAIT);
  if (result != 1 || byte != 'x') {
    const int saved = errno;
    State("failed-alias", alias);
    State("failed-peer", peer);
    std::cerr << name << " recv=" << result << " byte=" << static_cast<int>(byte)
              << " revents=" << ready.revents << " errno="
              << (result < 0 ? saved : 0) << "\n";
    throw std::runtime_error(std::string(name) + " positive-byte receive failed");
  }
  std::cout << name << " PASS revents=" << ready.revents << " recv=" << result << "\n";
}
void Duplicate(bool watch) {
  Pair sockets;
  Owned alias{dup(sockets.fd[0])};
  Check(alias.fd >= 0, "dup");
  Owned queue{watch ? kqueue() : -1};
  if (watch) {
    Check(queue.fd >= 0, "kqueue");
    struct kevent event{};
    EV_SET(&event, sockets.fd[0], EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, nullptr);
    Check(kevent(queue.fd, &event, 1, nullptr, 0, nullptr) == 0, "register original");
  }
  sockets.CloseFirst();
  Exchange(watch ? "watched-dup" : "dup", alias.fd, sockets.fd[1]);
}
void Transfer(bool receive_first, bool retain_alias, bool datagram = false) {
  Pair sockets;
  Pair control(datagram ? SOCK_DGRAM : SOCK_STREAM);
  Owned retained{retain_alias ? dup(sockets.fd[0]) : -1};
  Check(!retain_alias || retained.fd >= 0, "retain alias");
  Exchange("SCM-before-transfer", sockets.fd[0], sockets.fd[1]);
  State("SCM-peer-before-transfer", sockets.fd[1]);
  alignas(cmsghdr) std::array<unsigned char, CMSG_SPACE(sizeof(int))> ancillary{};
  char byte = 'f';
  iovec vector{&byte, 1};
  msghdr message{};
  message.msg_iov = &vector;
  message.msg_iovlen = 1;
  message.msg_control = ancillary.data();
  message.msg_controllen = ancillary.size();
  cmsghdr* header = CMSG_FIRSTHDR(&message);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  std::memcpy(CMSG_DATA(header), &sockets.fd[0], sizeof(int));
  Check(sendmsg(control.fd[0], &message, MSG_DONTWAIT) == 1, "send rights");
  if (!receive_first) {
    sockets.CloseFirst();
    // Finite delayed receive, not a claim that the kernel collector ran.
    Check(poll(nullptr, 0, 100) == 0, "delay rights receive");
  }
  ancillary.fill(0);
  message.msg_controllen = ancillary.size();
  Check(recvmsg(control.fd[1], &message, MSG_DONTWAIT) == 1, "recv rights");
  header = CMSG_FIRSTHDR(&message);
  Check(header && header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS &&
        header->cmsg_len == CMSG_LEN(sizeof(int)) && !(message.msg_flags & MSG_CTRUNC),
        "rights header");
  Owned imported{-1};
  std::memcpy(&imported.fd, CMSG_DATA(header), sizeof(int));
  Check(imported.fd >= 0, "imported fd");
  State("SCM-peer-after-import", sockets.fd[1]);
  if (receive_first) sockets.CloseFirst();
  if (retain_alias) {
    Check(close(retained.fd) == 0, "release sender lease after import");
    retained.fd = -1;
    Check(poll(nullptr, 0, 100) == 0, "delay after sender lease release");
  }
  Exchange(datagram ? "SCM-dgram-last-close" : receive_first ? "SCM-receive-first" :
           retain_alias ? "SCM-delayed-retained" : "SCM-delayed-last-close",
           imported.fd, sockets.fd[1]);
}
}  // namespace

int main(int argc, char** argv) {
  try {
    if (argc > 2) return 64;
    const std::string choice = argc == 2 ? argv[1] : "all";
    if (choice == "all" || choice == "dup") Duplicate(false);
    if (choice == "all" || choice == "watched-dup") Duplicate(true);
    if (choice == "all" || choice == "SCM-receive-first") Transfer(true, false);
    if (choice == "all" || choice == "SCM-delayed-last-close") Transfer(false, false);
    if (choice == "all" || choice == "SCM-delayed-retained") Transfer(false, true);
    if (choice == "SCM-dgram-last-close") Transfer(false, false, true);
    if (choice != "all" && choice != "dup" && choice != "watched-dup" &&
        choice != "SCM-receive-first" && choice != "SCM-delayed-last-close" &&
        choice != "SCM-delayed-retained" && choice != "SCM-dgram-last-close") return 64;
    return 0;
  } catch (const std::exception& error) {
    std::cerr << "FAIL " << error.what() << "\n";
    return 1;
  }
}
