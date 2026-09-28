#define _GNU_SOURCE 1
#include <fcntl.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>

__attribute__((visibility("default"))) int
central_fd_dup_epoll_fixture(int input_fd, int target_fd, int socket_fd) {
  int first = dup(input_fd);
  if (first < 0)
    return 10;
  int second = dup3(input_fd, target_fd, O_CLOEXEC);
  if (second < 0)
    return 11;
  int third = fcntl(input_fd, F_DUPFD_CLOEXEC, 64);
  if (third < 0)
    return 12;
  int epoll_fd = epoll_create1(EPOLL_CLOEXEC);
  if (epoll_fd < 0)
    return 13;
  struct epoll_event registration = {.events = EPOLLIN, .data.u64 = 0x1234};
  if (epoll_ctl(epoll_fd, EPOLL_CTL_ADD, socket_fd, &registration) != 0)
    return 14;
  struct epoll_event ready[2];
  int count = epoll_wait(epoll_fd, ready, 2, 0);
  char byte = 0;
  struct sockaddr_storage address;
  socklen_t length = sizeof(address);
  int option = 0;
  int socket_result =
      bind(socket_fd, (const struct sockaddr *)&address, sizeof(address)) |
      connect(socket_fd, (const struct sockaddr *)&address, sizeof(address)) |
      listen(socket_fd, 1) | shutdown(socket_fd, SHUT_RDWR) |
      (int)send(socket_fd, &byte, 1, 0) | (int)recv(socket_fd, &byte, 1, 0) |
      (int)sendto(socket_fd, &byte, 1, 0, (const struct sockaddr *)&address,
                  length) |
      (int)recvfrom(socket_fd, &byte, 1, 0, (struct sockaddr *)&address,
                    &length) |
      getsockopt(socket_fd, SOL_SOCKET, SO_TYPE, &option, &length) |
      setsockopt(socket_fd, SOL_SOCKET, SO_TYPE, &option, sizeof(option)) |
      getpeername(socket_fd, (struct sockaddr *)&address, &length) |
      getsockname(socket_fd, (struct sockaddr *)&address, &length);
  int accepted =
      accept4(socket_fd, (struct sockaddr *)&address, &length, SOCK_CLOEXEC);
  return close(first) | close(second) | close(third) | close(epoll_fd) |
         (accepted < 0 ? 1 : close(accepted)) | (count < 0 ? 1 : 0) |
         socket_result;
}
