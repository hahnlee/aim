#include <stdarg.h>
#include <stddef.h>

#if defined(__ANDROID__) && defined(__aarch64__)
_Static_assert(sizeof(va_list) == 32, "AAPCS64 va_list must be five fields");
_Static_assert(_Alignof(va_list) == 8, "AAPCS64 va_list alignment drift");
#endif

int abi_vsnprintf(char*, size_t, const char*, va_list);
int abi_vasprintf(char**, const char*, va_list);
int abi_snprintf(char*, size_t, const char*, ...);

double abi_consume(int marker, ...) {
  va_list ap;
  va_start(ap, marker);
  int integer = va_arg(ap, int);
  double real = va_arg(ap, double);
  va_end(ap);
  return integer + real;
}
