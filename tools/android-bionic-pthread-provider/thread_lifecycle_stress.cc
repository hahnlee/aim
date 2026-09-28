#include "aim_bionic_pthread.h"

#include <atomic>
#include <chrono>
#include <cstdio>
#include <memory>
#include <thread>

namespace {

constexpr int kAndroidEsrch = 3;
constexpr int kAndroidEbusy = 16;
constexpr int kAndroidEinval = 22;
constexpr int kAndroidEdeadlk = 35;
constexpr int kRounds = 100;

struct Gate {
  std::atomic<bool> release{};
  std::atomic<bool> done{};
  void* result{};
};

void* ReturnArgument(void* argument) { return argument; }

void* WaitAndReturn(void* opaque) {
  auto* gate = static_cast<Gate*>(opaque);
  while (!gate->release.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  void* result = gate->result;
  gate->done.store(true, std::memory_order_release);
  return result;
}

int WaitForReset() {
  const auto deadline =
      std::chrono::steady_clock::now() + std::chrono::seconds(5);
  for (;;) {
    const int result = aim_bionic_pthread_provider_reset();
    if (result == 0) return 0;
    if (result != kAndroidEbusy || std::chrono::steady_clock::now() >= deadline)
      return result;
    std::this_thread::yield();
  }
}

int RunRound() {
  AimAndroidPthread token = 0;
  void* result = nullptr;
  if (aim_bionic_pthread_create(
          &token, nullptr, &ReturnArgument,
          reinterpret_cast<void*>(static_cast<uintptr_t>(0x1234))) != 0 ||
      token == aim_bionic_pthread_self() ||
      aim_bionic_pthread_join(token, &result) != 0 ||
      result != reinterpret_cast<void*>(static_cast<uintptr_t>(0x1234)) ||
      aim_bionic_pthread_join(token, nullptr) != kAndroidEsrch) {
    return 1;
  }
  if (aim_bionic_pthread_join(aim_bionic_pthread_self(),
                                     nullptr) != kAndroidEdeadlk ||
      aim_bionic_pthread_join(INT64_C(0x7ffffffffffffffe), nullptr) !=
          kAndroidEsrch ||
      aim_bionic_pthread_detach(INT64_C(0x7ffffffffffffffe)) !=
          kAndroidEsrch) {
    return 2;
  }
  AimAndroidPthreadAttr attributes{};
  if (aim_bionic_pthread_attr_init(&attributes) != 0 ||
      aim_bionic_pthread_create(
          &token, &attributes, &ReturnArgument,
          reinterpret_cast<void*>(static_cast<uintptr_t>(0xa771))) != 0 ||
      aim_bionic_pthread_join(token, &result) != 0 ||
      result != reinterpret_cast<void*>(static_cast<uintptr_t>(0xa771)) ||
      aim_bionic_pthread_attr_destroy(&attributes) != 0) {
    return 3;
  }

  auto detached = std::make_unique<Gate>();
  detached->result = reinterpret_cast<void*>(static_cast<uintptr_t>(0xd37a));
  if (aim_bionic_pthread_create(&token, nullptr, &WaitAndReturn,
                                       detached.get()) != 0 ||
      aim_bionic_pthread_detach(token) != 0 ||
      aim_bionic_pthread_join(token, nullptr) != kAndroidEinval) {
    return 4;
  }
  detached->release.store(true, std::memory_order_release);
  while (!detached->done.load(std::memory_order_acquire))
    std::this_thread::yield();
  if (WaitForReset() != 0) return 5;
  detached.reset();

  Gate race;
  race.result = reinterpret_cast<void*>(static_cast<uintptr_t>(0xface));
  if (aim_bionic_pthread_create(&token, nullptr, &WaitAndReturn, &race) !=
      0) {
    return 6;
  }
  int join_result = -1;
  int detach_result = -1;
  std::thread joiner([&] {
    join_result = aim_bionic_pthread_join(token, nullptr);
  });
  std::thread detacher([&] {
    detach_result = aim_bionic_pthread_detach(token);
  });
  race.release.store(true, std::memory_order_release);
  joiner.join();
  detacher.join();
  const int successes = (join_result == 0) + (detach_result == 0);
  const auto valid = [](int value) {
    return value == 0 || value == kAndroidEsrch || value == kAndroidEinval;
  };
  if (successes != 1 || !valid(join_result) || !valid(detach_result)) return 7;
  if (WaitForReset() != 0) return 8;
  return 0;
}

}  // namespace

int main() {
  for (int round = 0; round < kRounds; ++round) {
    const int result = RunRound();
    if (result != 0) return result;
  }
  std::puts("pthread-thread-lifecycle-stress: PASS rounds=100 create+join+detach result=roundtrip self=EDEADLK foreign=ESRCH join-vs-detach=one-winner detached-clean=quiescent target-clean=reset ASan=clean");
  return 0;
}
