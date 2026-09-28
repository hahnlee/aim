#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

extern int __cxa_atexit(void (*function)(void*), void* argument, void* dso);
extern void __cxa_finalize(void* dso);

static unsigned char gDsoStorage;
static unsigned char gOtherDsoStorage;
static uintptr_t gArguments[16];
static _Atomic unsigned gLogCount;
static unsigned gLog[256];
static _Atomic unsigned gErrors;
static uintptr_t gConcurrentArguments[64];
static _Atomic uint64_t gConcurrentBits;
static _Atomic unsigned gConcurrentCalls;
static _Atomic unsigned gBlockingEntered;
static _Atomic unsigned gBlockingRelease;

static void* MainDso(void) { return &gDsoStorage; }
static void* OtherDso(void) { return &gOtherDsoStorage; }

static void Record(unsigned value) {
  const unsigned index = atomic_fetch_add_explicit(
      &gLogCount, 1, memory_order_relaxed);
  if (index < sizeof(gLog) / sizeof(gLog[0])) gLog[index] = value;
}

#define DEFINE_CHECKED_CALLBACK(number)                                      \
  static void Callback##number(void* argument) {                             \
    if (argument != &gArguments[number])                                     \
      atomic_fetch_add_explicit(&gErrors, 1, memory_order_relaxed);           \
    Record(number);                                                          \
  }

DEFINE_CHECKED_CALLBACK(1)
DEFINE_CHECKED_CALLBACK(2)
DEFINE_CHECKED_CALLBACK(3)
DEFINE_CHECKED_CALLBACK(4)
DEFINE_CHECKED_CALLBACK(5)
DEFINE_CHECKED_CALLBACK(6)
DEFINE_CHECKED_CALLBACK(8)

static void ReentrantOuter(void* argument) {
  if (argument != &gArguments[7])
    atomic_fetch_add_explicit(&gErrors, 1, memory_order_relaxed);
  Record(7);
  if (__cxa_atexit(Callback8, &gArguments[8], MainDso()) != 0)
    atomic_fetch_add_explicit(&gErrors, 1, memory_order_relaxed);
  __cxa_finalize(MainDso());
}

static void ConcurrentCallback(void* argument) {
  const uintptr_t* object = (const uintptr_t*)argument;
  const ptrdiff_t index = object - gConcurrentArguments;
  if (index < 0 || index >= 64) {
    atomic_fetch_add_explicit(&gErrors, 1, memory_order_relaxed);
    return;
  }
  atomic_fetch_or_explicit(&gConcurrentBits, UINT64_C(1) << (unsigned)index,
                           memory_order_relaxed);
  atomic_fetch_add_explicit(&gConcurrentCalls, 1, memory_order_relaxed);
}

static void BlockingCallback(void* argument) {
  if (argument != &gArguments[9])
    atomic_fetch_add_explicit(&gErrors, 1, memory_order_relaxed);
  atomic_store_explicit(&gBlockingEntered, 1, memory_order_release);
  while (atomic_load_explicit(&gBlockingRelease, memory_order_acquire) == 0) {
  }
  Record(9);
}

__attribute__((visibility("default"))) void bionic_dso_fixture_reset(void) {
  atomic_store_explicit(&gLogCount, 0, memory_order_relaxed);
  atomic_store_explicit(&gErrors, 0, memory_order_relaxed);
  atomic_store_explicit(&gConcurrentBits, 0, memory_order_relaxed);
  atomic_store_explicit(&gConcurrentCalls, 0, memory_order_relaxed);
  atomic_store_explicit(&gBlockingEntered, 0, memory_order_relaxed);
  atomic_store_explicit(&gBlockingRelease, 0, memory_order_relaxed);
  for (unsigned index = 0; index < sizeof(gLog) / sizeof(gLog[0]); ++index)
    gLog[index] = 0;
}

__attribute__((visibility("default"))) uintptr_t
bionic_dso_fixture_main_handle(void) {
  return (uintptr_t)MainDso();
}

__attribute__((visibility("default"))) uintptr_t
bionic_dso_fixture_other_handle(void) {
  return (uintptr_t)OtherDso();
}

__attribute__((visibility("default"))) uintptr_t
bionic_dso_fixture_callback(unsigned index) {
  switch (index) {
    case 1: return (uintptr_t)Callback1;
    case 2: return (uintptr_t)Callback2;
    case 3: return (uintptr_t)Callback3;
    default: return 0;
  }
}

__attribute__((visibility("default"))) uintptr_t
bionic_dso_fixture_argument(unsigned index) {
  return index < 16 ? (uintptr_t)&gArguments[index] : 0;
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_triples(void) {
  if (__cxa_atexit(Callback1, &gArguments[1], MainDso()) != 0) return 1;
  if (__cxa_atexit(Callback2, &gArguments[2], MainDso()) != 0) return 2;
  if (__cxa_atexit(Callback3, &gArguments[3], MainDso()) != 0) return 3;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_global_set(void) {
  if (__cxa_atexit(Callback4, &gArguments[4], MainDso()) != 0) return 4;
  if (__cxa_atexit(Callback5, &gArguments[5], OtherDso()) != 0) return 5;
  if (__cxa_atexit(Callback6, &gArguments[6], NULL) != 0) return 6;
  return 42;
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_reentrant(void) {
  return __cxa_atexit(ReentrantOuter, &gArguments[7], MainDso()) == 0 ? 42 : 7;
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_concurrent(unsigned index) {
  if (index >= 64) return 8;
  gConcurrentArguments[index] = index;
  return __cxa_atexit(ConcurrentCallback, &gConcurrentArguments[index],
                      MainDso());
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_blocking(void) {
  return __cxa_atexit(BlockingCallback, &gArguments[9], MainDso()) == 0 ? 42 : 9;
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_after_unpublish(void) {
  return __cxa_atexit(Callback1, &gArguments[1], MainDso());
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_register_null_callback(void) {
  return __cxa_atexit(NULL, NULL, MainDso());
}

__attribute__((visibility("default"))) void
bionic_dso_fixture_finalize_main(void) {
  __cxa_finalize(MainDso());
}

__attribute__((visibility("default"))) void
bionic_dso_fixture_finalize_global(void) {
  __cxa_finalize(NULL);
}

__attribute__((visibility("default"))) unsigned
bionic_dso_fixture_log_count(void) {
  return atomic_load_explicit(&gLogCount, memory_order_acquire);
}

__attribute__((visibility("default"))) unsigned
bionic_dso_fixture_log_at(unsigned index) {
  return index < atomic_load_explicit(&gLogCount, memory_order_acquire)
             ? gLog[index]
             : 0;
}

__attribute__((visibility("default"))) unsigned
bionic_dso_fixture_errors(void) {
  return atomic_load_explicit(&gErrors, memory_order_acquire);
}

__attribute__((visibility("default"))) int
bionic_dso_fixture_concurrent_complete(void) {
  return atomic_load_explicit(&gConcurrentBits, memory_order_acquire) ==
                 UINT64_MAX &&
             atomic_load_explicit(&gConcurrentCalls, memory_order_acquire) == 64
         ? 42
         : 10;
}

__attribute__((visibility("default"))) unsigned
bionic_dso_fixture_blocking_entered(void) {
  return atomic_load_explicit(&gBlockingEntered, memory_order_acquire);
}

__attribute__((visibility("default"))) void
bionic_dso_fixture_release_blocking(void) {
  atomic_store_explicit(&gBlockingRelease, 1, memory_order_release);
}
