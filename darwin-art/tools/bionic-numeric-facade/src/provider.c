#include "darwin_art_bionic_numeric.h"

#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>

extern void darwin_art_bionic_errno_store(int32_t android_errno);

_Static_assert(sizeof(long) == 8, "Android arm64 long width drift");
_Static_assert(sizeof(unsigned long) == 8,
               "Android arm64 unsigned long width drift");
_Static_assert(sizeof(intmax_t) == 8, "Android arm64 intmax_t width drift");
_Static_assert(INTMAX_MAX == INT64_MAX, "Android arm64 intmax_t limit drift");

enum {
  kAndroidEinval = 22,
  kAndroidErange = 34,
};

typedef struct ParseResult {
  uint64_t magnitude;
  const char* end;
  int negative;
  int converted;
  int overflow;
  int invalid_base;
} ParseResult;

static int IsSpace(unsigned char c) {
  return c == ' ' || c == '\t' || c == '\n' || c == '\v' || c == '\f' ||
         c == '\r';
}

static int IsDigit(unsigned char c) { return c >= '0' && c <= '9'; }

static int DigitValue(unsigned char c) {
  if (c >= '0' && c <= '9') return (int)(c - '0');
  if (c >= 'a' && c <= 'z') return 10 + (int)(c - 'a');
  if (c >= 'A' && c <= 'Z') return 10 + (int)(c - 'A');
  return -1;
}

static ParseResult ParseMagnitude(const char* input, int base, uint64_t limit) {
  ParseResult result = {0, input, 0, 0, 0, 0};
  if (base < 0 || base == 1 || base > 36) {
    result.invalid_base = 1;
    return result;
  }

  const char* p = input;
  while (IsSpace((unsigned char)*p)) ++p;
  if (*p == '-' || *p == '+') {
    result.negative = *p == '-';
    ++p;
  }

  int c = (unsigned char)*p++;
  if ((base == 0 || base == 16) && c == '0' &&
      (*p == 'x' || *p == 'X') && DigitValue((unsigned char)p[1]) >= 0 &&
      DigitValue((unsigned char)p[1]) < 16) {
    c = (unsigned char)p[1];
    p += 2;
    base = 16;
  }
  if ((base == 0 || base == 2) && c == '0' &&
      (*p == 'b' || *p == 'B') && IsDigit((unsigned char)p[1])) {
    c = (unsigned char)p[1];
    p += 2;
    base = 2;
  }
  if (base == 0) base = c == '0' ? 8 : 10;

  for (;;) {
    const int digit = DigitValue((unsigned char)c);
    if (digit < 0 || digit >= base) break;
    result.converted = 1;
    if (!result.overflow) {
      const uint64_t value = (uint64_t)digit;
      if (result.magnitude > (limit - value) / (uint64_t)base) {
        result.overflow = 1;
      } else {
        result.magnitude = result.magnitude * (uint64_t)base + value;
      }
    }
    c = (unsigned char)*p++;
  }
  result.end = result.converted ? p - 1 : input;
  return result;
}

static int64_t ParseSigned(const char* input, char** end, int base) {
  const char* scan = input;
  while (IsSpace((unsigned char)*scan)) ++scan;
  const int negative = *scan == '-';
  const uint64_t limit = negative ? UINT64_C(0x8000000000000000)
                                  : UINT64_C(0x7fffffffffffffff);
  const ParseResult parsed = ParseMagnitude(input, base, limit);
  if (end != NULL) *end = (char*)parsed.end;
  if (parsed.invalid_base) {
    darwin_art_bionic_errno_store(kAndroidEinval);
    return 0;
  }
  if (parsed.overflow) {
    darwin_art_bionic_errno_store(kAndroidErange);
    return parsed.negative ? INT64_MIN : INT64_MAX;
  }
  if (parsed.negative) {
    if (parsed.magnitude == UINT64_C(0x8000000000000000)) return INT64_MIN;
    return -(int64_t)parsed.magnitude;
  }
  return (int64_t)parsed.magnitude;
}

