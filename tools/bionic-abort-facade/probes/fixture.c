#include <android/set_abort_message.h>
#include <stdlib.h>

__attribute__((visibility("default"))) int bionic_abort_fixture_message(void) {
  android_set_abort_message("android-fixture-message");
  android_set_abort_message("must-not-replace-first");
  return 42;
}

__attribute__((visibility("default"), noreturn)) void
bionic_abort_fixture_abort(void) {
  abort();
}
