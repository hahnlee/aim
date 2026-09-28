// Minimal checks for the syscall-layer NDK tests (run under linux-run with
// the image's original bionic). Each test prints "ok <name>", "skip ..." or
// "FAIL <name>: <what> (errno)", and the program exits nonzero on failure.
#pragma once
#include <errno.h>
#include <stdio.h>
#include <string.h>

static int failures;
static int skipped;

#define CHECK(cond)                                                             \
  do {                                                                          \
    if (!(cond)) {                                                              \
      printf("FAIL %s:%d: %s (errno %d %s)\n", __func__, __LINE__, #cond, errno, \
             strerror(errno));                                                  \
      failures++;                                                               \
      return;                                                                   \
    }                                                                           \
  } while (0)

#define RUN(t)                            \
  do {                                    \
    int before = failures;                \
    skipped = 0;                          \
    t();                                  \
    if (failures == before && !skipped) { \
      printf("ok %s\n", #t);              \
    }                                     \
    fflush(stdout);                       \
  } while (0)

// fork(), or skip the test where the layer cannot fork yet.
#define FORK_OR_SKIP(p)                                \
  pid_t p = fork();                                    \
  do {                                                 \
    if (p < 0 && errno == ENOSYS) {                    \
      printf("skip %s: fork unavailable\n", __func__); \
      skipped = 1;                                     \
      return;                                          \
    }                                                  \
    CHECK(p >= 0);                                     \
  } while (0)

#define DONE()                                             \
  do {                                                     \
    printf(failures ? "FAILED %d\n" : "PASS\n", failures); \
    return failures ? 1 : 0;                               \
  } while (0)
