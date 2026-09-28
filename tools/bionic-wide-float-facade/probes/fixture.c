#include <errno.h>
#include <stdint.h>
#include <wchar.h>

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

static int Offset(const wchar_t* start, const wchar_t* end) {
  return (int)(end - start);
}

__attribute__((visibility("default"))) int bionic_wide_float_fixture_basic(
    void) {
  wchar_t* end = 0;
  static const wchar_t hex[] = {0x2003, '0', 'x', '1', '.', '2', 'p', '3',
                                't',    'a', 'i', 'l', 0};
  errno = 701;
  if (DoubleBits(wcstod(hex, &end)) != UINT64_C(0x4022000000000000) ||
      Offset(hex, end) != 8 || errno != 701)
    return 1;

  static const wchar_t invalid[] = {0x00a0, 'm', 'u', 'p', 'p', 'e', 't', 0};
  errno = 702;
  if (DoubleBits(wcstod(invalid, &end)) != 0 || end != invalid || errno != 702)
    return 2;

  static const wchar_t not_space[] = {0x200b, '4', '2', 0};
  if (DoubleBits(wcstod(not_space, &end)) != 0 || end != not_space) return 3;

  static const wchar_t nan_payload[] = {'n', 'a', 'n', '(', '0', 'x', 'f', 'f',
                                        ')', '!', 0};
  if ((DoubleBits(wcstod(nan_payload, &end)) &
       UINT64_C(0x7ff0000000000000)) != UINT64_C(0x7ff0000000000000) ||
      Offset(nan_payload, end) != 9)
    return 4;

  static const wchar_t overflow[] = {'1', 'e', '5', '0', '0', '0', '!', 0};
  errno = 0;
  if (DoubleBits(wcstod(overflow, &end)) != UINT64_C(0x7ff0000000000000) ||
      Offset(overflow, end) != 6 || errno != ERANGE)
    return 5;

  static const wchar_t underflow[] = {
      '7', '.', '0', '0', '6', '4', '9', '2', '3', '2', '1', '6', '2', '4',
      '0', '8', '5', '3', '5', '4', '6', '1', '8', '6', '4', '7', '9', '1',
      '6', '4', '4', '9', '5', 'e', '-', '4', '6', 0};
  if (FloatBits(wcstof(underflow, &end)) != 0 || *end != 0) return 6;

  static const wchar_t unicode_tail[] = {'4', '2', '.', '5', 0x2603, '9', 0};
  if (DoubleBits(wcstod(unicode_tail, &end)) != UINT64_C(0x4045400000000000) ||
      Offset(unicode_tail, end) != 4)
    return 7;

  static const wchar_t incomplete_exponent[] = {'1', 'e', '+', 'z', 0};
  if (DoubleBits(wcstod(incomplete_exponent, &end)) !=
          UINT64_C(0x3ff0000000000000) ||
      Offset(incomplete_exponent, end) != 1)
    return 8;
  return 42;
}

__attribute__((visibility("default"))) int bionic_wide_float_fixture_thread(
    uint32_t seed) {
  wchar_t* end = 0;
  static const wchar_t maximum[] = {'0', 'x', '1', '.', 'f', 'f', 'f', 'f',
                                    'f', 'e', 'p', '1', '2', '7', '!', 0};
  errno = (int)(1000 + seed);
  if (FloatBits(wcstof(maximum, &end)) != UINT32_C(0x7f7fffff) ||
      *end != '!' || errno != (int)(1000 + seed))
    return 20;
  static const wchar_t overflow[] = {'1', 'e', '1', '0', '0', '0', '!', 0};
  errno = 0;
  if (FloatBits(wcstof(overflow, &end)) != UINT32_C(0x7f800000) ||
      *end != '!' || errno != ERANGE)
    return 21;
  return 42;
}
