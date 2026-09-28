#include <errno.h>
#include <stdint.h>
#include <stdlib.h>

static int Offset(const char* start, const char* end) {
  return (int)(end - start);
}

static uint64_t DoubleBits(double value) {
  union {
    double value;
    uint64_t bits;
  } bits = {value};
  return bits.bits;
}

static uint32_t FloatBits(float value) {
  union {
    float value;
    uint32_t bits;
  } bits = {value};
  return bits.bits;
}

__attribute__((visibility("default"))) int bionic_float_fixture_basic(void) {
  char* end = 0;
  const char* input = " \t\v\f\r\n0x1.2p3tail";
  errno = 701;
  if (DoubleBits(strtod(input, &end)) != UINT64_C(0x4022000000000000) ||
      Offset(input, end) != 13 || errno != 701)
    return 1;

  input = "muppet";
  errno = 702;
  if (DoubleBits(strtod(input, &end)) != 0 || end != input || errno != 702)
    return 2;

  input = "-infinitude";
  if (DoubleBits(strtod(input, &end)) != UINT64_C(0xfff0000000000000) ||
      Offset(input, end) != 4)
    return 3;

  input = "nan(0xff)tail";
  if ((DoubleBits(strtod(input, &end)) & UINT64_C(0x7ff0000000000000)) !=
          UINT64_C(0x7ff0000000000000) ||
      Offset(input, end) != 9)
    return 4;

  input = "1e5000!";
  errno = 0;
  if (DoubleBits(strtod(input, &end)) != UINT64_C(0x7ff0000000000000) ||
      *end != '!' || errno != ERANGE)
    return 5;

  input = "1e-5000!";
  errno = 0;
  if (DoubleBits(strtod(input, &end)) != 0 || *end != '!' || errno != ERANGE)
    return 6;

  input = "2.2250738585072012e-308";
  if (DoubleBits(strtod(input, &end)) != UINT64_C(0x0010000000000000) ||
      *end != '\0')
    return 7;

  input = "7.0064923216240853546186479164495e-46";
  if (FloatBits(strtof(input, &end)) != 0 || *end != '\0') return 8;
  input = "7.0064923216240853546186479164496e-46";
  if (FloatBits(strtof(input, &end)) != 1 || *end != '\0') return 9;

  input = "-0x0p0";
  if (FloatBits(strtof(input, &end)) != UINT32_C(0x80000000) || *end != '\0')
    return 10;
  input = "1e+z";
  if (DoubleBits(strtod(input, &end)) != UINT64_C(0x3ff0000000000000) ||
      Offset(input, end) != 1)
    return 11;
  input = "2.5tail";
  if (DoubleBits(strtod_l(input, &end, (locale_t)(uintptr_t)0x12345)) !=
          UINT64_C(0x4004000000000000) ||
      Offset(input, end) != 3)
    return 12;
  input = "3.25tail";
  if (FloatBits(strtof_l(input, &end, (locale_t)(uintptr_t)0x54321)) !=
          UINT32_C(0x40500000) ||
      Offset(input, end) != 4)
    return 13;
  return 42;
}

__attribute__((visibility("default"))) int bionic_float_fixture_thread(
    uint32_t seed) {
  char* end = 0;
  errno = (int)(1000 + seed);
  if (FloatBits(strtof("0x1.fffffep127!", &end)) != UINT32_C(0x7f7fffff) ||
      *end != '!' || errno != (int)(1000 + seed))
    return 20;
  errno = 0;
  if (FloatBits(strtof("1e1000!", &end)) != UINT32_C(0x7f800000) ||
      *end != '!' || errno != ERANGE)
    return 21;
  return 42;
}
