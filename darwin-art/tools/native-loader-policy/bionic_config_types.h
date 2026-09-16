#pragma once
#include <cstdint>
#include <sys/types.h>
#include <limits.h>
// These imported units manipulate Android paths, not Darwin pathnames.
#undef PATH_MAX
#define PATH_MAX 4096
#include <cerrno>
#include <cassert>
#ifndef TEMP_FAILURE_RETRY
#define TEMP_FAILURE_RETRY(exp) ({ decltype(exp) rc; do { rc = (exp); } while (rc == -1 && errno == EINTR); rc; })
#endif
#undef __assert
#define __assert(file, line, expression) __assert_rtn("", file, line, expression)
using off64_t = int64_t;
static_assert(sizeof(off_t) == sizeof(off64_t));
#ifndef __LIBC_HIDDEN__
#define __LIBC_HIDDEN__ __attribute__((visibility("hidden")))
#endif
#ifndef __printflike
#define __printflike(a, b) __attribute__((format(printf, a, b)))
#endif
