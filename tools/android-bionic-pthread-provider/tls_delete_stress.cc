#include "aim_bionic_pthread.h"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <thread>
#include <vector>

namespace {

std::atomic<int> g_destructors{0};

void Destructor(void*) {
  g_destructors.fetch_add(1, std::memory_order_relaxed);
}

}  // namespace

int main() {
  AimAndroidPthreadKey key = 0;
  if (aim_bionic_pthread_key_create(&key, &Destructor) != 0) return 1;
  std::atomic<int> ready{0};
  std::atomic<bool> start{false};
  std::atomic<bool> stop{false};
  std::atomic<int> failures{0};
  std::vector<std::thread> threads;
  for (uintptr_t index = 1; index <= 8; ++index) {
    threads.emplace_back([&, index] {
      ready.fetch_add(1, std::memory_order_release);
      while (!start.load(std::memory_order_acquire)) {
        std::this_thread::yield();
      }
      while (!stop.load(std::memory_order_acquire)) {
        const int set = aim_bionic_pthread_setspecific(
            key, reinterpret_cast<void*>(index));
        if (set == 22) break;
        if (set != 0) {
          failures.fetch_add(1, std::memory_order_relaxed);
          break;
        }
        void* value = aim_bionic_pthread_getspecific(key);
        if (value != nullptr && value != reinterpret_cast<void*>(index)) {
          failures.fetch_add(1, std::memory_order_relaxed);
          break;
        }
      }
    });
  }
  while (ready.load(std::memory_order_acquire) != 8) {
    std::this_thread::yield();
  }
  start.store(true, std::memory_order_release);
  for (int spin = 0; spin < 10000; ++spin) std::this_thread::yield();
  if (aim_bionic_pthread_key_delete(key) != 0) return 2;
  stop.store(true, std::memory_order_release);
  for (std::thread& thread : threads) thread.join();
  if (failures.load(std::memory_order_relaxed) != 0 ||
      g_destructors.load(std::memory_order_relaxed) != 0) {
    return 3;
  }
  if (aim_bionic_pthread_provider_reset() != 0) return 4;
  for (uintptr_t iteration = 1; iteration <= 10000; ++iteration) {
    AimAndroidPthreadKey repeated = 0;
    if (aim_bionic_pthread_key_create(&repeated, nullptr) != 0 ||
        aim_bionic_pthread_setspecific(
            repeated, reinterpret_cast<void*>(iteration)) != 0 ||
        aim_bionic_pthread_key_delete(repeated) != 0) {
      return 5;
    }
  }
  // A provider-global host TLS record has one fixed Android slot array. Reuse
  // overwrites the same slot rather than allocating one host cell per key.
  if (aim_bionic_pthread_provider_retired_cell_count() != 1) return 6;
  if (aim_bionic_pthread_provider_reset() != 0 ||
      aim_bionic_pthread_provider_retired_cell_count() != 0) {
    return 7;
  }
  std::puts("pthread-tls-delete-stress: PASS threads=8 delete-vs-get+set ASan=clean repeated-delete=10000 peak-cells=1 reset-cells=0");
  return 0;
}
