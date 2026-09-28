#ifndef AIM_BIONIC_TIME_H_
#define AIM_BIONIC_TIME_H_

#include <stdint.h>
#include <sys/time.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimAndroidTimespec {
  int64_t tv_sec;
  int64_t tv_nsec;
} AimAndroidTimespec;

typedef struct AimAndroidTimeval {
  int64_t tv_sec;
  int64_t tv_usec;
} AimAndroidTimeval;

typedef void (*AimBionicTimeFunction)(void);

int aim_bionic_clock_gettime(int android_clock,
                                    AimAndroidTimespec* result);
int aim_bionic_clock_getres(int android_clock,
                                   AimAndroidTimespec* result);
int aim_bionic_nanosleep(const AimAndroidTimespec* request,
                                AimAndroidTimespec* remaining);
int aim_bionic_gettimeofday(AimAndroidTimeval* result,
                                   void* timezone);
int64_t aim_bionic_time(int64_t* output);
char* aim_bionic_ctime(const int64_t* value);
struct tm* aim_bionic_gmtime(const int64_t* value);
struct tm* aim_bionic_gmtime_r(const int64_t* value,
                                      struct tm* output);
struct tm* aim_bionic_localtime(const int64_t* value);
struct tm* aim_bionic_localtime_r(const int64_t* value,
                                         struct tm* output);
int64_t aim_bionic_mktime(struct tm* value);
long aim_bionic_sysconf(int android_name);
AimBionicTimeFunction aim_bionic_time_resolve(
    const char* import_name);
uintptr_t aim_bionic_time_data_resolve(const char* import_name);
int aim_bionic_time_capability_failed(void);

/* Test-harness-only signal controls; hidden from dynamic and guest resolution. */
__attribute__((visibility("hidden"))) int
aim_bionic_time_test_arm_alarm(uint32_t microseconds);
__attribute__((visibility("hidden"))) void
aim_bionic_time_test_finish_alarm(void);
__attribute__((visibility("hidden"))) int
aim_bionic_time_test_force_boottime_overflow(void);

#ifdef __cplusplus
}
#endif

#endif
