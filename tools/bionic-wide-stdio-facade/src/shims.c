#include "aim_bionic_wide_stdio.h"

#include <errno.h>

#define PRESERVE_HOST_ERRNO(call) \
  do {                            \
    int saved_errno = errno;      \
    uint32_t result = (call);     \
    errno = saved_errno;          \
    return result;                \
  } while (0)

uint32_t aim_bionic_fputwc(uint32_t wc, AimAndroidFile* file) {
  PRESERVE_HOST_ERRNO(aim_bionic_wide_stdio_fputwc_core(wc, file));
}

uint32_t aim_bionic_getwc(AimAndroidFile* file) {
  PRESERVE_HOST_ERRNO(aim_bionic_wide_stdio_getwc_core(file));
}

uint32_t aim_bionic_ungetwc(uint32_t wc, AimAndroidFile* file) {
  PRESERVE_HOST_ERRNO(aim_bionic_wide_stdio_ungetwc_core(wc, file));
}
