#include "aim_bionic_ioctl.h"

#include <stdarg.h>
#include <stddef.h>

#if defined(__ANDROID__)
_Static_assert(sizeof(va_list) == 32, "Android arm64 va_list width");
#else
_Static_assert(sizeof(va_list) == 8, "Darwin arm64 va_list width");
#endif

_Static_assert(sizeof(AimBionicIoctlFdInfo) == 8, "fd info size");
_Static_assert(offsetof(AimBionicIoctlFdInfo, kind) == 4,
               "kind offset");

size_t IoctlVaListSize(void) { return sizeof(va_list); }

void* ReadVariadicPointer(va_list arguments) {
  return va_arg(arguments, void*);
}
