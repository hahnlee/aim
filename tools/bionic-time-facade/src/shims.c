#include "aim_bionic_time.h"

#include <errno.h>
#include <limits.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

enum {
  ANDROID_CLOCK_REALTIME = 0,
  ANDROID_CLOCK_MONOTONIC = 1,
  ANDROID_CLOCK_PROCESS_CPUTIME_ID = 2,
  ANDROID_CLOCK_THREAD_CPUTIME_ID = 3,
  ANDROID_CLOCK_MONOTONIC_RAW = 4,
  ANDROID_CLOCK_REALTIME_COARSE = 5,
  ANDROID_CLOCK_MONOTONIC_COARSE = 6,
  ANDROID_CLOCK_BOOTTIME = 7,
  ANDROID_EIO = 5,
  ANDROID_EFAULT = 14,
  ANDROID_EINVAL = 22,
  ANDROID_EOVERFLOW = 75,
  ANDROID_SC_PAGESIZE = 39,
  ANDROID_SC_PAGE_SIZE = 40,
  ANDROID_SC_NPROCESSORS_CONF = 96,
  ANDROID_SC_NPROCESSORS_ONLN = 97,
  ANDROID_SC_PHYS_PAGES = 98,
  ANDROID_SC_AVPHYS_PAGES = 99,
};

_Static_assert(sizeof(AimAndroidTimespec) == 16,
               "Android arm64 timespec size drift");
_Static_assert(offsetof(AimAndroidTimespec, tv_nsec) == 8,
               "Android arm64 timespec offset drift");
_Static_assert(sizeof(AimAndroidTimeval) == 16,
               "Android arm64 timeval size drift");
_Static_assert(offsetof(AimAndroidTimeval, tv_usec) == 8,
               "Android arm64 timeval offset drift");
_Static_assert(sizeof(time_t) == 8 && sizeof(long) == 8,
               "Darwin arm64 time ABI drift");
_Static_assert(CLOCK_REALTIME == 0 && CLOCK_MONOTONIC == 6 &&
                   CLOCK_PROCESS_CPUTIME_ID == 12 &&
                   CLOCK_THREAD_CPUTIME_ID == 16,
               "Darwin clock identifiers drift");
_Static_assert(_SC_PAGESIZE == 29 && _SC_NPROCESSORS_CONF == 57 &&
                   _SC_NPROCESSORS_ONLN == 58,
               "Darwin sysconf identifiers drift");

extern int aim_bionic_errno_set_from_darwin(int darwin_errno);
extern void aim_bionic_errno_store(int32_t android_errno);

static _Atomic int gCapabilityFailure;
static pthread_once_t gTimezoneOnce = PTHREAD_ONCE_INIT;
// Keep the host symbol in the sealed provider's dependency audit even though
// supported Android selectors are served from the virtual device snapshot.
static long (*volatile const kHostSysconf)(int) = sysconf;
long aim_bionic_timezone;
int aim_bionic_daylight;
char* aim_bionic_tzname[2];

static void InitializeTimezone(void) {
  tzset();
  aim_bionic_timezone = timezone;
  aim_bionic_daylight = daylight;
  aim_bionic_tzname[0] = tzname[0];
  aim_bionic_tzname[1] = tzname[1];
}

static int FailAndroid(int android_errno) {
  aim_bionic_errno_store(android_errno);
  return -1;
}

static int FailCapability(void) {
  atomic_store_explicit(&gCapabilityFailure, 1, memory_order_release);
  return FailAndroid(ANDROID_EIO);
}

static int FailHost(int host_errno) {
  if (host_errno != 0 && aim_bionic_errno_set_from_darwin(host_errno))
    return -1;
  return FailCapability();
}

static void CopyTimespec(AimAndroidTimespec* destination,
                         const struct timespec* source) {
  destination->tv_sec = source->tv_sec;
  destination->tv_nsec = source->tv_nsec;
}

static int HostClockForAndroid(int android_clock, clockid_t* host_clock) {
  switch (android_clock) {
    case ANDROID_CLOCK_REALTIME:
      *host_clock = CLOCK_REALTIME;
      return 1;
    case ANDROID_CLOCK_MONOTONIC:
    case ANDROID_CLOCK_MONOTONIC_RAW:
    case ANDROID_CLOCK_MONOTONIC_COARSE:
      *host_clock = CLOCK_MONOTONIC;
      return 1;
    case ANDROID_CLOCK_REALTIME_COARSE:
      *host_clock = CLOCK_REALTIME;
      return 1;
    case ANDROID_CLOCK_PROCESS_CPUTIME_ID:
      *host_clock = CLOCK_PROCESS_CPUTIME_ID;
      return 1;
    case ANDROID_CLOCK_THREAD_CPUTIME_ID:
      *host_clock = CLOCK_THREAD_CPUTIME_ID;
      return 1;
    default:
      return 0;
  }
}

