// File offsets of read-only regular files, which the layer may hold for the
// host while one thread seeks and reads them (#3033): lseek(2), read(2),
// readv(2) and pread(2) as on Linux, and the offset of an open description
// shared through dup, fork, exec, SCM_RIGHTS, threads and stdio as one.
// argv[1] is a writable directory; `--exec-offset FD` reports FD's offset.
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <unistd.h>

#include "check.h"

static char path[256];
static const char* self;

// A file whose byte at offset i is (char)(i * 7).
static int open_data(void) {
  int w = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  if (w < 0) {
    return -1;
  }
  char buf[8192];
  for (int i = 0; i < (int)sizeof buf; i++) {
    buf[i] = (char)(i * 7);
  }
  if (write(w, buf, sizeof buf) != (ssize_t)sizeof buf) {
    close(w);
    return -1;
  }
  close(w);
  return open(path, O_RDONLY | O_CLOEXEC);
}

static int at(off_t offset, const char* buf, int n) {
  for (int i = 0; i < n; i++) {
    if (buf[i] != (char)((offset + i) * 7)) {
      return 0;
    }
  }
  return 1;
}

static void seek_and_read(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  char buf[64];
  CHECK(lseek(fd, 0, SEEK_CUR) == 0);
  CHECK(lseek(fd, 100, SEEK_SET) == 100);
  CHECK(lseek(fd, 0, SEEK_CUR) == 100);
  CHECK(read(fd, buf, 10) == 10 && at(100, buf, 10));
  CHECK(lseek(fd, 0, SEEK_CUR) == 110);
  CHECK(lseek(fd, -10, SEEK_CUR) == 100);
  CHECK(read(fd, buf, 4) == 4 && at(100, buf, 4));
  // pread neither uses nor moves the offset.
  CHECK(pread(fd, buf, 8, 4000) == 8 && at(4000, buf, 8));
  CHECK(lseek(fd, 0, SEEK_CUR) == 104);
  struct iovec v[2] = {{buf, 3}, {buf + 3, 5}};
  CHECK(readv(fd, v, 2) == 8 && at(104, buf, 8));
  CHECK(lseek(fd, 0, SEEK_CUR) == 112);
  CHECK(preadv(fd, v, 2, 50) == 8 && at(50, buf, 8));
  CHECK(lseek(fd, 0, SEEK_CUR) == 112);
  CHECK(lseek(fd, -8, SEEK_END) == 8184);
  CHECK(read(fd, buf, sizeof buf) == 8 && at(8184, buf, 8));
  CHECK(read(fd, buf, sizeof buf) == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 8192);
  // Past the end reads nothing, and leaves the offset where it was set.
  CHECK(lseek(fd, 100000, SEEK_SET) == 100000);
  CHECK(read(fd, buf, sizeof buf) == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 100000);
  // A failed seek leaves the offset.
  errno = 0;
  CHECK(lseek(fd, -1, SEEK_SET) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(lseek(fd, -200000, SEEK_CUR) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(lseek(fd, 0, 42) == -1 && errno == EINVAL);
  CHECK(lseek(fd, 0, SEEK_CUR) == 100000);
  // A read-only description has no write, and its offset stays.
  CHECK(lseek(fd, 20, SEEK_SET) == 20);
  errno = 0;
  CHECK(write(fd, buf, 1) == -1 && errno == EBADF);
  CHECK(lseek(fd, 0, SEEK_CUR) == 20);
  CHECK(lseek(fd, 0, SEEK_DATA) == 0);
  CHECK(lseek(fd, 0, SEEK_HOLE) == 8192);
  CHECK(lseek(fd, 0, SEEK_CUR) == 8192);
  close(fd);
}

// One offset for every fd of a description.
static void dup_shares_the_offset(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  char buf[16];
  CHECK(lseek(fd, 300, SEEK_SET) == 300 && read(fd, buf, 5) == 5);
  int d = dup(fd);
  CHECK(d >= 0);
  CHECK(lseek(d, 0, SEEK_CUR) == 305);
  CHECK(read(d, buf, 5) == 5 && at(305, buf, 5));
  CHECK(lseek(fd, 0, SEEK_CUR) == 310);
  CHECK(lseek(fd, 1000, SEEK_SET) == 1000);
  CHECK(lseek(d, 0, SEEK_CUR) == 1000);
  close(d);
  // Alone again: the offset is kept.
  CHECK(lseek(fd, 0, SEEK_CUR) == 1000);
  CHECK(read(fd, buf, 4) == 4 && at(1000, buf, 4));
  int d2 = fcntl(fd, F_DUPFD_CLOEXEC, 50);
  CHECK(d2 >= 50 && lseek(d2, 0, SEEK_CUR) == 1004);
  close(d2);
  // A reopen through /proc is a description of its own.
  char proc[64];
  snprintf(proc, sizeof proc, "/proc/self/fd/%d", fd);
  int r = open(proc, O_RDONLY | O_CLOEXEC);
  CHECK(r >= 0 && lseek(r, 0, SEEK_CUR) == 0);
  CHECK(read(r, buf, 2) == 2 && at(0, buf, 2));
  CHECK(lseek(fd, 0, SEEK_CUR) == 1004);
  close(r);
  close(fd);
}

// A fork child shares the parent's descriptions: it sees the offset the
// parent set, and its reads move the parent's.
static void fork_shares_the_offset(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  char buf[16];
  CHECK(lseek(fd, 2000, SEEK_SET) == 2000 && read(fd, buf, 8) == 8);
  FORK_OR_SKIP(p);
  if (p == 0) {
    char b[16];
    int ok = lseek(fd, 0, SEEK_CUR) == 2008 && read(fd, b, 12) == 12 && at(2008, b, 12);
    ok = ok && lseek(fd, 100, SEEK_CUR) == 2120;
    _exit(ok ? 0 : 1);
  }
  int status;
  CHECK(waitpid(p, &status, 0) == p && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 2120);
  CHECK(read(fd, buf, 4) == 4 && at(2120, buf, 4));
  close(fd);
}

// A new program sees the offset its fds had at execve.
static void exec_keeps_the_offset(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  char buf[16];
  CHECK(lseek(fd, 3000, SEEK_SET) == 3000 && read(fd, buf, 7) == 7);
  CHECK(fcntl(fd, F_SETFD, 0) == 0);
  int pipefd[2];
  CHECK(pipe(pipefd) == 0);
  FORK_OR_SKIP(p);
  if (p == 0) {
    char n[16];
    snprintf(n, sizeof n, "%d", fd);
    dup2(pipefd[1], 1);
    execl(self, self, "--exec-offset", n, (char*)NULL);
    _exit(127);
  }
  close(pipefd[1]);
  char out[32] = {0};
  CHECK(read(pipefd[0], out, sizeof out - 1) > 0);
  close(pipefd[0]);
  int status;
  CHECK(waitpid(p, &status, 0) == p && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  CHECK(atoi(out) == 3007);
  // The program read 3 bytes.
  CHECK(lseek(fd, 0, SEEK_CUR) == 3010);
  close(fd);
}

// A description sent over a socket is the sender's: one offset.
static void scm_rights_shares_the_offset(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  char buf[16];
  CHECK(lseek(fd, 4000, SEEK_SET) == 4000 && read(fd, buf, 4) == 4);
  int sv[2];
  CHECK(socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, sv) == 0);
  union {
    struct cmsghdr h;
    char b[CMSG_SPACE(sizeof(int))];
  } ctl;
  memset(&ctl, 0, sizeof ctl);
  char byte = 'x';
  struct iovec iov = {&byte, 1};
  struct msghdr m = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = &ctl, .msg_controllen = sizeof ctl};
  struct cmsghdr* c = CMSG_FIRSTHDR(&m);
  c->cmsg_level = SOL_SOCKET;
  c->cmsg_type = SCM_RIGHTS;
  c->cmsg_len = CMSG_LEN(sizeof(int));
  memcpy(CMSG_DATA(c), &fd, sizeof fd);
  CHECK(sendmsg(sv[0], &m, 0) == 1);
  // The offset moves while the description is in flight.
  CHECK(read(fd, buf, 6) == 6);
  m.msg_controllen = sizeof ctl;
  CHECK(recvmsg(sv[1], &m, 0) == 1);
  c = CMSG_FIRSTHDR(&m);
  CHECK(c && c->cmsg_type == SCM_RIGHTS);
  int r;
  memcpy(&r, CMSG_DATA(c), sizeof r);
  CHECK(lseek(r, 0, SEEK_CUR) == 4010);
  CHECK(read(r, buf, 5) == 5 && at(4010, buf, 5));
  CHECK(lseek(fd, 0, SEEK_CUR) == 4015);
  close(r);
  close(sv[0]);
  close(sv[1]);
  close(fd);
}

static int thread_fd;
static void* thread_reads(void* arg) {
  char b[8];
  (void)arg;
  return (void*)(long)(lseek(thread_fd, 0, SEEK_CUR) == 5000 && read(thread_fd, b, 8) == 8 &&
                       at(5000, b, 8) && lseek(thread_fd, 0, SEEK_CUR) == 5008);
}

// Threads share one fd table, and so every offset.
static void threads_share_the_offset(void) {
  thread_fd = open_data();
  CHECK(thread_fd >= 0);
  CHECK(lseek(thread_fd, 5000, SEEK_SET) == 5000);
  pthread_t t;
  void* ok = NULL;
  CHECK(pthread_create(&t, NULL, thread_reads, NULL) == 0);
  CHECK(pthread_join(t, &ok) == 0 && ok);
  CHECK(lseek(thread_fd, 0, SEEK_CUR) == 5008);
  close(thread_fd);
}

// stdio over a seeking reader: fseeko, ftello and fread in small steps.
static void stdio_reads_where_it_seeks(void) {
  int fd = open_data();
  CHECK(fd >= 0);
  FILE* f = fdopen(fd, "r");
  CHECK(f != NULL);
  char buf[16];
  for (int i = 0; i < 200; i++) {
    off_t o = (i * 389) % 8000;
    CHECK(fseeko(f, o, SEEK_SET) == 0 && ftello(f) == o);
    CHECK(fread(buf, 1, sizeof buf, f) == sizeof buf && at(o, buf, sizeof buf));
  }
  fclose(f);
}

int main(int argc, char** argv) {
  if (argc == 3 && strcmp(argv[1], "--exec-offset") == 0) {
    int fd = atoi(argv[2]);
    char b[3];
    off_t o = lseek(fd, 0, SEEK_CUR);
    printf("%lld\n", (long long)o);
    return o >= 0 && read(fd, b, 3) == 3 && at(o, b, 3) ? 0 : 1;
  }
  self = argv[0];
  snprintf(path, sizeof path, "%s/offset-%d", argc > 1 ? argv[1] : "/data/local/tmp", getpid());
  RUN(seek_and_read);
  RUN(dup_shares_the_offset);
  RUN(fork_shares_the_offset);
  RUN(exec_keeps_the_offset);
  RUN(scm_rights_shares_the_offset);
  RUN(threads_share_the_offset);
  RUN(stdio_reads_where_it_seeks);
  unlink(path);
  DONE();
}