static uint64_t ParseUnsigned(const char* input, char** end, int base) {
  const ParseResult parsed = ParseMagnitude(input, base, UINT64_MAX);
  if (end != NULL) *end = (char*)parsed.end;
  if (parsed.invalid_base) {
    darwin_art_bionic_errno_store(kAndroidEinval);
    return 0;
  }
  if (parsed.overflow) {
    darwin_art_bionic_errno_store(kAndroidErange);
    return UINT64_MAX;
  }
  return parsed.negative ? UINT64_C(0) - parsed.magnitude : parsed.magnitude;
}

#define HOST_ERRNO_GUARD(body) \
  do {                         \
    const int saved = errno;   \
    body;                      \
    errno = saved;             \
    return result;             \
  } while (0)

long darwin_art_bionic_strtol(const char* input, char** end, int base) {
  HOST_ERRNO_GUARD(const long result = (long)ParseSigned(input, end, base));
}

long long darwin_art_bionic_strtoll(const char* input, char** end, int base) {
  HOST_ERRNO_GUARD(
      const long long result = (long long)ParseSigned(input, end, base));
}

intmax_t darwin_art_bionic_strtoimax(const char* input, char** end, int base) {
  HOST_ERRNO_GUARD(
      const intmax_t result = (intmax_t)ParseSigned(input, end, base));
}

unsigned long darwin_art_bionic_strtoul(const char* input, char** end,
                                        int base) {
  HOST_ERRNO_GUARD(
      const unsigned long result = (unsigned long)ParseUnsigned(input, end, base));
}

unsigned long long darwin_art_bionic_strtoull(const char* input, char** end,
                                              int base) {
  HOST_ERRNO_GUARD(const unsigned long long result =
                       (unsigned long long)ParseUnsigned(input, end, base));
}

uint64_t darwin_art_bionic_strtoumax(const char* input, char** end, int base) {
  return (uint64_t)darwin_art_bionic_strtoull(input, end, base);
}

long long darwin_art_bionic_strtoll_l(const char* input, char** end, int base,
                                     DarwinArtAndroidLocale locale) {
  (void)locale;
  return darwin_art_bionic_strtoll(input, end, base);
}

unsigned long long darwin_art_bionic_strtoull_l(
    const char* input, char** end, int base, DarwinArtAndroidLocale locale) {
  (void)locale;
  return darwin_art_bionic_strtoull(input, end, base);
}

long long darwin_art_bionic_atoll(const char* input) {
  return darwin_art_bionic_strtoll(input, NULL, 10);
}

DarwinArtBionicDiv darwin_art_bionic_div(int numerator, int denominator) {
  DarwinArtBionicDiv result = {numerator / denominator,
                               numerator % denominator};
  return result;
}

DarwinArtBionicLongLongDiv darwin_art_bionic_lldiv(long long numerator,
                                                    long long denominator) {
  DarwinArtBionicLongLongDiv result = {numerator / denominator,
                                       numerator % denominator};
  return result;
}

static int NameEquals(const char* left, const char* right) {
  if (left == NULL || right == NULL) return 0;
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return *left == *right;
}

DarwinArtBionicNumericFunction darwin_art_bionic_numeric_resolve(
    const char* name) {
  if (NameEquals(name, "atoll"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_atoll;
  if (NameEquals(name, "div"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_div;
  if (NameEquals(name, "lldiv"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_lldiv;
  if (NameEquals(name, "strtol"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtol;
  if (NameEquals(name, "strtoll"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoll;
  if (NameEquals(name, "strtoimax"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoimax;
  if (NameEquals(name, "strtoll_l"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoll_l;
  if (NameEquals(name, "strtoul"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoul;
  if (NameEquals(name, "strtoull"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoull;
  if (NameEquals(name, "strtoumax"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoumax;
  if (NameEquals(name, "strtoull_l"))
    return (DarwinArtBionicNumericFunction)darwin_art_bionic_strtoull_l;
  return NULL;
}
