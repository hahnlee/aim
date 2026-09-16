#include <errno.h>
#include <inttypes.h>
#include <locale.h>
#include <stdint.h>
#include <stdlib.h>

static int Offset(const char* start, const char* end) {
  return (int)(end - start);
}

__attribute__((visibility("default"))) int bionic_numeric_fixture_basic(void) {
  char* end;
  const char* text = " \t-0x2aZ";
  errno = 701;
  if (strtol(text, &end, 0) != -42 || Offset(text, end) != 7 || errno != 701)
    return 1;
  text = "0b101101tail";
  if (strtoll(text, &end, 0) != 45 || Offset(text, end) != 8) return 2;
  text = "0779";
  if (strtoul(text, &end, 0) != 63 || Offset(text, end) != 3) return 3;
  text = "-18446744073709551615x";
  errno = 702;
  if (strtoull(text, &end, 10) != 1 || Offset(text, end) != 21 || errno != 702)
    return 4;
  text = "9223372036854775808!";
  errno = 0;
  if (strtol(text, &end, 10) != INT64_MAX || Offset(text, end) != 19 ||
      errno != ERANGE)
    return 5;
  text = "-9223372036854775809!";
  errno = 0;
  if (strtoll(text, &end, 10) != INT64_MIN || Offset(text, end) != 20 ||
      errno != ERANGE)
    return 6;
  text = "18446744073709551616!";
  errno = 0;
  if (strtoull(text, &end, 10) != UINT64_MAX || Offset(text, end) != 20 ||
      errno != ERANGE)
    return 7;
  text = "  +xyz";
  errno = 703;
  if (strtol(text, &end, 10) != 0 || end != text || errno != 703) return 8;
  text = "123";
  errno = 0;
  if (strtoll(text, &end, 1) != 0 || end != text || errno != EINVAL) return 9;
  text = "0x";
  errno = 704;
  if (strtol(text, &end, 0) != 0 || Offset(text, end) != 1 || errno != 704)
    return 10;
  text = "0b2";
  errno = 705;
  if (strtol(text, &end, 2) != 0 || end != text || errno != 705) return 11;
  text = "z!";
  if (strtoul(text, &end, 36) != 35 || Offset(text, end) != 1) return 12;
  text = "-9223372036854775808";
  errno = 706;
  if (strtoll_l(text, &end, 10, (locale_t)(uintptr_t)0x1234) != INT64_MIN ||
      *end != '\0' || errno != 706)
    return 13;
  text = "-1";
  if (strtoull_l(text, &end, 10, (locale_t)(uintptr_t)1) != UINT64_MAX ||
      *end != '\0')
    return 14;
  text = "9223372036854775807!";
  errno = 707;
  if (strtoimax(text, &end, 10) != INTMAX_MAX || Offset(text, end) != 19 ||
      errno != 707)
    return 15;
  text = "-9223372036854775808!";
  errno = 708;
  if (strtoimax(text, &end, 10) != INTMAX_MIN || Offset(text, end) != 20 ||
      errno != 708)
    return 16;
  text = "9223372036854775808!";
  errno = 0;
  if (strtoimax(text, &end, 10) != INTMAX_MAX || Offset(text, end) != 19 ||
      errno != ERANGE)
    return 17;
  text = "-9223372036854775809!";
  errno = 0;
  if (strtoimax(text, &end, 10) != INTMAX_MIN || Offset(text, end) != 20 ||
      errno != ERANGE)
    return 18;
  text = "123";
  errno = 709;
  if (strtoimax(text, &end, 1) != 0 || end != text || errno != EINVAL)
    return 19;
  return 42;
}

__attribute__((visibility("default"))) int bionic_numeric_fixture_thread(
    uint32_t seed) {
  char* end;
  errno = (int)(1000 + seed);
  if (strtoull("ffffffffffffffff", &end, 16) != UINT64_MAX || *end != '\0' ||
      errno != (int)(1000 + seed))
    return 20;
  errno = 0;
  if (strtoll("999999999999999999999999999999q", &end, 10) != INT64_MAX ||
      *end != 'q' || errno != ERANGE)
    return 21;
  errno = (int)(2000 + seed);
  if (strtol("-1010102", &end, 2) != -42 || *end != '2' ||
      errno != (int)(2000 + seed))
    return 22;
  return 42;
}
