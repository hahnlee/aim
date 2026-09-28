#include "aim_bionic_wide_integer.h"

#include <errno.h>
#include <fenv.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

extern void aim_bionic_errno_store(int32_t android_errno);

_Static_assert(sizeof(long) == 8, "Darwin/Android arm64 long width drift");
_Static_assert(sizeof(AimAndroidWchar) == 4,
               "Android wchar_t width drift");

enum {
  kAndroidEinval = 22,
  kAndroidErange = 34,
};

typedef struct ParseResult {
  uint64_t magnitude;
  const AimAndroidWchar* end;
  int negative;
  int converted;
  int overflow;
  int invalid_base;
} ParseResult;

typedef struct HostState {
  int saved_errno;
  fenv_t saved_environment;
} HostState;

static HostState SaveHostState(void) {
  HostState state;
  state.saved_errno = errno;
  fegetenv(&state.saved_environment);
  return state;
}

static void RestoreHostState(const HostState* state) {
  fesetenv(&state->saved_environment);
  errno = state->saved_errno;
}

static int IsSpace(AimAndroidWchar value) {
  return value == ' ' || (value >= '\t' && value <= '\r');
}

static int IsDigit(AimAndroidWchar value) {
  return value >= '0' && value <= '9';
}

static int DigitValue(AimAndroidWchar value) {
  if (value >= '0' && value <= '9') return (int)(value - '0');
  value |= 0x20;
  if (value >= 'a' && value <= 'z') return 10 + (int)(value - 'a');
  return -1;
}

static ParseResult ParseMagnitude(const AimAndroidWchar* input,
                                  int base,
                                  uint64_t limit) {
  ParseResult result = {0, input, 0, 0, 0, 0};
  if (base < 0 || base == 1 || base > 36) {
    result.invalid_base = 1;
    return result;
  }
  const AimAndroidWchar* cursor = input;
  while (IsSpace(*cursor)) ++cursor;
  if (*cursor == '-' || *cursor == '+') {
    result.negative = *cursor == '-';
    ++cursor;
  }

  AimAndroidWchar current = *cursor++;
  if ((base == 0 || base == 16) && current == '0' &&
      (*cursor == 'x' || *cursor == 'X') &&
      DigitValue(cursor[1]) >= 0 && DigitValue(cursor[1]) < 16) {
    current = cursor[1];
    cursor += 2;
    base = 16;
  }
  if ((base == 0 || base == 2) && current == '0' &&
      (*cursor == 'b' || *cursor == 'B') && IsDigit(cursor[1])) {
    current = cursor[1];
    cursor += 2;
    base = 2;
  }
  if (base == 0) base = current == '0' ? 8 : 10;

  for (;;) {
    const int digit = DigitValue(current);
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
    current = *cursor++;
  }
  result.end = result.converted ? cursor - 1 : input;
  return result;
}

static int64_t ParseSigned(const AimAndroidWchar* input,
                           AimAndroidWchar** end_pointer,
                           int base) {
  // Bionic rejects the base before touching the input buffer.
  if (base < 0 || base == 1 || base > 36) {
    if (end_pointer != NULL) *end_pointer = (AimAndroidWchar*)input;
    aim_bionic_errno_store(kAndroidEinval);
    return 0;
  }
  const AimAndroidWchar* scan = input;
  while (IsSpace(*scan)) ++scan;
  const uint64_t limit = *scan == '-' ? UINT64_C(0x8000000000000000)
                                      : UINT64_C(0x7fffffffffffffff);
  const ParseResult parsed = ParseMagnitude(input, base, limit);
  if (end_pointer != NULL)
    *end_pointer = (AimAndroidWchar*)parsed.end;
  if (parsed.invalid_base) {
    aim_bionic_errno_store(kAndroidEinval);
    return 0;
  }
  if (parsed.overflow) {
    aim_bionic_errno_store(kAndroidErange);
    return parsed.negative ? INT64_MIN : INT64_MAX;
  }
  if (parsed.negative) {
    if (parsed.magnitude == UINT64_C(0x8000000000000000)) return INT64_MIN;
    return -(int64_t)parsed.magnitude;
  }
  return (int64_t)parsed.magnitude;
}

static uint64_t ParseUnsigned(const AimAndroidWchar* input,
                              AimAndroidWchar** end_pointer,
                              int base) {
  const ParseResult parsed = ParseMagnitude(input, base, UINT64_MAX);
  if (end_pointer != NULL)
    *end_pointer = (AimAndroidWchar*)parsed.end;
  if (parsed.invalid_base) {
    aim_bionic_errno_store(kAndroidEinval);
    return 0;
  }
  if (parsed.overflow) {
    aim_bionic_errno_store(kAndroidErange);
    return UINT64_MAX;
  }
  return parsed.negative ? UINT64_C(0) - parsed.magnitude : parsed.magnitude;
}

long aim_bionic_wcstol(const AimAndroidWchar* input,
                              AimAndroidWchar** end_pointer,
                              int base) {
  const HostState state = SaveHostState();
  const long result = (long)ParseSigned(input, end_pointer, base);
  RestoreHostState(&state);
  return result;
}

long long aim_bionic_wcstoll(const AimAndroidWchar* input,
                                    AimAndroidWchar** end_pointer,
                                    int base) {
  const HostState state = SaveHostState();
  const long long result = (long long)ParseSigned(input, end_pointer, base);
  RestoreHostState(&state);
  return result;
}

unsigned long aim_bionic_wcstoul(
    const AimAndroidWchar* input,
    AimAndroidWchar** end_pointer,
    int base) {
  const HostState state = SaveHostState();
  const unsigned long result =
      (unsigned long)ParseUnsigned(input, end_pointer, base);
  RestoreHostState(&state);
  return result;
}

unsigned long long aim_bionic_wcstoull(
    const AimAndroidWchar* input,
    AimAndroidWchar** end_pointer,
    int base) {
  const HostState state = SaveHostState();
  const unsigned long long result =
      (unsigned long long)ParseUnsigned(input, end_pointer, base);
  RestoreHostState(&state);
  return result;
}

void* aim_bionic_wide_integer_resolve(const char* soname,
                                              const char* symbol,
                                              const char* version) {
  if (soname == NULL || symbol == NULL || version == NULL ||
      strcmp(soname, "libc.so") != 0 || strcmp(version, "LIBC") != 0)
    return NULL;
#define RESOLVE(name)                                      \
  if (strcmp(symbol, #name) == 0)                          \
    return (void*)(uintptr_t)&aim_bionic_##name
  RESOLVE(wcstol);
  RESOLVE(wcstoll);
  RESOLVE(wcstoul);
  RESOLVE(wcstoull);
#undef RESOLVE
  return NULL;
}

int aim_bionic_wide_integer_capability(const char* capability) {
  if (capability == NULL) return 0;
  return strcmp(capability, "Android-wchar32") == 0 ||
         strcmp(capability, "AOSP-wide-integer") == 0 ||
         strcmp(capability, "base-0,2..36") == 0;
}

void aim_bionic_wide_integer_test_prepare_host_state(void) {
  errno = 31871;
  fesetround(FE_UPWARD);
  feraiseexcept(FE_DIVBYZERO);
}

int aim_bionic_wide_integer_test_host_state_is_preserved(void) {
  return errno == 31871 && fegetround() == FE_UPWARD &&
         fetestexcept(FE_ALL_EXCEPT) == FE_DIVBYZERO;
}
