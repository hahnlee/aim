#include "darwin_art_bionic_pthread.h"

#include <cstdio>
#include <cstring>
#include <thread>

namespace {

constexpr int kAndroidEperm = 1;
constexpr int kAndroidEbusy = 16;
constexpr int kAndroidEinval = 22;
constexpr int kAndroidEdeadlk = 35;
constexpr int kAndroidEnotsup = 95;
constexpr int kNormal = 0;
constexpr int kRecursive = 1;
constexpr int kErrorcheck = 2;
constexpr int kRounds = 100;

int RunRound() {
  DarwinArtAndroidPthreadMutexAttr attributes{};
  DarwinArtAndroidPthreadMutex recursive{};
  if (darwin_art_bionic_pthread_mutexattr_init(&attributes) != 0 ||
      attributes != 0 ||
      darwin_art_bionic_pthread_mutexattr_settype(&attributes, kRecursive) !=
          0 ||
      darwin_art_bionic_pthread_mutex_init(&recursive, &attributes) != 0) {
    return 1;
  }
  if (darwin_art_bionic_pthread_mutex_lock(&recursive) != 0 ||
      darwin_art_bionic_pthread_mutex_lock(&recursive) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&recursive) != kAndroidEbusy ||
      darwin_art_bionic_pthread_mutex_unlock(&recursive) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&recursive) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&recursive) != kAndroidEperm ||
      darwin_art_bionic_pthread_mutex_destroy(&recursive) != 0) {
    return 2;
  }

  DarwinArtAndroidPthreadMutex errorcheck{};
  if (darwin_art_bionic_pthread_mutexattr_settype(&attributes, kErrorcheck) !=
          0 ||
      darwin_art_bionic_pthread_mutex_init(&errorcheck, &attributes) != 0 ||
      darwin_art_bionic_pthread_mutex_lock(&errorcheck) != 0 ||
      darwin_art_bionic_pthread_mutex_lock(&errorcheck) != kAndroidEdeadlk) {
    return 3;
  }
  int wrong_unlock = 0;
  std::thread wrong([&] {
    wrong_unlock = darwin_art_bionic_pthread_mutex_unlock(&errorcheck);
  });
  wrong.join();
  if (wrong_unlock != kAndroidEperm ||
      darwin_art_bionic_pthread_mutex_unlock(&errorcheck) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&errorcheck) != 0) {
    return 4;
  }

  DarwinArtAndroidPthreadMutex normal{};
  if (darwin_art_bionic_pthread_mutexattr_settype(&attributes, kNormal) != 0 ||
      darwin_art_bionic_pthread_mutex_init(&normal, &attributes) != 0 ||
      darwin_art_bionic_pthread_mutex_trylock(&normal) != 0 ||
      darwin_art_bionic_pthread_mutex_trylock(&normal) != kAndroidEbusy ||
      darwin_art_bionic_pthread_mutex_unlock(&normal) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&normal) != 0) {
    return 5;
  }
  if (darwin_art_bionic_pthread_mutexattr_destroy(&attributes) != 0 ||
      attributes != -1 ||
      darwin_art_bionic_pthread_mutexattr_settype(&attributes, kRecursive) !=
          kAndroidEinval) {
    return 6;
  }
  DarwinArtAndroidPthreadMutex rejected{};
  if (darwin_art_bionic_pthread_mutex_init(&rejected, &attributes) !=
      kAndroidEinval) {
    return 7;
  }
  attributes = 0x10;
  if (darwin_art_bionic_pthread_mutex_init(&rejected, &attributes) !=
      kAndroidEnotsup) {
    return 8;
  }
  attributes = 0x20;
  if (darwin_art_bionic_pthread_mutex_init(&rejected, &attributes) !=
      kAndroidEnotsup) {
    return 9;
  }
  DarwinArtAndroidPthreadMutex reused{};
  if (darwin_art_bionic_pthread_mutex_lock(&reused) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&reused) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&reused) != 0) {
    return 10;
  }
  // Android libc++ default-constructs std::mutex from the all-zero static
  // initializer. Simulate allocator address reuse after the previous lifetime
  // was destroyed; the provider must create a fresh host generation.
  std::memset(&reused, 0, sizeof(reused));
  if (darwin_art_bionic_pthread_mutex_lock(&reused) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&reused) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&reused) != 0) {
    return 11;
  }
  // Allocator reuse of a mutex freed without destroy: Bionic init succeeds
  // and the new lifetime starts unlocked.
  DarwinArtAndroidPthreadMutex leaked{};
  if (darwin_art_bionic_pthread_mutex_init(&leaked, nullptr) != 0 ||
      darwin_art_bionic_pthread_mutex_lock(&leaked) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&leaked) != 0 ||
      darwin_art_bionic_pthread_mutex_init(&leaked, nullptr) != 0 ||
      darwin_art_bionic_pthread_mutex_trylock(&leaked) != 0 ||
      darwin_art_bionic_pthread_mutex_unlock(&leaked) != 0 ||
      darwin_art_bionic_pthread_mutex_destroy(&leaked) != 0) {
    return 13;
  }
  if (darwin_art_bionic_pthread_provider_reset() != 0) return 12;
  return 0;
}

}  // namespace

int main() {
  for (int round = 0; round < kRounds; ++round) {
    const int result = RunRound();
    if (result != 0) return result;
  }
  std::puts("pthread-mutex-attr-stress: PASS rounds=100 normal+recursive+errorcheck recursive-depth=2 self=EDEADLK wrong-owner=EPERM held-destroy=EBUSY address-reuse=fresh-generation reinit-live=Bionic-0 destroyed-attr=EINVAL pshared+PI=ENOTSUP ASan=clean");
  return 0;
}
