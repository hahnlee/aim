#include <errno.h>
#include <locale.h>
#include <stdint.h>
#include <stdlib.h>
#include <wchar.h>

typedef union {
  long double value;
  uint64_t word[2];
} Binary128;

_Static_assert(sizeof(long double) == 16, "Android arm64 binary128 required");
_Static_assert(sizeof(wchar_t) == 4, "Android wchar32 required");

__attribute__((noinline)) static int bits_equal(const volatile Binary128* value,
                                                uint64_t low,
                                                uint64_t high) {
  return value->word[0] == low && value->word[1] == high;
}

__attribute__((visibility("default"))) int binary128_fixture_basic(void) {
  char* end = 0;
  Binary128 value = {.value = strtold("1.5x", &end)};
  if (!bits_equal(&value, UINT64_C(0), UINT64_C(0x3fff800000000000)) ||
      *end != 'x') return 1;
  value.value = strtold("-0", &end);
  if (!bits_equal(&value, 0, UINT64_C(0x8000000000000000)) || *end != 0)
    return 2;
  value.value = strtold("0x0.0000000000000000000000000001p-16382", &end);
  if (!bits_equal(&value, 1, 0) || *end != 0) return 3;
  value.value = strtold("0x1.ffffffffffffffffffffffffffffp+16383", &end);
  if (!bits_equal(&value, UINT64_MAX, UINT64_C(0x7ffeffffffffffff)) ||
      *end != 0) return 4;
  value.value = strtold("inf", &end);
  if (!bits_equal(&value, 0, UINT64_C(0x7fff000000000000)) || *end != 0)
    return 5;
  value.value = strtold("nan(0x1234)", &end);
  if (!bits_equal(&value, UINT64_C(0x1234),
                  UINT64_C(0x7fff000000000000)) ||
      *end != 0)
    return 6;

  value.value = strtold_l("2.25!", &end, (locale_t)(uintptr_t)0x12345);
  if (!bits_equal(&value, 0, UINT64_C(0x4000200000000000)) || *end != '!')
    return 7;

  const wchar_t wide[] = {0x2003, '-', '2', '.', '5', '?', 0};
  wchar_t* wide_end = 0;
  value.value = wcstold(wide, &wide_end);
  if (!bits_equal(&value, 0, UINT64_C(0xc000400000000000)) ||
      *wide_end != '?') return 8;

  errno = 0;
  value.value = strtold("1e5000", &end);
  if (!bits_equal(&value, 0, UINT64_C(0x7fff000000000000)) || *end != 0)
    return 9;
  return errno == ERANGE ? 42 : 10;
}

__attribute__((visibility("default"))) int binary128_fixture_thread(
    uint32_t seed) {
  char* end = 0;
  const char* text = (seed & 1) ? "-3.5z" : "4.5z";
  Binary128 value = {.value = strtold(text, &end)};
  const uint64_t expected =
      (seed & 1) ? UINT64_C(0xc000c00000000000) : UINT64_C(0x4001200000000000);
  return bits_equal(&value, 0, expected) && *end == 'z' ? 42 : 1;
}
