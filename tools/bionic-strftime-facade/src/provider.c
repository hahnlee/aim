#include "aim_bionic_strftime.h"

#include <errno.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum {
  kAndroidEio = 5,
  kAndroidEfault = 14,
  kAndroidEnosys = 38,
  kAndroidErange = 34,
  kAndroidEoverflow = 75,
  kAndroidEopnotsupp = 95,
  kMaxZoneName = 31,
  kMaxOffset = 24 * 60 * 60,
};

extern void aim_bionic_errno_store(int value);
extern size_t aim_bionic_strftime_upstream_l(
    char* restrict destination, size_t capacity,
    const char* restrict format, const struct tm* restrict broken_down,
    void* locale);

_Static_assert(sizeof(AimAndroidTm) == sizeof(struct tm),
               "Android/Darwin struct tm size drift");
#define CHECK_TM_FIELD(field)                                                \
  _Static_assert(offsetof(AimAndroidTm, field) ==                     \
                     offsetof(struct tm, field),                            \
                 "Android/Darwin struct tm field drift: " #field)
CHECK_TM_FIELD(tm_sec);
CHECK_TM_FIELD(tm_min);
CHECK_TM_FIELD(tm_hour);
CHECK_TM_FIELD(tm_mday);
CHECK_TM_FIELD(tm_mon);
CHECK_TM_FIELD(tm_year);
CHECK_TM_FIELD(tm_wday);
CHECK_TM_FIELD(tm_yday);
CHECK_TM_FIELD(tm_isdst);
CHECK_TM_FIELD(tm_gmtoff);
CHECK_TM_FIELD(tm_zone);
#undef CHECK_TM_FIELD

typedef struct TimezoneState {
  pthread_mutex_t mutex;
  pthread_cond_t quiescent;
  bool active;
  bool draining;
  size_t in_flight;
  int32_t standard_offset;
  int32_t daylight_offset;
  char standard_name[kMaxZoneName + 1];
  char daylight_name[kMaxZoneName + 1];
} TimezoneState;

static TimezoneState g_timezone = {
    PTHREAD_MUTEX_INITIALIZER,
    PTHREAD_COND_INITIALIZER,
    false,
    false,
    0,
    0,
    0,
    {0},
    {0},
};

char* aim_bionic_strftime_tzname[2] = {
    g_timezone.standard_name,
    g_timezone.daylight_name,
};

static _Thread_local bool g_core_capability_failure;

static bool PortableZoneName(const char* name) {
  if (name == NULL || name[0] == '\0') return false;
  size_t length = 0;
  for (; name[length] != '\0'; ++length) {
    const unsigned char value = (unsigned char)name[length];
    const bool valid = (value >= 'A' && value <= 'Z') ||
                       (value >= 'a' && value <= 'z') ||
                       (value >= '0' && value <= '9') || value == '+' ||
                       value == '-';
    if (!valid || length == kMaxZoneName) return false;
  }
  return true;
}

static bool AcquireTimezone(void) {
  (void)pthread_mutex_lock(&g_timezone.mutex);
  if (!g_timezone.active || g_timezone.draining ||
      g_timezone.in_flight == SIZE_MAX) {
    (void)pthread_mutex_unlock(&g_timezone.mutex);
    return false;
  }
  ++g_timezone.in_flight;
  (void)pthread_mutex_unlock(&g_timezone.mutex);
  return true;
}

static void ReleaseTimezone(void) {
  (void)pthread_mutex_lock(&g_timezone.mutex);
  if (g_timezone.in_flight == 0) abort();
  --g_timezone.in_flight;
  if (g_timezone.in_flight == 0) {
    (void)pthread_cond_broadcast(&g_timezone.quiescent);
  }
  (void)pthread_mutex_unlock(&g_timezone.mutex);
}

AimBionicStrftimeLifecycleStatus aim_bionic_strftime_activate(
    const char* standard_name, int32_t standard_offset,
    const char* daylight_name, int32_t daylight_offset) {
  const int saved_errno = errno;
  AimBionicStrftimeLifecycleStatus result =
      AIM_BIONIC_STRFTIME_OK;
  if (!PortableZoneName(standard_name) || !PortableZoneName(daylight_name) ||
      standard_offset < -kMaxOffset || standard_offset > kMaxOffset ||
      daylight_offset < -kMaxOffset || daylight_offset > kMaxOffset) {
    errno = saved_errno;
    return AIM_BIONIC_STRFTIME_INVALID_ARGUMENT;
  }
  (void)pthread_mutex_lock(&g_timezone.mutex);
  if (g_timezone.active || g_timezone.draining) {
    result = AIM_BIONIC_STRFTIME_ALREADY_ACTIVE;
  } else {
    (void)strcpy(g_timezone.standard_name, standard_name);
    (void)strcpy(g_timezone.daylight_name, daylight_name);
    g_timezone.standard_offset = standard_offset;
    g_timezone.daylight_offset = daylight_offset;
    g_timezone.active = true;
  }
  (void)pthread_mutex_unlock(&g_timezone.mutex);
  errno = saved_errno;
  return result;
}

AimBionicStrftimeLifecycleStatus
aim_bionic_strftime_deactivate(void) {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&g_timezone.mutex);
  if (!g_timezone.active || g_timezone.draining) {
    (void)pthread_mutex_unlock(&g_timezone.mutex);
    errno = saved_errno;
    return AIM_BIONIC_STRFTIME_NOT_ACTIVE;
  }
  g_timezone.draining = true;
  while (g_timezone.in_flight != 0) {
    (void)pthread_cond_wait(&g_timezone.quiescent, &g_timezone.mutex);
  }
  g_timezone.active = false;
  g_timezone.draining = false;
  g_timezone.standard_offset = 0;
  g_timezone.daylight_offset = 0;
  memset(g_timezone.standard_name, 0, sizeof(g_timezone.standard_name));
  memset(g_timezone.daylight_name, 0, sizeof(g_timezone.daylight_name));
  (void)pthread_mutex_unlock(&g_timezone.mutex);
  errno = saved_errno;
  return AIM_BIONIC_STRFTIME_OK;
}

