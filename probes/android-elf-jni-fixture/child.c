#include <stdint.h>

extern void aim_fixture_record_lifecycle(int phase);
extern int AimFixtureGrandchildValue(void);
extern int __cxa_atexit(void (*function)(void*), void* argument, void* dso);

__attribute__((visibility("hidden"))) void* __dso_handle = &__dso_handle;

static int g_child_state;

static void ChildCxaFinalize(void* argument) {
  if (argument == &g_child_state) {
    aim_fixture_record_lifecycle(6);
  }
}

__attribute__((constructor)) static void ChildInitialize(void) {
  g_child_state = AimFixtureGrandchildValue() + 10;
  if (__cxa_atexit(&ChildCxaFinalize, &g_child_state, __dso_handle) != 0) {
    g_child_state = -1;
  }
  aim_fixture_record_lifecycle(1);
}

__attribute__((destructor)) static void ChildFinalize(void) {
  aim_fixture_record_lifecycle(7);
}

__attribute__((visibility("default"))) int AimFixtureChildValue(void) {
  return g_child_state;
}
