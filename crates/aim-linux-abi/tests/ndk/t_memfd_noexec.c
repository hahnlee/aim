// vm.memfd_noexec (mm/memfd.c check_sysctl_memfd_noexec): what a memfd
// asked for with neither MFD_EXEC nor MFD_NOEXEC_SEAL gets, and refusing
// MFD_EXEC. Run as `t_memfd_noexec SCOPE` with /proc/sys/vm/memfd_noexec
// holding SCOPE.
#include <fcntl.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include "check.h"

static int scope;

// The exec bits of a memfd made with `flags`, and its seals; -1 if
// memfd_create failed (errno kept).
static int make(unsigned flags, int* seals) {
  int fd = memfd_create("noexec", flags);
  if (fd < 0) return -1;
  struct stat st;
  int exec = fstat(fd, &st) == 0 ? (int)(st.st_mode & 0111) : -2;
  *seals = fcntl(fd, F_GET_SEALS);
  close(fd);
  return exec;
}

static void sysctl_read(void) {
  char value[8] = {0};
  int fd = open("/proc/sys/vm/memfd_noexec", O_RDONLY | O_CLOEXEC);
  CHECK(fd >= 0);
  CHECK(read(fd, value, sizeof value - 1) > 0 && atoi(value) == scope);
  close(fd);
}

static void defaults(void) {
  int seals = 0;
  switch (scope) {
    case 0:
      // MFD_EXEC: executable, unsealable without MFD_ALLOW_SEALING.
      CHECK(make(MFD_CLOEXEC, &seals) == 0111 && seals == F_SEAL_SEAL);
      CHECK(make(MFD_ALLOW_SEALING, &seals) == 0111 && seals == 0);
      break;
    case 1:
      // MFD_NOEXEC_SEAL: no exec bits, F_SEAL_EXEC, sealable.
      CHECK(make(MFD_CLOEXEC, &seals) == 0 && seals == F_SEAL_EXEC);
      CHECK(make(MFD_EXEC, &seals) == 0111 && seals == F_SEAL_SEAL);
      break;
    default:
      // Enforced: MFD_EXEC is refused, MFD_NOEXEC_SEAL is the default.
      CHECK(make(MFD_CLOEXEC, &seals) == 0 && seals == F_SEAL_EXEC);
      errno = 0;
      CHECK(make(MFD_EXEC, &seals) == -1 && errno == EACCES);
      CHECK(make(MFD_NOEXEC_SEAL, &seals) == 0 && seals == F_SEAL_EXEC);
      break;
  }
  // Both at once is EINVAL at any scope.
  errno = 0;
  CHECK(make(MFD_EXEC | MFD_NOEXEC_SEAL, &seals) == -1 && errno == EINVAL);
}

int main(int argc, char** argv) {
  scope = argc > 1 ? atoi(argv[1]) : 0;
  RUN(sysctl_read);
  RUN(defaults);
  DONE();
}
