#pragma once

#include_next <unistd.h>

// Darwin's off_t and lseek are already 64-bit. Android/Linux exposes the
// equivalent entry point under the lseek64 spelling as well.
#if !defined(AIM_AOSP_COMPAT_LSEEK64)
static inline off_t lseek64(int fd, off_t offset, int whence) {
  return lseek(fd, offset, whence);
}
#endif
