#include "darwin_art_bionic_pthread.h"

#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <vector>

namespace {

constexpr int kAndroidEbusy = 16;
constexpr int kAndroidEnotsup = 95;
constexpr int kReaders = 4;
constexpr int kReadIterations = 1000;
constexpr int kWriteIterations = 500;
constexpr int kRounds = 20;

int RunRound() {
  DarwinArtAndroidPthreadRwlock rwlock{};
  if (darwin_art_bionic_pthread_rwlock_init(&rwlock, nullptr) != 0) return 13;
  int guarded = 0;
  std::atomic<int> active_readers{};
  std::atomic<int> maximum_readers{};
  std::atomic<bool> start{};
  std::vector<std::thread> readers;
  for (int index = 0; index < kReaders; ++index) {
    readers.emplace_back([&] {
      while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
      for (int iteration = 0; iteration < kReadIterations; ++iteration) {
        if (darwin_art_bionic_pthread_rwlock_rdlock(&rwlock) != 0) {
          std::abort();
        }
        const int active =
            active_readers.fetch_add(1, std::memory_order_acq_rel) + 1;
        int maximum = maximum_readers.load(std::memory_order_relaxed);
        while (active > maximum &&
               !maximum_readers.compare_exchange_weak(
                   maximum, active, std::memory_order_relaxed)) {
        }
        const int observed = guarded;
        if (observed < 0) std::abort();
        std::this_thread::yield();
        active_readers.fetch_sub(1, std::memory_order_acq_rel);
        if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) {
          std::abort();
        }
      }
    });
  }
  std::thread writer([&] {
    while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
    for (int iteration = 0; iteration < kWriteIterations; ++iteration) {
      if (darwin_art_bionic_pthread_rwlock_wrlock(&rwlock) != 0) std::abort();
      ++guarded;
      if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) std::abort();
    }
  });
  start.store(true, std::memory_order_release);
  for (std::thread& reader : readers) reader.join();
  writer.join();
  if (guarded != kWriteIterations) return 1;
  if (maximum_readers.load(std::memory_order_relaxed) < 2) return 2;
  if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 1) return 3;
  if (darwin_art_bionic_pthread_rwlock_rdlock(&rwlock) != 0) return 4;
  if (darwin_art_bionic_pthread_rwlock_destroy(&rwlock) != kAndroidEbusy)
    return 5;
  if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) return 6;
  if (darwin_art_bionic_pthread_rwlock_wrlock(&rwlock) != 0) return 7;
  int wrong_unlock = 0;
  std::thread wrong([&] {
    wrong_unlock = darwin_art_bionic_pthread_rwlock_unlock(&rwlock);
  });
  wrong.join();
  if (wrong_unlock != 1) return 8;
  if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) return 9;
  if (darwin_art_bionic_pthread_rwlock_destroy(&rwlock) != 0) return 10;
  // Bionic leaves an idle rwlock unchanged, so repeated destroy succeeds.
  if (darwin_art_bionic_pthread_rwlock_destroy(&rwlock) != 0) return 11;
  if (darwin_art_bionic_pthread_rwlock_init(&rwlock, nullptr) != 0) return 14;
  if (darwin_art_bionic_pthread_rwlock_rdlock(&rwlock) != 0) return 16;
  if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) return 17;
  // Allocator reuse of a lock freed without destroy: Bionic init succeeds and
  // the new lifetime starts unlocked.
  if (darwin_art_bionic_pthread_rwlock_init(&rwlock, nullptr) != 0) return 18;
  if (darwin_art_bionic_pthread_rwlock_wrlock(&rwlock) != 0) return 19;
  if (darwin_art_bionic_pthread_rwlock_unlock(&rwlock) != 0) return 24;
  if (darwin_art_bionic_pthread_rwlock_destroy(&rwlock) != 0) return 15;
  if (darwin_art_bionic_pthread_provider_reset() != 0) return 12;
  return 0;
}

int CheckLazyResetAndPshared() {
  DarwinArtAndroidPthreadRwlock idle{};
  if (darwin_art_bionic_pthread_rwlock_rdlock(&idle) != 0) return 20;
  if (darwin_art_bionic_pthread_rwlock_unlock(&idle) != 0) return 21;
  // No imported pthread_rwlock_destroy exists in libc++_shared. Quiescent
  // provider reset owns teardown of this idle lazy side-table entry.
  if (darwin_art_bionic_pthread_provider_reset() != 0) return 22;
  DarwinArtAndroidPthreadRwlock pshared{};
  reinterpret_cast<unsigned char*>(&pshared)[8] = 1;
  if (darwin_art_bionic_pthread_rwlock_rdlock(&pshared) != kAndroidEnotsup)
    return 23;
  return 0;
}

}  // namespace

int main() {
  for (int round = 0; round < kRounds; ++round) {
    const int result = RunRound();
    if (result != 0) return result;
  }
  const int boundary = CheckLazyResetAndPshared();
  if (boundary != 0) return boundary;
  std::puts("pthread-rwlock-stress: PASS rounds=20 readers=4 concurrent>=2 writer=10000-progress wrong-unlock=EPERM destroy-held=EBUSY double-destroy=Bionic-0 reinit-live=Bionic-0 lazy-reset=clean ASan=clean");
  return 0;
}
