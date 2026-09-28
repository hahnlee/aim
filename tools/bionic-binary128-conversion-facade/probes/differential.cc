#include <errno.h>
#include <fenv.h>

#include <cstdint>

#include "aim_bionic_errno.h"

extern "C" void aim_bionic_strtold_raw(const char*, char**, void*);

struct Bits {
  uint64_t low;
  uint64_t high;
};

static bool Parse(const char* text, int rounding, uint64_t low, uint64_t high) {
  Bits bits{};
  char* end = nullptr;
  if (fesetround(rounding) != 0) return false;
  aim_bionic_errno_store(777);
  aim_bionic_strtold_raw(text, &end, &bits);
  return *end == '\0' && bits.low == low && bits.high == high &&
         aim_bionic_errno_load() == 777;
}

int main() {
  const char* halfway = "0x1.00000000000000000000000000008p0";
  if (!Parse(halfway, FE_TONEAREST, 0, UINT64_C(0x3fff000000000000)))
    return 1;
  if (!Parse(halfway, FE_UPWARD, 1, UINT64_C(0x3fff000000000000)))
    return 2;
  if (!Parse(halfway, FE_DOWNWARD, 0, UINT64_C(0x3fff000000000000)))
    return 3;
  if (!Parse(halfway, FE_TOWARDZERO, 0, UINT64_C(0x3fff000000000000)))
    return 4;
  Bits bits{};
  char* end = nullptr;
  errno = 711;
  fesetround(FE_UPWARD);
  feraiseexcept(FE_DIVBYZERO);
  aim_bionic_strtold_raw("12.5tail", &end, &bits);
  if (end[0] != 't' || errno != 711 || fegetround() != FE_UPWARD ||
      fetestexcept(FE_DIVBYZERO) == 0)
    return 5;
  return 0;
}