static int ConvertContinuousTime(uint64_t ticks, uint32_t numerator,
                                 uint32_t denominator,
                                 AimAndroidTimespec* result) {
  if (denominator == 0) return FailCapability();
  const __uint128_t nanoseconds =
      ((__uint128_t)ticks * numerator) / denominator;
  const __uint128_t seconds = nanoseconds / 1000000000u;
  if (seconds > INT64_MAX) return FailAndroid(ANDROID_EOVERFLOW);
  result->tv_sec = (int64_t)seconds;
  result->tv_nsec = (int64_t)(nanoseconds % 1000000000u);
  return 0;
}

static int ReadBoottime(AimAndroidTimespec* result) {
  mach_timebase_info_data_t timebase;
  if (mach_timebase_info(&timebase) != KERN_SUCCESS) return FailCapability();
  return ConvertContinuousTime(mach_continuous_time(), timebase.numer,
                               timebase.denom, result);
}

int aim_bionic_clock_gettime(int android_clock,
                                    AimAndroidTimespec* result) {
  const int saved_host_errno = errno;
  int outcome;
  if (result == NULL) {
    outcome = FailAndroid(ANDROID_EFAULT);
  } else if (android_clock == ANDROID_CLOCK_BOOTTIME) {
    outcome = ReadBoottime(result);
  } else {
    clockid_t host_clock;
    if (!HostClockForAndroid(android_clock, &host_clock)) {
      outcome = FailAndroid(ANDROID_EINVAL);
    } else {
      struct timespec host_result;
      if (clock_gettime(host_clock, &host_result) == 0) {
        CopyTimespec(result, &host_result);
        outcome = 0;
      } else {
        outcome = FailHost(errno);
      }
    }
  }
  errno = saved_host_errno;
  return outcome;
}

int aim_bionic_clock_getres(int android_clock,
                                   AimAndroidTimespec* result) {
  const int saved_host_errno = errno;
  int outcome;
  if (result == NULL) {
    outcome = FailAndroid(ANDROID_EFAULT);
  } else if (android_clock == ANDROID_CLOCK_BOOTTIME) {
    result->tv_sec = 0;
    result->tv_nsec = 1;
    outcome = 0;
  } else {
    clockid_t host_clock;
    if (!HostClockForAndroid(android_clock, &host_clock)) {
      outcome = FailAndroid(ANDROID_EINVAL);
    } else {
      struct timespec host_result;
      if (clock_getres(host_clock, &host_result) == 0) {
        CopyTimespec(result, &host_result);
        outcome = 0;
      } else {
        outcome = FailHost(errno);
      }
    }
  }
  errno = saved_host_errno;
  return outcome;
}

int aim_bionic_nanosleep(const AimAndroidTimespec* request,
                                AimAndroidTimespec* remaining) {
  const int saved_host_errno = errno;
  int outcome;
  if (request == NULL) {
    outcome = FailAndroid(ANDROID_EFAULT);
  } else if (request->tv_sec < 0 || request->tv_nsec < 0 ||
             request->tv_nsec >= 1000000000) {
    outcome = FailAndroid(ANDROID_EINVAL);
  } else {
    const struct timespec host_request = {
        .tv_sec = (time_t)request->tv_sec,
        .tv_nsec = (long)request->tv_nsec,
    };
    struct timespec host_remaining;
    const int result = nanosleep(
        &host_request, remaining == NULL ? NULL : &host_remaining);
    if (result == 0) {
      outcome = 0;
    } else {
      const int host_error = errno;
      if (host_error == EINTR && remaining != NULL)
        CopyTimespec(remaining, &host_remaining);
      outcome = FailHost(host_error);
    }
  }
  errno = saved_host_errno;
  return outcome;
}

int aim_bionic_gettimeofday(AimAndroidTimeval* result,
                                   void* timezone) {
  const int saved_host_errno = errno;
  struct timeval host_result;
  const int outcome = gettimeofday(result == NULL ? NULL : &host_result,
                                   timezone);
  if (outcome == 0 && result != NULL) {
    result->tv_sec = (int64_t)host_result.tv_sec;
    result->tv_usec = (int64_t)host_result.tv_usec;
  }
  if (outcome != 0) FailHost(errno);
  errno = saved_host_errno;
  return outcome;
}

