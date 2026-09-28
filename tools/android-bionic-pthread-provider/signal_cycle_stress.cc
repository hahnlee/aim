#include "aim_bionic_pthread.h"

#include <atomic>
#include <cassert>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <thread>
#include <unistd.h>

// Android arm64 sigaction ABI; use the real process-state trampoline.
struct AndroidAction {
  int flags;
  void (*handler)(int);
  uint64_t mask;
  void (*restorer)();
};
extern "C" int aim_bionic_sigaction(int, const AndroidAction*, AndroidAction*);
extern "C" int aim_bionic_sigsuspend(const uint64_t*);
extern "C" int aim_bionic_pthread_sigmask(int, const uint64_t*, uint64_t*);
extern "C" int aim_bionic_sem_init(void*, int, unsigned);
extern "C" int aim_bionic_sem_post(void*);
extern "C" int aim_bionic_sem_getvalue(void*, int*);
extern "C" int aim_bionic_sem_trywait(void*);
extern "C" void aim_bionic_errno_store(int32_t value) { errno = value; }

namespace {
std::atomic<unsigned> entered{0}, restarted{0}, returned{0};
std::atomic<bool> ready{false}, done{false}, bad_mask{false};
alignas(8) unsigned char ack[16]{};
static_assert(std::atomic<unsigned>::is_always_lock_free);
constexpr uint64_t Bit(unsigned n) { return uint64_t{1} << (n - 1); }
constexpr uint64_t kHandlerMask = ~(Bit(2) | Bit(3) | Bit(6) | Bit(15));
constexpr uint64_t kRestartMask = kHandlerMask & ~Bit(24);

void Restart(int signal) {
  if (signal != 24) _exit(90);
  restarted.fetch_add(1, std::memory_order_release);
}
void Suspend(int signal, void*, void*) {
  if (signal != 30) _exit(91);
  const unsigned cycle = entered.fetch_add(1, std::memory_order_acq_rel) + 1;
  if (aim_bionic_sem_post(ack) != 0) _exit(92);
  while (restarted.load(std::memory_order_acquire) < cycle)
    aim_bionic_sigsuspend(&kRestartMask);
  returned.fetch_add(1, std::memory_order_release);
}
template <class F> bool Wait(F predicate) {
  const auto end = std::chrono::steady_clock::now() + std::chrono::seconds(2);
  while (!predicate()) {
    if (std::chrono::steady_clock::now() >= end) return false;
    std::this_thread::yield();
  }
  return true;
}
void* Worker(void*) {
  uint64_t empty = 0;
  assert(aim_bionic_pthread_sigmask(2, &empty, nullptr) == 0);
  ready.store(true, std::memory_order_release);
  while (!done.load(std::memory_order_acquire)) {
    uint64_t current = 0;
    aim_bionic_pthread_sigmask(2, nullptr, &current);
    if ((current & (Bit(30) | Bit(24))) != 0) bad_mask.store(true);
    std::this_thread::yield();
  }
  return nullptr;
}
}  // namespace

int main() {
  alarm(15);
  AndroidAction suspend{0x10000004, reinterpret_cast<void (*)(int)>(Suspend), kHandlerMask, nullptr};
  AndroidAction restart{0x10000000, Restart, kHandlerMask, nullptr};
  assert(aim_bionic_sigaction(30, &suspend, nullptr) == 0);
  assert(aim_bionic_sigaction(24, &restart, nullptr) == 0);
  assert(aim_bionic_sem_init(ack, 0, 0) == 0);
  AimAndroidPthread worker = 0;
  assert(aim_bionic_pthread_create(&worker, nullptr, Worker, nullptr) == 0);
  assert(Wait([] { return ready.load(std::memory_order_acquire); }));
  for (unsigned cycle = 1; cycle <= 100; ++cycle) {
    assert(aim_bionic_pthread_kill(worker, 30) == 0);
    if (!Wait([] { int value = 0; return aim_bionic_sem_getvalue(ack, &value) == 0 && value == 1; })) {
      std::fprintf(stderr, "signal-cycle: FAIL cycle=%u entered=%u restarted=%u returned=%u bad-mask=%d\n", cycle, entered.load(), restarted.load(), returned.load(), bad_mask.load());
      _exit(1);
    }
    assert(aim_bionic_sem_trywait(ack) == 0);
    assert(aim_bionic_pthread_kill(worker, 24) == 0);
    assert(Wait([=] { return returned.load(std::memory_order_acquire) == cycle; }));
  }
  done.store(true, std::memory_order_release);
  assert(aim_bionic_pthread_join(worker, nullptr) == 0);
  assert(!bad_mask.load());
  std::puts("signal-cycle: PASS cycles=100 guest-sigaction=actual sem-ack=actual mask-restored=yes");
}
