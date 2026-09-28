#include "aim_bionic_errno.h"

#include <errno.h>
#include <stdint.h>

static _Thread_local int32_t gFixtureValue;

void aim_errno_fixture_prepare(int32_t value, int host_errno_value) {
  gFixtureValue = value;
  errno = host_errno_value;
}

int32_t aim_errno_fixture_thread_value(void) {
  return gFixtureValue;
}

int aim_errno_fixture_host_errno_is(int expected) {
  return errno == expected;
}

int aim_errno_fixture_cells_are_disjoint(void) {
  return (void*)aim_bionic___errno() != (void*)&errno;
}
