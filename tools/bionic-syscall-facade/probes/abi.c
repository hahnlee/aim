#include <stdarg.h>
#include <stddef.h>

#if defined(__ANDROID__)
_Static_assert(sizeof(va_list) == 32, "Android arm64 va_list width");
#else
_Static_assert(sizeof(va_list) == 8, "Darwin arm64 va_list width");
#endif

size_t SyscallVaListSize(void) { return sizeof(va_list); }

long ReadVariadicLong(va_list arguments) { return va_arg(arguments, long); }
