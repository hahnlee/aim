#include <errno.h>
#include <fenv.h>

// Audit executable only. Never a member of the production conversion archive.
extern "C" void darwin_art_bionic_binary128_conversion_test_prepare_host_state() {
  errno = 31997;
  fesetround(FE_DOWNWARD);
  feraiseexcept(FE_DIVBYZERO);
}

extern "C" int
darwin_art_bionic_binary128_conversion_test_host_state_is_preserved() {
  return errno == 31997 && fegetround() == FE_DOWNWARD &&
         (fetestexcept(FE_DIVBYZERO) != 0);
}
