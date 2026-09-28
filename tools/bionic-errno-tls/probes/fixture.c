#include <errno.h>
#include <stdint.h>

extern int32_t aim_errno_fixture_thread_value(void);

__attribute__((visibility("default"))) int bionic_errno_fixture_run(void) {
  int* first = __errno();
  if (first == 0 || *first != 0) return -1001;
  const int32_t wanted = aim_errno_fixture_thread_value();
  errno = wanted;
  int* second = __errno();
  if (second != first) return -1002;
  return errno == wanted ? errno : -1003;
}
