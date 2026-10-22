// preadv/pwritev and preadv2/pwritev2 as readv(2) has them: only the v2
// calls take offset -1 (the current position); RWF_APPEND writes at the
// end and RWF_NOAPPEND at the position, whatever the description's
// O_APPEND; a positioned write on an O_APPEND description appends, as
// pwrite(2) BUGS has it; RWF_DSYNC and RWF_SYNC write through; unknown flags, and
// RWF_NOWAIT and RWF_ATOMIC no descriptor here offers, are EOPNOTSUPP.
#include <fcntl.h>
#include <linux/fs.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <unistd.h>

#include "check.h"

static char base[256];

static int file(const char* name, int flags) {
  char path[512];
  snprintf(path, sizeof path, "%s/%s", base, name);
  int fd = open(path, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC | flags, 0644);
  if (fd >= 0 && write(fd, "abcdef", 6) != 6) {
    close(fd);
    return -1;
  }
  return fd;
}

static int contents(int fd, char* out, size_t size) {
  memset(out, 0, size);
  return pread(fd, out, size - 1, 0);
}

static void offsets(void) {
  int fd = file("offsets", 0);
  CHECK(fd >= 0);
  char b[4] = {0};
  struct iovec v = {b, 2};
  errno = 0;
  CHECK(preadv(fd, &v, 1, -1) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(pwritev(fd, &v, 1, -2) == -1 && errno == EINVAL);
  errno = 0;
  CHECK(preadv2(fd, &v, 1, -2, 0) == -1 && errno == EINVAL);
  // -1 reads at the current position and moves it.
  CHECK(lseek(fd, 1, SEEK_SET) == 1);
  CHECK(preadv2(fd, &v, 1, -1, 0) == 2 && memcmp(b, "bc", 2) == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 3);
  // An explicit offset leaves it.
  CHECK(preadv2(fd, &v, 1, 4, RWF_HIPRI) == 2 && memcmp(b, "ef", 2) == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 3);
  // EINVAL comes before the descriptor's EBADF.
  errno = 0;
  CHECK(preadv(-1, &v, 1, -1) == -1 && errno == EINVAL);
  close(fd);
}

static void unsupported(void) {
  int fd = file("unsupported", 0);
  CHECK(fd >= 0);
  char b[2];
  struct iovec v = {b, 2};
  errno = 0;
  CHECK(preadv2(fd, &v, 1, 0, 0x1000) == -1 && errno == EOPNOTSUPP);
  errno = 0;
  CHECK(preadv2(fd, &v, 1, 0, RWF_NOWAIT) == -1 && errno == EOPNOTSUPP);
  errno = 0;
  CHECK(pwritev2(fd, &v, 1, 0, RWF_ATOMIC) == -1 && errno == EOPNOTSUPP);
  close(fd);
}

static void append(void) {
  char out[32];
  int fd = file("append", 0);
  CHECK(fd >= 0);
  struct iovec v = {"XY", 2};
  // RWF_APPEND at an offset writes the end and leaves the position.
  CHECK(lseek(fd, 1, SEEK_SET) == 1);
  CHECK(pwritev2(fd, &v, 1, 0, RWF_APPEND) == 2);
  CHECK(contents(fd, out, sizeof out) == 8 && strcmp(out, "abcdefXY") == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 1);
  // At the current position, it moves the position past the write.
  CHECK(pwritev2(fd, &v, 1, -1, RWF_APPEND) == 2);
  CHECK(lseek(fd, 0, SEEK_CUR) == 10);
  close(fd);
  // RWF_NOAPPEND on an O_APPEND description writes at the position.
  fd = file("noappend", O_APPEND);
  CHECK(fd >= 0);
  CHECK(lseek(fd, 2, SEEK_SET) == 2);
  CHECK(pwritev2(fd, &v, 1, -1, RWF_NOAPPEND) == 2);
  CHECK(contents(fd, out, sizeof out) == 6 && strcmp(out, "abXYef") == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 4);
  close(fd);
}

// pwrite(2) BUGS: on an O_APPEND description a positioned write appends
// whatever its offset, and leaves the position; RWF_NOAPPEND writes at it.
static void described_append(void) {
  char out[32];
  int fd = file("described", O_APPEND);
  CHECK(fd >= 0);
  CHECK(lseek(fd, 1, SEEK_SET) == 1);
  CHECK(pwrite(fd, "P", 1, 0) == 1);
  struct iovec v = {"V", 1};
  CHECK(pwritev(fd, &v, 1, 0) == 1);
  struct iovec w = {"W", 1};
  CHECK(pwritev2(fd, &w, 1, 0, 0) == 1);
  CHECK(contents(fd, out, sizeof out) == 9 && strcmp(out, "abcdefPVW") == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 1);
  struct iovec n = {"N", 1};
  CHECK(pwritev2(fd, &n, 1, 0, RWF_NOAPPEND) == 1);
  CHECK(contents(fd, out, sizeof out) == 9 && strcmp(out, "NbcdefPVW") == 0);
  CHECK(lseek(fd, 0, SEEK_CUR) == 1);
  close(fd);
}

static void sync_flags(void) {
  int fd = file("sync", 0);
  CHECK(fd >= 0);
  struct iovec v = {"zz", 2};
  CHECK(pwritev2(fd, &v, 1, 0, RWF_DSYNC) == 2);
  CHECK(pwritev2(fd, &v, 1, 2, RWF_SYNC) == 2);
  char out[16];
  CHECK(contents(fd, out, sizeof out) == 6 && strcmp(out, "zzzzef") == 0);
  close(fd);
}

int main(int argc, char** argv) {
  snprintf(base, sizeof base, "%s/rwf-%d", argc > 1 ? argv[1] : "/data/local/tmp", getpid());
  mkdir(base, 0755);
  RUN(offsets);
  RUN(unsupported);
  RUN(append);
  RUN(described_append);
  RUN(sync_flags);
  DONE();
}
