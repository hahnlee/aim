// Linux 6.12 drivers/dma-buf/{sync_file,dma-fence-unwrap}.c: merge metadata,
// capacity checks and flattened contexts, including a creator that exits.
#include "check.h"
#include <fcntl.h>
#include <linux/sync_file.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

static int merged = -1;

static void metadata(void) {
  struct sync_fence_info fences[3];
  struct sync_file_info info = {0};
  CHECK(ioctl(3, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 1 && info.status == 0);
  CHECK(strncmp(info.name, "aim-aim", 7) == 0);
  struct sync_merge_data merge = {.fd2 = 4};
  memset(merge.name, 'x', sizeof(merge.name));
  CHECK(ioctl(3, SYNC_IOC_MERGE, &merge) == 0);
  CHECK(merge.name[31] == 0);
  merged = merge.fence;
  CHECK(fcntl(merged, F_GETFD) == FD_CLOEXEC);
  info = (struct sync_file_info){.sync_fence_info = 1};
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 2 && info.status == 0);
  CHECK(memcmp(info.name, merge.name, 32) == 0);
  memset(fences, 0xa5, sizeof(fences));
  info.num_fences = 1;
  info.sync_fence_info = (uintptr_t)fences;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == -1 && errno == EINVAL);
  CHECK(((unsigned char *)fences)[0] == 0xa5);
  info.num_fences = 3;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 2 && info.status == 0);
  for (int i = 0; i < 2; i++) {
    CHECK(strcmp(fences[i].obj_name, "aim") == 0);
    CHECK(strcmp(fences[i].driver_name, "aim") == 0);
    CHECK(fences[i].status == 0 && fences[i].flags == 0);
    CHECK(fences[i].timestamp_ns == 0);
  }
  CHECK(((unsigned char *)&fences[2])[0] == 0xa5);
  info.flags = 1;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == -1 && errno == EINVAL);
  info.flags = 0;
  info.pad = 1;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == -1 && errno == EINVAL);
  info.pad = 0;
  info.sync_fence_info = 1;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == -1 && errno == EFAULT);
  int alias = dup(3);
  CHECK(alias >= 0);
  merge = (struct sync_merge_data){.name = "nested", .fd2 = alias};
  CHECK(ioctl(merged, SYNC_IOC_MERGE, &merge) == 0);
  CHECK(close(alias) == 0);
  CHECK(close(merged) == 0);
  merged = merge.fence;
  info = (struct sync_file_info){0};
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 2 && strcmp(info.name, "nested") == 0);
  merge = (struct sync_merge_data){.name = "same-context", .fd2 = 3};
  CHECK(ioctl(3, SYNC_IOC_MERGE, &merge) == 0);
  info = (struct sync_file_info){0};
  CHECK(ioctl(merge.fence, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 1);
  CHECK(close(merge.fence) == 0);
}

static void transfer(void) {
  int sockets[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
  pid_t child = fork();
  CHECK(child >= 0);
  if (child == 0) {
    close(sockets[0]);
    struct sync_merge_data merge = {.name = "child-owned", .fd2 = 3};
    if (ioctl(merged, SYNC_IOC_MERGE, &merge) < 0) _exit(10);
    char byte = 0;
    struct iovec iov = {.iov_base = &byte, .iov_len = 1};
    union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control;
    memset(&control, 0, sizeof(control));
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1,
                        .msg_control = control.bytes, .msg_controllen = sizeof(control)};
    struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
    c->cmsg_level = SOL_SOCKET;
    c->cmsg_type = SCM_RIGHTS;
    c->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(c), &merge.fence, sizeof(int));
    if (sendmsg(sockets[1], &msg, 0) != 1) _exit(11);
    // The receiver acknowledges externalization before the last sender close.
    if (read(sockets[1], &byte, 1) != 1) _exit(12);
    _exit(0);
  }
  CHECK(close(sockets[1]) == 0);
  char byte;
  struct iovec iov = {.iov_base = &byte, .iov_len = 1};
  union { struct cmsghdr align; char bytes[CMSG_SPACE(sizeof(int))]; } control;
  struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1,
                      .msg_control = control.bytes, .msg_controllen = sizeof(control)};
  CHECK(recvmsg(sockets[0], &msg, 0) == 1);
  struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
  CHECK(c && c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS);
  int received;
  memcpy(&received, CMSG_DATA(c), sizeof(int));
  CHECK(write(sockets[0], "x", 1) == 1);
  int status;
  CHECK(waitpid(child, &status, 0) == child);
  CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(close(sockets[0]) == 0);
  CHECK(close(merged) == 0);
  CHECK(close(3) == 0 && close(4) == 0);
  merged = received;
  struct sync_file_info info = {0};
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 2 && strcmp(info.name, "child-owned") == 0);
}

static void signal_states(void) {
  struct pollfd p = {.fd = merged, .events = POLLIN};
  CHECK(poll(&p, 1, 0) == 0);
  CHECK(write(5, "a", 1) == 1);
  char ack;
  CHECK(read(5, &ack, 1) == 1);
  struct sync_fence_info fences[2];
  struct sync_file_info info = {.num_fences = 2, .sync_fence_info = (uintptr_t)fences};
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.status == 0 && info.num_fences == 2);
  int signaled = fences[0].status == 1 ? 0 : 1;
  CHECK(fences[signaled].status == 1 && fences[signaled].timestamp_ns == 111);
  CHECK(fences[1 - signaled].status == 0 && fences[1 - signaled].timestamp_ns == 0);
  CHECK(poll(&p, 1, 0) == 0);
  {
    struct sync_merge_data merge = {.name = "pending", .fd2 = merged};
    CHECK(ioctl(merged, SYNC_IOC_MERGE, &merge) == 0);
    struct sync_fence_info pending;
    struct sync_file_info pruned = {.num_fences = 1, .sync_fence_info = (uintptr_t)&pending};
    CHECK(ioctl(merge.fence, SYNC_IOC_FILE_INFO, &pruned) == 0);
    CHECK(pruned.num_fences == 1 && pruned.status == 0 && pending.timestamp_ns == 0);
    CHECK(strcmp(pruned.name, "pending") == 0);
    CHECK(close(merge.fence) == 0);
  }
  CHECK(write(5, "b", 1) == 1);
  CHECK(read(5, &ack, 1) == 1);
  CHECK(poll(&p, 1, 5000) == 1 && p.revents == POLLIN);
  info.num_fences = 2;
  CHECK(ioctl(merged, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.status == -5);
  CHECK(fences[signaled].status == 1 && fences[signaled].timestamp_ns == 111);
  CHECK(fences[1 - signaled].status == -5 && fences[1 - signaled].timestamp_ns == 222);
  for (int i = 0; i < 2; i++) CHECK(fences[i].flags == 0);
  // An all-signaled merge is Linux 6.12's private successful stub.
  struct sync_merge_data merge = {.name = "complete", .fd2 = merged};
  CHECK(ioctl(merged, SYNC_IOC_MERGE, &merge) == 0);
  info.num_fences = 2;
  CHECK(ioctl(merge.fence, SYNC_IOC_FILE_INFO, &info) == 0);
  CHECK(info.num_fences == 1 && info.status == 1);
  CHECK(strcmp(info.name, "complete") == 0);
  CHECK(fences[0].status == 1 && fences[0].timestamp_ns == 222);
  CHECK(close(merge.fence) == 0);
  CHECK(close(merged) == 0 && close(5) == 0);
}

int main(void) {
  RUN(metadata);
  if (!failures) RUN(transfer);
  if (!failures) RUN(signal_states);
  DONE();
}
