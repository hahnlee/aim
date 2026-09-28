#include <locale.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>

__attribute__((visibility("default"))) size_t StrftimeFixtureFormat(
    char* destination, size_t capacity, const char* format,
    const struct tm* broken_down) {
  /* Bionic's C-only implementation intentionally ignores locale_t. */
  return strftime_l(destination, capacity, format, broken_down,
                    (locale_t)(uintptr_t)0x5a5a);
}
