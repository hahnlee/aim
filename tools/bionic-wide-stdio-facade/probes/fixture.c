#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <wchar.h>

__attribute__((visibility("default"))) int bionic_wide_stdio_fixture_run(
    FILE* input,
    FILE* output) {
  errno = 777;
  if ((uint32_t)getwc(input) != UINT32_C(0x41) || errno != 777) return 1;
  if ((uint32_t)getwc(input) != UINT32_C(0x1f600)) return 2;
  if (getwc(input) != WEOF) return 3;
  if ((uint32_t)ungetwc((wint_t)UINT32_C(0x1f642), input) !=
          UINT32_C(0x1f642) ||
      (uint32_t)getwc(input) != UINT32_C(0x1f642)) {
    return 4;
  }
  if ((uint32_t)fputwc((wchar_t)UINT32_C(0x1f600), output) !=
          UINT32_C(0x1f600) ||
      (uint32_t)fputwc((wchar_t)UINT32_C(0xd800), output) !=
          UINT32_C(0xd800)) {
    return 5;
  }
  errno = 0;
  if (fputwc((wchar_t)UINT32_C(0x200000), output) != WEOF ||
      errno != EILSEQ) {
    return 6;
  }
  return 42;
}
