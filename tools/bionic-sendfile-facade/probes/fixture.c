#include <errno.h>
#include <stdint.h>
#include <sys/sendfile.h>

__attribute__((visibility("default"))) int sendfile_fixture(void) {
  errno = 0;
  if (sendfile(21, 20, 0, 5) != 3 || errno != 0) return 10;
  if (sendfile(21, 20, 0, 5) != 2 || errno != 0) return 11;
  if (sendfile(21, 20, 0, 5) != 0 || errno != 0) return 12;
  off_t offset = 1;
  if (sendfile(22, 20, &offset, 3) != 3 || offset != 4) return 13;
  if (sendfile(22, 20, &offset, 9) != 1 || offset != 5) return 14;
  errno = 0;
  if (sendfile(21, 99, 0, 1) != -1 || errno != EBADF) return 15;
  errno = 0;
  if (sendfile(23, 20, 0, 1) != -1 || errno != EROFS) return 16;
  return 42;
}
