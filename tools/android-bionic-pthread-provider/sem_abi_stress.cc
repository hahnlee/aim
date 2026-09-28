#include <cassert>
#include <atomic>
#include <climits>
#include <chrono>
#include <cstdint>
#include <ctime>
#include <thread>

extern "C" int aim_bionic_sem_destroy(void*);
extern "C" int aim_bionic_sem_getvalue(void*, int*);
extern "C" int aim_bionic_sem_init(void*, int, unsigned);
extern "C" int aim_bionic_sem_post(void*);
extern "C" int aim_bionic_sem_trywait(void*);
extern "C" int aim_bionic_sem_timedwait(void*, const timespec*);
extern "C" int aim_bionic_sem_wait(void*);

namespace {
std::atomic<int> g_errno{0};
}

extern "C" void aim_bionic_errno_store(int32_t value) {
  g_errno.store(value, std::memory_order_relaxed);
}

int main() {
  alignas(64) unsigned char storage[64] = {};
  void* semaphore = storage;

  assert(aim_bionic_sem_init(semaphore, 0, 0) == 0);
  g_errno.store(9);
  assert(aim_bionic_sem_trywait(semaphore) == -1);
  assert(g_errno.load() == 11);  // EAGAIN
  assert(aim_bionic_sem_post(semaphore) == 0);
  assert(aim_bionic_sem_wait(semaphore) == 0);

  timespec timeout{};
  clock_gettime(CLOCK_REALTIME, &timeout);
  timeout.tv_nsec += 10 * 1000 * 1000;
  if (timeout.tv_nsec >= 1000000000L) {
    ++timeout.tv_sec;
    timeout.tv_nsec -= 1000000000L;
  }
  g_errno.store(9);
  assert(aim_bionic_sem_timedwait(semaphore, &timeout) == -1);
  assert(g_errno.load() == 110);  // ETIMEDOUT, not stale errno

  timeout.tv_nsec = 1000000000L;
  assert(aim_bionic_sem_timedwait(semaphore, &timeout) == -1);
  assert(g_errno.load() == 22);  // EINVAL

  assert(aim_bionic_sem_destroy(semaphore) == 0);
  assert(aim_bionic_sem_destroy(semaphore) == -1);
  assert(g_errno.load() == 22);

  assert(aim_bionic_sem_init(semaphore, 0, UINT_MAX) == -1);
  assert(g_errno.load() == 22);  // value exceeds Android SEM_VALUE_MAX
  assert(aim_bionic_sem_init(semaphore, 0, 0x3fffffffU) == 0);
  assert(aim_bionic_sem_post(semaphore) == -1);
  assert(g_errno.load() == 75);  // EOVERFLOW
  assert(aim_bionic_sem_destroy(semaphore) == 0);

  assert(aim_bionic_sem_init(semaphore, 0, 1) == 0);
  timeout.tv_nsec = 1000000000L;
  assert(aim_bionic_sem_timedwait(semaphore, &timeout) == 0);
  assert(aim_bionic_sem_destroy(semaphore) == 0);

  // A zero-valued, idle semaphore is valid to destroy; an active waiter is
  // the condition that makes destruction busy.
  assert(aim_bionic_sem_init(semaphore, 0, 0) == 0);
  std::atomic<bool> entered{false};
  std::thread waiter([&] {
    entered.store(true, std::memory_order_release);
    assert(aim_bionic_sem_wait(semaphore) == 0);
  });
  while (!entered.load(std::memory_order_acquire)) std::this_thread::yield();
  std::this_thread::sleep_for(std::chrono::milliseconds(10));
  assert(aim_bionic_sem_destroy(semaphore) == -1);
  assert(g_errno.load() == 16);  // EBUSY
  assert(aim_bionic_sem_post(semaphore) == 0);
  waiter.join();
  assert(aim_bionic_sem_destroy(semaphore) == 0);
  return 0;
}
