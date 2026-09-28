#include <stdint.h>
#include <wchar.h>

_Static_assert(sizeof(wchar_t) == 4, "Android wchar32 required");
_Static_assert(sizeof(long double) == 16, "Android binary128 required");

static int equal(const wchar_t* left, const wchar_t* right) {
  while (*left == *right) {
    if (*left++ == 0) return 1;
    ++right;
  }
  return 0;
}

__attribute__((visibility("default"))) int swprintf_fixture(void) {
  wchar_t output[128];
  if (swprintf(output, 128, L"%f", 1.25) != 8 ||
      !equal(output, L"1.250000")) return 1;
  long double value = 1.0L / 8.0L;
  if (swprintf(output, 128, L"%Lf", value) != 8 ||
      !equal(output, L"0.125000")) return 2;
  if (swprintf(output, 128, L"%Lf", -0.0L) != 9 ||
      !equal(output, L"-0.000000")) return 3;
  if (swprintf(output, 4, L"%f", 1.0) != -1 || output[3] != 0) return 4;
  return 0;
}
