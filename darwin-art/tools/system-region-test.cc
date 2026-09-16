#include "../compat/memory/system_region.h"
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

int main(int argc, char** argv) {
  if (argc == 3 && strcmp(argv[1], "--reader") == 0) {
    const int socket = atoi(argv[2]);
    char byte = 0;
    iovec io{&byte, 1};
    alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))]{};
    msghdr message{};
    message.msg_iov = &io;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
    assert(recvmsg(socket, &message, 0) == 1);
    auto* header = CMSG_FIRSTHDR(&message);
    assert(header && header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS);
    assert(!(message.msg_flags & MSG_CTRUNC));
    int fd = *reinterpret_cast<int*>(CMSG_DATA(header));
    assert((fcntl(fd, F_GETFL) & O_ACCMODE) == O_RDONLY);
    struct stat info{};
    assert(fstat(fd, &info) == 0 && info.st_nlink == 0 && info.st_size == 4096);
    assert(pwrite(fd, "X", 1, 0) == -1 && errno == EBADF);
    assert(ftruncate(fd, 0) == -1);
    assert(mmap(nullptr, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) == MAP_FAILED);
    auto* reader = static_cast<const char*>(mmap(nullptr, 4096, PROT_READ, MAP_SHARED, fd, 0));
    assert(reader != MAP_FAILED);
    assert(mprotect(const_cast<char*>(reader), 4096, PROT_READ | PROT_WRITE) == -1);
    assert(reader[0] == 'A');
    assert(write(socket, "a", 1) == 1);
    assert(read(socket, &byte, 1) == 1);
    assert(reader[0] == 'B');
    assert(munmap(const_cast<char*>(reader), 4096) == 0);
    close(fd);
    close(socket);
    return 0;
  }
  assert(!darwin_art::memory::SystemRegion::Create(0) && errno == EINVAL);
  auto region = darwin_art::memory::SystemRegion::Create(4096);
  assert(region && region->size() == 4096);
  int writer_fd = region->DuplicateWriter(), reader_fd = region->DuplicateReader();
  assert(writer_fd >= 0 && reader_fd >= 0);
  assert(fcntl(writer_fd, F_GETFD) & FD_CLOEXEC);
  assert(fcntl(reader_fd, F_GETFD) & FD_CLOEXEC);
  auto* writer = static_cast<char*>(mmap(nullptr, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, writer_fd, 0));
  assert(writer != MAP_FAILED);
  writer[0] = 'A';
  int sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    close(sockets[0]);
    char fd_text[24];
    snprintf(fd_text, sizeof(fd_text), "%d", sockets[1]);
    execl(argv[0], argv[0], "--reader", fd_text, nullptr);
    _exit(127);
  }
  close(sockets[1]);
  // Neither creator lifetime nor the writer FD is needed by exported readers.
  region.reset();
  close(writer_fd);
  char byte = 1;
  iovec io{&byte, 1};
  alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))]{};
  msghdr message{};
  message.msg_iov = &io;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = sizeof(control);
  auto* header = CMSG_FIRSTHDR(&message);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  *reinterpret_cast<int*>(CMSG_DATA(header)) = reader_fd;
  assert(sendmsg(sockets[0], &message, 0) == 1);
  close(reader_fd);
  assert(read(sockets[0], &byte, 1) == 1);
  writer[0] = 'B';
  assert(write(sockets[0], "b", 1) == 1);
  close(sockets[0]);
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  assert(munmap(writer, 4096) == 0);
  puts("system-region exec/SCM_RIGHTS: writes denied, system updates visible, lifetime PASS");
}
