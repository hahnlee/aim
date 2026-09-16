// Tests descriptor storage/transport, not kernel-enforced ashmem sealing.
#include "../compat/memory/shared_memory.h"
#include "../compat/darwin_android_platform.h"
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

// Isolate the backing implementation from Bionic's ioctl/errno adapters.
extern "C" int darwin_art_bionic_errno_set_from_darwin(int error) {
  errno = error;
  return 0;
}
extern "C" int darwin_art_bionic_ioctl_bind_shared_memory(
    int (*callback)(int, uint32_t, void*, int*, int*)) {
  assert(callback != nullptr);
  return 0;
}

int main() {
  assert(ASharedMemory_create("invalid", 0) == -1);
  assert(errno == EINVAL);
  int fd = ASharedMemory_create("contract", 4096);
  assert(fd >= 0);
  auto* writer = static_cast<uint32_t*>(
      mmap(nullptr, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0));
  assert(writer != MAP_FAILED);
  *writer = 0x12345678;
  int duplicate = darwin_art_android_shared_memory_dup(fd);
  assert(duplicate >= 0);
  assert(ASharedMemory_setProt(duplicate, PROT_READ) == 0);
  assert(ASharedMemory_setProt(fd, PROT_READ | PROT_WRITE) == -1);
  assert(errno == EINVAL);
  size_t size = 0;
  int protection = 0;
  assert(darwin_art_android_shared_memory_get_info(fd, &size, &protection) == 1);
  assert(size == 4096 && protection == PROT_READ);
  int sockets[2];
  assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    close(sockets[0]);
    close(fd);
    close(duplicate);
    assert(munmap(writer, 4096) == 0);
    char byte = 0;
    iovec io{&byte, 1};
    alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))]{};
    msghdr message{};
    message.msg_iov = &io;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
    assert(recvmsg(sockets[1], &message, 0) == 1);
    assert((message.msg_flags & MSG_CTRUNC) == 0);
    cmsghdr* header = CMSG_FIRSTHDR(&message);
    assert(header && header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS);
    int received = *reinterpret_cast<int*>(CMSG_DATA(header));
    assert(darwin_art_android_shared_memory_get_info(received, &size, &protection) == 1);
    assert(size == 4096 && protection == PROT_READ);
    auto* reader = static_cast<const uint32_t*>(
        mmap(nullptr, size, PROT_READ, MAP_SHARED, received, 0));
    assert(reader != MAP_FAILED && *reader == 0x12345678);
    assert(munmap(const_cast<uint32_t*>(reader), size) == 0);
    assert(darwin_art_android_shared_memory_close(received) == 1);
    close(sockets[1]);
    _exit(0);
  }
  close(sockets[1]);
  char byte = 1;
  iovec io{&byte, 1};
  alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))]{};
  msghdr message{};
  message.msg_iov = &io;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = sizeof(control);
  cmsghdr* header = CMSG_FIRSTHDR(&message);
  header->cmsg_level = SOL_SOCKET;
  header->cmsg_type = SCM_RIGHTS;
  header->cmsg_len = CMSG_LEN(sizeof(int));
  *reinterpret_cast<int*>(CMSG_DATA(header)) = duplicate;
  assert(sendmsg(sockets[0], &message, 0) == 1);
  assert(darwin_art_android_shared_memory_close(duplicate) == 1);
  assert(darwin_art_android_shared_memory_close(fd) == 1);
  assert(munmap(writer, 4096) == 0);
  close(sockets[0]);
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  puts("shared-memory descriptor/SCM_RIGHTS contract PASS (not sealing)");
}
