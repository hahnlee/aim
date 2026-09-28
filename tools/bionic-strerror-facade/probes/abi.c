#include <stddef.h>
#include <string.h>

typedef int (*XsiStrerrorR)(int, char*, size_t);
_Static_assert(_Generic(&strerror_r, XsiStrerrorR: 1, default: 0),
               "API 35 arm64 strerror_r is not the XSI int ABI");