void aim_bionic_strftime_tzset(void) {
  /* The immutable timezone owner is installed explicitly. */
}

static int64_t FloorDiv(int64_t value, int64_t divisor) {
  int64_t quotient = value / divisor;
  const int64_t remainder = value % divisor;
  if (remainder < 0) --quotient;
  return quotient;
}

/* Proleptic Gregorian days since 1970-01-01, valid for the full int tm_year
 * domain. This normalizes out-of-range month/day fields before epoch math. */
static int64_t DaysFromCivil(int64_t year, unsigned month, unsigned day) {
  year -= month <= 2;
  const int64_t era = FloorDiv(year, 400);
  const unsigned year_of_era = (unsigned)(year - era * 400);
  const unsigned adjusted_month = month > 2 ? month - 3 : month + 9;
  const unsigned day_of_year =
      (153 * adjusted_month + 2) / 5 + day - 1;
  const unsigned day_of_era =
      year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  return era * 146097 + (int64_t)day_of_era - 719468;
}

time_t aim_bionic_strftime_mktime(struct tm* value) {
  if (value == NULL) {
    g_core_capability_failure = true;
    return (time_t)-1;
  }
  int32_t standard_offset;
  int32_t daylight_offset;
  (void)pthread_mutex_lock(&g_timezone.mutex);
  standard_offset = g_timezone.standard_offset;
  daylight_offset = g_timezone.daylight_offset;
  (void)pthread_mutex_unlock(&g_timezone.mutex);
  if (value->tm_isdst < 0 && standard_offset != daylight_offset) {
    /* A transition database is required to infer DST from civil fields. */
    g_core_capability_failure = true;
    return (time_t)-1;
  }
  const int32_t offset = value->tm_isdst > 0 ? daylight_offset : standard_offset;
  int64_t year = (int64_t)value->tm_year + 1900;
  const int64_t month_quotient = FloorDiv(value->tm_mon, 12);
  year += month_quotient;
  const unsigned month = (unsigned)(value->tm_mon - month_quotient * 12) + 1;
  /* Every struct tm arithmetic input is a 32-bit int. Even at its extrema,
   * this expression remains inside int64_t for the complete tm_year domain. */
  const int64_t seconds = DaysFromCivil(year, month, 1) * 86400 +
                          ((int64_t)value->tm_mday - 1) * 86400 +
                          (int64_t)value->tm_hour * 3600 +
                          (int64_t)value->tm_min * 60 + value->tm_sec - offset;
  return (time_t)seconds;
}

int aim_bionic_strftime_ascii_tolower(int value) {
  return value >= 'A' && value <= 'Z' ? value + ('a' - 'A') : value;
}

