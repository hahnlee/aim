#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <wchar.h>

static int Offset(const wchar_t* start, const wchar_t* end) {
  return (int)(end - start);
}

__attribute__((visibility("default"))) int bionic_wide_integer_fixture_basic(
    void) {
  wchar_t* end = 0;
  const wchar_t hex[] = {L' ', L'\t', L'-', L'0', L'x', L'2', L'a', L'Z', 0};
  errno = 701;
  if (wcstol(hex, &end, 0) != -42 || Offset(hex, end) != 7 || errno != 701)
    return 1;

  const wchar_t binary[] = {L'0', L'b', L'1', L'0', L'1', L'1', L'0', L'1',
                            L't', L'a', L'i', L'l', 0};
  if (wcstoll(binary, &end, 0) != 45 || Offset(binary, end) != 8) return 2;

  const wchar_t signed_overflow[] = {L'9', L'2', L'2', L'3', L'3', L'7', L'2',
                                     L'0', L'3', L'6', L'8', L'5', L'4', L'7',
                                     L'7', L'5', L'8', L'0', L'8', L'!', 0};
  errno = 0;
  if (wcstoll(signed_overflow, &end, 10) != INT64_MAX || *end != L'!' ||
      errno != ERANGE)
    return 3;

  const wchar_t unsigned_overflow[] = {
      L'1', L'8', L'4', L'4', L'6', L'7', L'4', L'4', L'0', L'7', L'3',
      L'7', L'0', L'9', L'5', L'5', L'1', L'6', L'1', L'6', L'!', 0};
  errno = 0;
  if (wcstoull(unsigned_overflow, &end, 10) != UINT64_MAX || *end != L'!' ||
      errno != ERANGE)
    return 4;

  const wchar_t negative_unsigned[] = {L'-', L'1', 0};
  errno = 702;
  if (wcstoul(negative_unsigned, &end, 10) != ULONG_MAX || *end != 0 ||
      errno != 702)
    return 5;

  const wchar_t invalid_base[] = {L'1', L'2', L'3', 0};
  errno = 0;
  if (wcstol(invalid_base, &end, 1) != 0 || end != invalid_base ||
      errno != EINVAL)
    return 6;

  const wchar_t non_ascii[] = {(wchar_t)0xffffffffU, L'4', L'2', 0};
  errno = 703;
  if (wcstoull(non_ascii, &end, 10) != 0 || end != non_ascii || errno != 703)
    return 7;
  const wchar_t surrogate[] = {(wchar_t)0xd800, L'1', 0};
  if (wcstol(surrogate, &end, 10) != 0 || end != surrogate) return 8;
  return 42;
}

__attribute__((visibility("default"))) int bionic_wide_integer_fixture_thread(
    uint32_t seed) {
  const wchar_t maximum[] = {L'f', L'f', L'f', L'f', L'f', L'f', L'f', L'f',
                             L'f', L'f', L'f', L'f', L'f', L'f', L'f', L'f',
                             L'!', 0};
  wchar_t* end = 0;
  errno = (int)(1000 + seed);
  if (wcstoull(maximum, &end, 16) != UINT64_MAX || *end != L'!' ||
      errno != (int)(1000 + seed))
    return 20;
  const wchar_t overflow[] = {L'9', L'9', L'9', L'9', L'9', L'9', L'9', L'9',
                              L'9', L'9', L'9', L'9', L'9', L'9', L'9', L'9',
                              L'9', L'9', L'9', L'9', L'9', L'!', 0};
  errno = 0;
  if (wcstoll(overflow, &end, 10) != INT64_MAX || *end != L'!' ||
      errno != ERANGE)
    return 21;
  return 42;
}
