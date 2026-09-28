#include <errno.h>
#include <limits.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static int Ordered(const struct timespec* left, const struct timespec* right) {
  return right->tv_sec > left->tv_sec ||
         (right->tv_sec == left->tv_sec && right->tv_nsec >= left->tv_nsec);
}

static long long DifferenceNanoseconds(const struct timespec* start,
                                       const struct timespec* end) {
  return (end->tv_sec - start->tv_sec) * 1000000000LL +
         (end->tv_nsec - start->tv_nsec);
}

__attribute__((visibility("default"))) int bionic_time_fixture_basic(void) {
  struct timespec monotonic_start;
  struct timespec monotonic_end;
  struct timespec realtime;
  struct timespec boottime_start;
  struct timespec boottime_end;
  struct timespec cpu;
  if (clock_gettime(CLOCK_MONOTONIC, &monotonic_start) != 0) return 1;
  const struct timespec request = {0, 30000000};
  if (nanosleep(&request, NULL) != 0) return 2;
  if (clock_gettime(CLOCK_MONOTONIC, &monotonic_end) != 0 ||
      !Ordered(&monotonic_start, &monotonic_end)) return 3;
  const long long elapsed =
      DifferenceNanoseconds(&monotonic_start, &monotonic_end);
  if (elapsed < 10000000LL || elapsed > 2000000000LL) return 4;

  if (clock_gettime(CLOCK_REALTIME, &realtime) != 0 ||
      realtime.tv_sec < 1600000000LL || realtime.tv_sec > 4102444800LL ||
      realtime.tv_nsec < 0 || realtime.tv_nsec >= 1000000000L) return 5;
  if (clock_gettime(CLOCK_BOOTTIME, &boottime_start) != 0 ||
      clock_gettime(CLOCK_BOOTTIME, &boottime_end) != 0 ||
      !Ordered(&boottime_start, &boottime_end)) return 6;
  if (clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &cpu) != 0 || cpu.tv_sec < 0 ||
      cpu.tv_nsec < 0 || cpu.tv_nsec >= 1000000000L) return 7;
  if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &cpu) != 0 || cpu.tv_sec < 0 ||
      cpu.tv_nsec < 0 || cpu.tv_nsec >= 1000000000L) return 8;

  if (clock_gettime(CLOCK_MONOTONIC_RAW, &cpu) != 0 || cpu.tv_sec < 0 ||
      cpu.tv_nsec < 0 || cpu.tv_nsec >= 1000000000L)
    return 9;
  const struct timespec invalid = {-1, 0};
  errno = 0;
  if (nanosleep(&invalid, NULL) != -1 || errno != EINVAL) return 10;

  const long page = sysconf(_SC_PAGESIZE);
  const long page_alias = sysconf(_SC_PAGE_SIZE);
  const long configured = sysconf(_SC_NPROCESSORS_CONF);
  const long online = sysconf(_SC_NPROCESSORS_ONLN);
  const long physical_pages = sysconf(_SC_PHYS_PAGES);
  const long available_pages = sysconf(_SC_AVPHYS_PAGES);
  if (page < 4096 || (page & (page - 1)) != 0 || page_alias != page) return 11;
  if (configured < 1 || online < 1 || online > configured) return 12;
  if (physical_pages <= 0 || available_pages <= 0 ||
      (unsigned long long)physical_pages * (unsigned long long)page !=
          8ULL * 1024ULL * 1024ULL * 1024ULL ||
      (unsigned long long)available_pages * (unsigned long long)page !=
          4ULL * 1024ULL * 1024ULL * 1024ULL)
    return 15;
  errno = 0;
  if (sysconf(0x7fffffff) != -1 || errno != EINVAL) return 13;
  struct timeval wall = {.tv_sec = -1, .tv_usec = -1};
  if (gettimeofday(&wall, NULL) != 0 || wall.tv_sec < 1600000000LL ||
      wall.tv_sec > 4102444800LL || wall.tv_usec < 0 ||
      wall.tv_usec >= 1000000L || wall.tv_sec < realtime.tv_sec - 2 ||
      wall.tv_sec > realtime.tv_sec + 2)
    return 14;
  return 42;
}

__attribute__((visibility("default"))) int bionic_time_fixture_page_size(void) {
  const long page = sysconf(_SC_PAGESIZE);
  return page > 0 && page <= INT_MAX ? (int)page : -1;
}

__attribute__((visibility("default"))) int
bionic_time_fixture_configured_processors(void) {
  const long count = sysconf(_SC_NPROCESSORS_CONF);
  return count > 0 && count <= INT_MAX ? (int)count : -1;
}

__attribute__((visibility("default"))) int
bionic_time_fixture_online_processors(void) {
  const long count = sysconf(_SC_NPROCESSORS_ONLN);
  return count > 0 && count <= INT_MAX ? (int)count : -1;
}

__attribute__((visibility("default"))) int
bionic_time_fixture_interrupted(void) {
  const struct timespec request = {1, 0};
  struct timespec remaining = {-1, -1};
  errno = 0;
  if (nanosleep(&request, &remaining) != -1 || errno != EINTR) return 20;
  if (remaining.tv_sec < 0 || remaining.tv_sec > 1 || remaining.tv_nsec < 0 ||
      remaining.tv_nsec >= 1000000000L) return 21;
  const long long remaining_ns =
      remaining.tv_sec * 1000000000LL + remaining.tv_nsec;
  if (remaining_ns < 100000000LL || remaining_ns >= 1000000000LL) return 22;
  return 42;
}