int aim_bionic_strftime_ascii_toupper(int value) {
  return value >= 'a' && value <= 'z' ? value - ('a' - 'A') : value;
}

int aim_bionic_strftime_ascii_islower(int value) {
  return value >= 'a' && value <= 'z';
}

int aim_bionic_strftime_ascii_isupper(int value) {
  return value >= 'A' && value <= 'Z';
}

int aim_bionic_strftime_decimal(char* destination,
                                       const char* format, ...) {
  (void)format;
  va_list arguments;
  va_start(arguments, format);
  const bool is_unsigned = strchr(format, 'u') != NULL;
  const uintmax_t unsigned_value =
      is_unsigned ? va_arg(arguments, uintmax_t) : 0;
  const intmax_t signed_value =
      is_unsigned ? 0 : va_arg(arguments, intmax_t);
  va_end(arguments);
  uintmax_t magnitude = is_unsigned
                            ? unsigned_value
                            : (signed_value < 0
                                   ? (uintmax_t)(-(signed_value + 1)) + 1
                                   : (uintmax_t)signed_value);
  char reverse[64];
  size_t count = 0;
  do {
    reverse[count++] = (char)('0' + magnitude % 10);
    magnitude /= 10;
  } while (magnitude != 0);
  size_t written = 0;
  if (!is_unsigned && signed_value < 0) destination[written++] = '-';
  while (count != 0) destination[written++] = reverse[--count];
  destination[written] = '\0';
  return (int)written;
}

size_t aim_bionic_strftime_l(char* destination, size_t capacity,
                                    const char* format,
                                    const AimAndroidTm* broken_down,
                                    void* locale) {
  const int saved_errno = errno;
  if (destination == NULL || format == NULL || broken_down == NULL) {
    aim_bionic_errno_store(kAndroidEfault);
    errno = saved_errno;
    return 0;
  }
  if (!AcquireTimezone()) {
    aim_bionic_errno_store(kAndroidEnosys);
    errno = saved_errno;
    return 0;
  }
  g_core_capability_failure = false;
  errno = 0;
  const size_t result = aim_bionic_strftime_upstream_l(
      destination, capacity, format, (const struct tm*)broken_down, locale);
  const int core_errno = errno;
  const bool capability_failure = g_core_capability_failure;
  ReleaseTimezone();
  errno = saved_errno;
  if (capability_failure) {
    aim_bionic_errno_store(kAndroidEopnotsupp);
    return 0;
  }
  if (result == 0 && core_errno != 0) {
    if (core_errno == ERANGE) {
      aim_bionic_errno_store(kAndroidErange);
    } else if (core_errno == EOVERFLOW) {
      aim_bionic_errno_store(kAndroidEoverflow);
    } else {
      aim_bionic_errno_store(kAndroidEio);
    }
  }
  return result;
}

size_t aim_bionic_strftime(char* destination, size_t capacity,
                                  const char* format,
                                  const AimAndroidTm* broken_down) {
  return aim_bionic_strftime_l(destination, capacity, format,
                                      broken_down, NULL);
}

char* aim_bionic_strptime(const char* input, const char* format,
                                 AimAndroidTm* broken_down) {
  const int saved_errno = errno;
  char* result = strptime(input, format, (struct tm*)broken_down);
  errno = saved_errno;
  return result;
}

void aim_bionic_tzset(void) {
  /* Timezone state is installed explicitly by the process owner. */
}

AimBionicStrftimeFunction aim_bionic_strftime_resolve(
    const char* soname, const char* symbol, const char* version) {
  const int saved_errno = errno;
  AimBionicStrftimeFunction result = NULL;
  if (soname != NULL && symbol != NULL && version != NULL &&
      strcmp(soname, "libc.so") == 0 && strcmp(version, "LIBC") == 0) {
    if (strcmp(symbol, "strftime") == 0)
      result = (AimBionicStrftimeFunction)aim_bionic_strftime;
    else if (strcmp(symbol, "strftime_l") == 0)
      result = (AimBionicStrftimeFunction)aim_bionic_strftime_l;
    else if (strcmp(symbol, "strptime") == 0)
      result = (AimBionicStrftimeFunction)aim_bionic_strptime;
    else if (strcmp(symbol, "tzset") == 0)
      result = (AimBionicStrftimeFunction)aim_bionic_tzset;
  }
  errno = saved_errno;
  return result;
}