int64_t aim_bionic_time(int64_t* output) {
  const int saved_host_errno = errno;
  time_t value = time(NULL);
  if (output != NULL) *output = (int64_t)value;
  errno = saved_host_errno;
  return (int64_t)value;
}

char* aim_bionic_ctime(const int64_t* value) {
  if (value == NULL) return NULL;
  const time_t host = (time_t)*value;
  return ctime(&host);
}

struct tm* aim_bionic_gmtime(const int64_t* value) {
  if (value == NULL) return NULL;
  const time_t host = (time_t)*value;
  return gmtime(&host);
}

struct tm* aim_bionic_gmtime_r(const int64_t* value,
                                      struct tm* output) {
  if (value == NULL || output == NULL) return NULL;
  const time_t host = (time_t)*value;
  return gmtime_r(&host, output);
}

struct tm* aim_bionic_localtime(const int64_t* value) {
  if (value == NULL) return NULL;
  const time_t host = (time_t)*value;
  return localtime(&host);
}

struct tm* aim_bionic_localtime_r(const int64_t* value,
                                         struct tm* output) {
  if (value == NULL || output == NULL) return NULL;
  const time_t host = (time_t)*value;
  return localtime_r(&host, output);
}

int64_t aim_bionic_mktime(struct tm* value) {
  return value == NULL ? -1 : (int64_t)mktime(value);
}

int64_t aim_bionic_timegm(struct tm* value) {
  return value == NULL ? -1 : (int64_t)timegm(value);
}

char* aim_bionic_asctime_r(const struct tm* value, char* buffer) {
  return value == NULL || buffer == NULL ? NULL : asctime_r(value, buffer);
}

double aim_bionic_difftime(int64_t left, int64_t right) {
  return difftime((time_t)left, (time_t)right);
}

int64_t aim_bionic_clock(void) { return (int64_t)clock(); }
unsigned aim_bionic_sleep(unsigned seconds) { return sleep(seconds); }
int aim_bionic_usleep(unsigned microseconds) {
  return usleep(microseconds);
}

long aim_bionic_sysconf(int android_name) {
  const int saved_host_errno = errno;
  if (getenv("AIM_DEBUG_SYSCONF") != NULL) {
    fprintf(stderr, "DARWIN sysconf selector=%d caller=%p\n", android_name,
            __builtin_return_address(0));
  }
  (void)kHostSysconf;
  long result;
  switch (android_name) {
    case ANDROID_SC_PAGESIZE:
    case ANDROID_SC_PAGE_SIZE:
      // Android's page-size selectors describe the process's actual mapping
      // granularity. Apple Silicon hosts currently use 16 KiB pages; keeping
      // a 4 KiB answer here makes native engines disagree with mmap/mprotect.
      result = (long)getpagesize();
      if (result <= 0) {
        errno = saved_host_errno;
        return FailCapability();
      }
      break;
    case ANDROID_SC_NPROCESSORS_CONF:
      // Match Java/libcore's host-backed topology. Do not use a process
      // affinity query here: it can describe a virtual guest restriction.
      result = kHostSysconf(_SC_NPROCESSORS_CONF);
      if (result <= 0) {
        errno = saved_host_errno;
        return FailCapability();
      }
      break;
    case ANDROID_SC_NPROCESSORS_ONLN:
      // The online selector has its own Darwin counterpart; on this host it
      // currently agrees with configured CPUs, but preserve POSIX semantics if
      // that changes.
      result = kHostSysconf(_SC_NPROCESSORS_ONLN);
      if (result <= 0) {
        errno = saved_host_errno;
        return FailCapability();
      }
      break;
    case ANDROID_SC_PHYS_PAGES:
      // Preserve the virtual device's fixed 8 GiB physical-memory snapshot,
      // expressed in the host's actual page units.
      result = (long)(8ULL * 1024ULL * 1024ULL * 1024ULL /
                      (unsigned long)getpagesize());
      break;
    case ANDROID_SC_AVPHYS_PAGES:
      // Preserve the corresponding 4 GiB available-memory snapshot.
      result = (long)(4ULL * 1024ULL * 1024ULL * 1024ULL /
                      (unsigned long)getpagesize());
      break;
    default:
      errno = saved_host_errno;
      return FailAndroid(ANDROID_EINVAL);
  }
  if (getenv("AIM_DEBUG_SYSCONF") != NULL) {
    fprintf(stderr, "DARWIN sysconf name=%d result=%ld caller=%p\n",
            android_name, result, __builtin_return_address(0));
  }
  errno = saved_host_errno;
  return result;
}

