// close(2): a dup keeps an open description alive; process exit releases it.
#include <fcntl.h>
#include <stdlib.h>
#include <unistd.h>
#include "check.h"

static void lifetime(void) {
  int fd = open("/dev/fuse", O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  int copy = dup(fd);
  CHECK(copy >= 0);
  CHECK(close(fd) == 0);
  char ack;
  CHECK(write(3, "d", 1) == 1 && read(3, &ack, 1) == 1);
  CHECK(fcntl(copy, F_GETFD) >= 0);
  // A second session's allocation must retain the still-live first session.
  int other = open("/dev/fuse", O_RDWR | O_CLOEXEC);
  CHECK(other >= 0 && close(other) == 0);
  CHECK(close(copy) == 0);
  CHECK(write(3, "c", 1) == 1 && read(3, &ack, 1) == 1);
  fd = open("/dev/fuse", O_RDWR);
  CHECK(fd >= 0);
  CHECK(write(3, "e", 1) == 1 && read(3, &ack, 1) == 1);
  // _exit leaves the transport cleanup to the independent broker.
}

int main(void) {
  RUN(lifetime);
  printf(failures ? "FAILED %d\n" : "PASS\n", failures);
  fflush(stdout);
  _exit(failures ? 1 : 0);
}
