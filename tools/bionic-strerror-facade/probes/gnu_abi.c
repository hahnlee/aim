#define _GNU_SOURCE 1
#include <stddef.h>
#include <string.h>

typedef char* (*GnuStrerrorR)(int, char*, size_t);
_Static_assert(_Generic(&strerror_r, GnuStrerrorR: 1, default: 0),
               "API 35 arm64 GNU strerror_r signature drift");

__attribute__((visibility("default"))) GnuStrerrorR gnu_strerror_r_address(void) {
  return &strerror_r;
}