static int NameCompare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

typedef struct Binding {
  const char* name;
  AimBionicTimeFunction address;
} Binding;

static const Binding kBindings[] = {
    {"asctime_r", (AimBionicTimeFunction)aim_bionic_asctime_r},
    {"clock", (AimBionicTimeFunction)aim_bionic_clock},
    {"clock_getres", (AimBionicTimeFunction)aim_bionic_clock_getres},
    {"clock_gettime", (AimBionicTimeFunction)aim_bionic_clock_gettime},
    {"ctime", (AimBionicTimeFunction)aim_bionic_ctime},
    {"difftime", (AimBionicTimeFunction)aim_bionic_difftime},
    {"gettimeofday", (AimBionicTimeFunction)aim_bionic_gettimeofday},
    {"gmtime", (AimBionicTimeFunction)aim_bionic_gmtime},
    {"gmtime_r", (AimBionicTimeFunction)aim_bionic_gmtime_r},
    {"localtime", (AimBionicTimeFunction)aim_bionic_localtime},
    {"localtime_r", (AimBionicTimeFunction)aim_bionic_localtime_r},
    {"mktime", (AimBionicTimeFunction)aim_bionic_mktime},
    {"nanosleep", (AimBionicTimeFunction)aim_bionic_nanosleep},
    {"sleep", (AimBionicTimeFunction)aim_bionic_sleep},
    {"sysconf", (AimBionicTimeFunction)aim_bionic_sysconf},
    {"time", (AimBionicTimeFunction)aim_bionic_time},
    {"timegm", (AimBionicTimeFunction)aim_bionic_timegm},
    {"usleep", (AimBionicTimeFunction)aim_bionic_usleep},
};

AimBionicTimeFunction aim_bionic_time_resolve(
    const char* import_name) {
  if (import_name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = NameCompare(import_name, kBindings[middle].name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}

uintptr_t aim_bionic_time_data_resolve(const char* import_name) {
  if (import_name == NULL) return 0;
  (void)pthread_once(&gTimezoneOnce, InitializeTimezone);
  if (strcmp(import_name, "daylight") == 0)
    return (uintptr_t)&aim_bionic_daylight;
  if (strcmp(import_name, "timezone") == 0)
    return (uintptr_t)&aim_bionic_timezone;
  if (strcmp(import_name, "tzname") == 0)
    return (uintptr_t)&aim_bionic_tzname;
  return 0;
}

int aim_bionic_time_capability_failed(void) {
  return atomic_load_explicit(&gCapabilityFailure, memory_order_acquire);
}

static struct sigaction gPreviousAlarm;
static int gAlarmInstalled;

static void AlarmHandler(int signal_number) { (void)signal_number; }

int aim_bionic_time_test_arm_alarm(uint32_t microseconds) {
  if (gAlarmInstalled) return -1;
  struct sigaction action = {0};
  action.sa_handler = AlarmHandler;
  if (sigemptyset(&action.sa_mask) != 0 ||
      sigaction(SIGALRM, &action, &gPreviousAlarm) != 0)
    return -1;
  gAlarmInstalled = 1;
  const struct itimerval timer = {
      .it_interval = {0, 0},
      .it_value = {(time_t)(microseconds / 1000000u),
                   (suseconds_t)(microseconds % 1000000u)},
  };
  if (setitimer(ITIMER_REAL, &timer, NULL) == 0) return 0;
  sigaction(SIGALRM, &gPreviousAlarm, NULL);
  gAlarmInstalled = 0;
  return -1;
}

void aim_bionic_time_test_finish_alarm(void) {
  if (!gAlarmInstalled) return;
  const struct itimerval disabled = {0};
  setitimer(ITIMER_REAL, &disabled, NULL);
  sigaction(SIGALRM, &gPreviousAlarm, NULL);
  gAlarmInstalled = 0;
}

int aim_bionic_time_test_force_boottime_overflow(void) {
  AimAndroidTimespec ignored;
  return ConvertContinuousTime(UINT64_MAX, UINT32_MAX, 1, &ignored);
}
