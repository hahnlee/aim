#include <bits/struct_file.h>
#include <stdarg.h>
#include <stdio.h>

_Static_assert(sizeof(FILE) == 152, "Android FILE size drift");
_Static_assert(_Alignof(FILE) == 8, "Android FILE alignment drift");
_Static_assert(sizeof(va_list) == 32, "Android AArch64 va_list size drift");

static int consume(FILE* file, const char* format, ...) {
  va_list args;
  va_start(args, format);
  int result = vfprintf(file, format, args);
  va_end(args);
  return result;
}

int abi_probe(FILE* file) {
  return fprintf(file, "%d", 1) + consume(file, "%d", 2);
}
