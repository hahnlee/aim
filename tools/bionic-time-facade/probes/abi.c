#include <stddef.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

_Static_assert(sizeof(struct timespec) == 16, "Android arm64 timespec size drift");
_Static_assert(offsetof(struct timespec, tv_sec) == 0, "timespec sec offset drift");
_Static_assert(offsetof(struct timespec, tv_nsec) == 8,
               "timespec nsec offset drift");
_Static_assert(sizeof(struct timeval) == 16,
               "Android arm64 timeval size drift");
_Static_assert(offsetof(struct timeval, tv_usec) == 8,
               "timeval usec offset drift");
_Static_assert(sizeof(((struct timeval*)0)->tv_usec) == 8,
               "Android arm64 timeval usec width drift");
_Static_assert(CLOCK_REALTIME == 0 && CLOCK_MONOTONIC == 1 &&
                   CLOCK_PROCESS_CPUTIME_ID == 2 &&
                   CLOCK_THREAD_CPUTIME_ID == 3 && CLOCK_BOOTTIME == 7,
               "Android clock identifier drift");
_Static_assert(_SC_PAGESIZE == 39 && _SC_PAGE_SIZE == 40 &&
                   _SC_NPROCESSORS_CONF == 96 && _SC_NPROCESSORS_ONLN == 97,
               "Android sysconf identifier drift");

typedef int (*ClockGettimeSignature)(clockid_t, struct timespec*);
typedef int (*NanosleepSignature)(const struct timespec*, struct timespec*);
typedef int (*GettimeofdaySignature)(struct timeval*, struct timezone*);
typedef long (*SysconfSignature)(int);
_Static_assert(_Generic(&clock_gettime, ClockGettimeSignature: 1, default: 0),
               "clock_gettime signature drift");
_Static_assert(_Generic(&nanosleep, NanosleepSignature: 1, default: 0),
               "nanosleep signature drift");
_Static_assert(_Generic(&gettimeofday, GettimeofdaySignature: 1, default: 0),
               "gettimeofday signature drift");
_Static_assert(_Generic(&sysconf, SysconfSignature: 1, default: 0),
               "sysconf signature drift");
