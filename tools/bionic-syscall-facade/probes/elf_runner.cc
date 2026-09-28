#include "aim_bionic_errno.h"
#include "aim_bionic_syscall.h"
#include "aim_elf_loader.h"

#include <errno.h>
#include <signal.h>
#include <sys/mman.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <set>
#include <thread>
#include <vector>

namespace {

volatile sig_atomic_t g_signal_delivered = 0;

void RecordSignal(int signal_number) { g_signal_delivered = signal_number; }

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  const auto function = aim_bionic_syscall_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (function == nullptr) return AIM_ELF_RESOLVE_NOT_FOUND;
  *output = reinterpret_cast<uintptr_t>(function);
  return AIM_ELF_RESOLVE_FOUND;
}

template <typename Function>
Function Lookup(AimElfHandle* image, const char* name) {
  uintptr_t address = 0;
  char message[256]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  if (aim_elf_lookup(image, name, &address, &error) !=
          AIM_ELF_OK ||
      address == 0) {
    std::fprintf(stderr, "lookup %s: %s\n", name, message);
    std::abort();
  }
  return reinterpret_cast<Function>(address);
}

void Check(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-syscall-facade: FAIL %s\n", message);
    std::abort();
  }
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load Android ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "run initializers");

  using Gettid = long (*)();
  using Getpid = long (*)();
  using RtTgSigqueueinfo = long (*)(int, int, int, const void*);
  using Getrandom = long (*)(void*, uint64_t, uint32_t);
  using Getaffinity = long (*)(int, uint64_t, void*);
  using Setaffinity = long (*)(int, uint64_t, const void*);
  using Wait = long (*)(int32_t*, int32_t, int64_t);
  using Wake = long (*)(int32_t*);
  using CountedWake = long (*)(int32_t*, int32_t);
  using Readable = long (*)(const void*);
  Gettid gettid = Lookup<Gettid>(image, "SyscallFixtureGettid");
  Getpid getpid = Lookup<Getpid>(image, "SyscallFixtureGetpid");
  RtTgSigqueueinfo rt_tgsigqueueinfo = Lookup<RtTgSigqueueinfo>(
      image, "SyscallFixtureRtTgSigqueueinfo");
  Getrandom getrandom =
      Lookup<Getrandom>(image, "SyscallFixtureGetrandom");
  Getaffinity getaffinity =
      Lookup<Getaffinity>(image, "SyscallFixtureGetaffinity");
  Setaffinity setaffinity =
      Lookup<Setaffinity>(image, "SyscallFixtureSetaffinity");
  Wait wait = Lookup<Wait>(image, "SyscallFixtureWait");
  Wake wake_one = Lookup<Wake>(image, "SyscallFixtureWakeOne");
  Wake wake_all = Lookup<Wake>(image, "SyscallFixtureWakeAll");
  Wait plain_wait = Lookup<Wait>(image, "SyscallFixturePlainWait");
  CountedWake plain_wake =
      Lookup<CountedWake>(image, "SyscallFixturePlainWake");
  Wake wait_bitset = Lookup<Wake>(image, "SyscallFixtureWaitBitset");
  Wake wake_bitset = Lookup<Wake>(image, "SyscallFixtureWakeBitset");
  Readable readable = Lookup<Readable>(image, "SyscallFixtureReadable");
  Gettid unknown = Lookup<Gettid>(image, "SyscallFixtureUnknown");
  Wake bad_futex = Lookup<Wake>(image, "SyscallFixtureBadFutex");

  errno = EDOM;
  const long main_tid = gettid();
  Check(main_tid > 0 && gettid() == main_tid && errno == EDOM,
        "stable main gettid and host errno");
  const long main_pid = getpid();
  Check(main_pid > 0 && main_pid == static_cast<long>(::getpid()) &&
            getpid() == main_pid && errno == EDOM,
        "host process getpid and host errno");
  std::vector<long> tids(8);
  std::vector<std::thread> tid_threads;
  for (size_t index = 0; index < tids.size(); ++index) {
    tid_threads.emplace_back([&, index] {
      const long first = gettid();
      const long second = gettid();
      Check(first > 0 && first == second, "stable worker gettid");
      tids[index] = first;
    });
  }
  for (std::thread& thread : tid_threads) thread.join();
  std::set<long> distinct(tids.begin(), tids.end());
  Check(distinct.size() == tids.size() && distinct.count(main_tid) == 0,
        "unique per-thread gettid");

  uint8_t android_siginfo[128]{};
  aim_bionic_errno_store(0);
  Check(rt_tgsigqueueinfo(getpid(), static_cast<int>(main_tid), 0,
                          android_siginfo) == -1 &&
            aim_bionic_errno_load() == 22,
        "rt_tgsigqueueinfo rejects signal zero");
  aim_bionic_errno_store(0);
  Check(rt_tgsigqueueinfo(getpid(), INT32_MAX, 10, android_siginfo) == -1 &&
            aim_bionic_errno_load() == 3,
        "rt_tgsigqueueinfo unknown virtual tid ESRCH");
  struct sigaction action {};
  action.sa_handler = RecordSignal;
  sigemptyset(&action.sa_mask);
  Check(sigaction(SIGUSR1, &action, nullptr) == 0,
        "install rt_tgsigqueueinfo probe handler");
  Check(rt_tgsigqueueinfo(getpid(), static_cast<int>(main_tid), 10,
                          android_siginfo) == 0 &&
            g_signal_delivered == SIGUSR1,
        "rt_tgsigqueueinfo translates and targets virtual tid");

  uint8_t random_bytes[32];
  std::fill(std::begin(random_bytes), std::end(random_bytes), uint8_t{0xa5});
  aim_bionic_errno_store(0);
  Check(getrandom(random_bytes, sizeof(random_bytes), 1) ==
            static_cast<long>(sizeof(random_bytes)) &&
            aim_bionic_errno_load() == 0 &&
            !std::all_of(std::begin(random_bytes), std::end(random_bytes),
                         [](uint8_t byte) { return byte == 0xa5; }),
        "getrandom host CSPRNG and GRND_NONBLOCK");
  Check(getrandom(nullptr, 0, 0) == 0, "getrandom empty buffer");
  aim_bionic_errno_store(0);
  Check(getrandom(random_bytes, sizeof(random_bytes), 0x80000000u) == -1 &&
            aim_bionic_errno_load() == 22,
        "getrandom unknown flags EINVAL");
  aim_bionic_errno_store(0);
  Check(getrandom(random_bytes, sizeof(random_bytes), 2u | 4u) == -1 &&
            aim_bionic_errno_load() == 22,
        "getrandom RANDOM plus INSECURE EINVAL");

  uint8_t affinity[128]{};
  const size_t cpu_count = static_cast<size_t>(sysconf(_SC_NPROCESSORS_ONLN));
  const size_t mask_size = ((cpu_count + 63) / 64) * 8;
  Check(cpu_count > 0 && mask_size <= sizeof(affinity), "host CPU count");
  uint8_t expected_affinity[128]{};
  for (size_t cpu = 0; cpu < cpu_count; ++cpu)
    expected_affinity[cpu / 8] |= static_cast<uint8_t>(1u << (cpu % 8));
  Check(getaffinity(0, sizeof(affinity), affinity) == static_cast<long>(mask_size) &&
            std::equal(std::begin(affinity), std::end(affinity), expected_affinity),
        "host online CPU affinity");
  Check(setaffinity(0, sizeof(affinity), affinity) == 0,
        "unrestricted affinity succeeds without pinning");
  uint8_t restricted_affinity[128]{};
  restricted_affinity[0] = 1;
  aim_bionic_errno_store(0);
  if (cpu_count > 1)
    Check(setaffinity(0, sizeof(restricted_affinity), restricted_affinity) == -1 &&
              aim_bionic_errno_load() == 38,
          "exact CPU restriction explicitly unsupported");
  std::fill(std::begin(affinity), std::end(affinity), uint8_t{0xa5});
  Check(getaffinity(0, sizeof(affinity), affinity) == static_cast<long>(mask_size) &&
            std::equal(affinity, affinity + mask_size, expected_affinity) &&
            std::all_of(affinity + mask_size, std::end(affinity),
                        [](uint8_t byte) { return byte == 0xa5; }),
        "failed restriction leaves host mask; raw syscall preserves tail");
  Check(getaffinity(0, mask_size - 1, affinity) == -1 &&
            aim_bionic_errno_load() == 22, "short mask EINVAL");
  Check(getaffinity(INT32_MAX, sizeof(affinity), affinity) == -1 &&
            aim_bionic_errno_load() == 3, "unknown target ESRCH");
  Check(setaffinity(INT32_MAX, sizeof(affinity), expected_affinity) == -1 &&
            aim_bionic_errno_load() == 3, "unknown set target ESRCH");
  Check(getaffinity(static_cast<int>(main_tid), sizeof(affinity), affinity) ==
            static_cast<long>(mask_size), "explicit current target");
  Check(getaffinity(static_cast<int>(tids.front()), sizeof(affinity), affinity) == -1 &&
            aim_bionic_errno_load() == 3, "exited target ESRCH");
  std::atomic<long> affinity_tid{0};
  std::atomic<bool> affinity_done{false};
  std::thread affinity_worker([&] {
    affinity_tid.store(gettid());
    while (!affinity_done.load()) std::this_thread::yield();
  });
  while (affinity_tid.load() == 0) std::this_thread::yield();
  Check(getaffinity(static_cast<int>(affinity_tid.load()), sizeof(affinity), affinity) ==
            static_cast<long>(mask_size), "other live target");
  Check(setaffinity(static_cast<int>(affinity_tid.load()), sizeof(expected_affinity),
                    expected_affinity) == 0, "other live target unrestricted set");
  affinity_done.store(true);
  affinity_worker.join();
  std::fill(std::begin(affinity), std::end(affinity), uint8_t{0xa5});
  Check(aim_bionic_affinity_get(0, sizeof(affinity), affinity) == 0 &&
            std::equal(std::begin(affinity), std::end(affinity), expected_affinity),
        "libc fixed ABI returns zero and clears tail");
  Check(aim_bionic_affinity_set(0, sizeof(affinity), affinity) == 0,
        "libc fixed ABI unrestricted set");
  Check(aim_bionic_affinity_get(0, sizeof(affinity), nullptr) == -1 &&
            aim_bionic_errno_load() == 14, "libc null output EFAULT");
  std::fill(std::begin(restricted_affinity), std::end(restricted_affinity), 0);
  aim_bionic_errno_store(0);
  Check(setaffinity(0, sizeof(restricted_affinity), restricted_affinity) == -1 &&
            aim_bionic_errno_load() == 22,
        "reject empty affinity");

  alignas(4) int32_t word = 0;
  constexpr size_t kWaiters = 4;
  std::vector<long> wait_results(kWaiters, -2);
  std::vector<int32_t> wait_errnos(kWaiters, -1);
  std::vector<std::thread> wait_threads;
  for (size_t index = 0; index < kWaiters; ++index) {
    wait_threads.emplace_back([&, index] {
      aim_bionic_errno_store(0);
      wait_results[index] = wait(&word, 0, 2000000000LL);
      wait_errnos[index] = aim_bionic_errno_load();
    });
  }
  const auto waiter_deadline =
      std::chrono::steady_clock::now() + std::chrono::seconds(2);
  while (aim_bionic_syscall_waiter_count(&word) != kWaiters &&
         std::chrono::steady_clock::now() < waiter_deadline) {
    std::this_thread::yield();
  }
  Check(aim_bionic_syscall_waiter_count(&word) == kWaiters,
        "contention admission");
  __atomic_store_n(&word, 1, __ATOMIC_SEQ_CST);
  Check(wake_all(&word) == static_cast<long>(kWaiters), "wake all count");
  for (std::thread& thread : wait_threads) thread.join();
  for (size_t index = 0; index < kWaiters; ++index) {
    Check(wait_results[index] == 0 && wait_errnos[index] == 0,
          "wake all result");
  }

  word = 0;
  std::atomic<int> completed{0};
  std::thread first([&] {
    Check(wait(&word, 0, 2000000000LL) == 0, "wake-one first waiter");
    completed.fetch_add(1, std::memory_order_relaxed);
  });
  std::thread second([&] {
    Check(wait(&word, 0, 2000000000LL) == 0, "wake-one second waiter");
    completed.fetch_add(1, std::memory_order_relaxed);
  });
  while (aim_bionic_syscall_waiter_count(&word) != 2) {
    std::this_thread::yield();
  }
  Check(wake_one(&word) == 1, "wake one count");
  while (completed.load(std::memory_order_relaxed) != 1) {
    std::this_thread::yield();
  }
  Check(aim_bionic_syscall_waiter_count(&word) == 1,
        "wake one leaves one waiter");
  Check(wake_all(&word) == 1, "final wake count");
  first.join();
  second.join();

  word = 0;
  std::thread plain_waiter([&] {
    Check(plain_wait(&word, 0, 2000000000LL) == 0,
          "plain futex wait result");
  });
  while (aim_bionic_syscall_waiter_count(&word) != 1) {
    std::this_thread::yield();
  }
  Check(plain_wake(&word, 3) == 1, "plain futex wake count");
  plain_waiter.join();

  word = 0;
  std::thread bitset_waiter([&] {
    Check(wait_bitset(&word) == 0, "bitset futex wait result");
  });
  while (aim_bionic_syscall_waiter_count(&word) != 1) {
    std::this_thread::yield();
  }
  Check(wake_bitset(&word) == 1, "bitset futex wake count");
  bitset_waiter.join();

  word = 7;
  aim_bionic_errno_store(0);
  Check(wait(&word, 0, 1000000LL) == -1 &&
            aim_bionic_errno_load() == 11,
        "futex compare mismatch EAGAIN");
  word = 0;
  aim_bionic_errno_store(0);
  Check(wait(&word, 0, 20000000LL) == -1 &&
            aim_bionic_errno_load() == 110,
        "futex relative timeout ETIMEDOUT");
  Check(wake_one(&word) == 0, "timeout leaves no wake token");

  word = 0;
  std::atomic<bool> spurious_done{false};
  long spurious_result = 0;
  int32_t spurious_errno = 0;
  const auto spurious_start = std::chrono::steady_clock::now();
  std::thread spurious_waiter([&] {
    aim_bionic_errno_store(0);
    spurious_result = wait(&word, 0, 80000000LL);
    spurious_errno = aim_bionic_errno_load();
    spurious_done.store(true, std::memory_order_release);
  });
  while (aim_bionic_syscall_waiter_count(&word) != 1) {
    std::this_thread::yield();
  }
  while (!spurious_done.load(std::memory_order_acquire)) {
    aim_bionic_syscall_spurious_wake(&word);
    std::this_thread::yield();
  }
  spurious_waiter.join();
  const auto spurious_elapsed = std::chrono::steady_clock::now() - spurious_start;
  Check(spurious_result == -1 && spurious_errno == 110 &&
            spurious_elapsed >= std::chrono::milliseconds(40) &&
            spurious_elapsed < std::chrono::milliseconds(500),
        "spurious wake does not extend monotonic deadline");
  Check(wake_one(&word) == 0, "spurious timeout leaves no token");

  constexpr size_t kWaitTableCapacity = 257;
  std::vector<int32_t> capacity_words(kWaitTableCapacity, 0);
  std::vector<long> capacity_results(kWaitTableCapacity, -2);
  std::vector<std::thread> capacity_threads;
  capacity_threads.reserve(kWaitTableCapacity);
  for (size_t index = 0; index < kWaitTableCapacity; ++index) {
    capacity_threads.emplace_back([&, index] {
      capacity_results[index] =
          wait(&capacity_words[index], 0, 5000000000LL);
    });
  }
  const auto capacity_deadline =
      std::chrono::steady_clock::now() + std::chrono::seconds(5);
  for (;;) {
    size_t admitted = 0;
    for (size_t index = 0; index < kWaitTableCapacity; ++index) {
      admitted += aim_bionic_syscall_waiter_count(
          &capacity_words[index]) == 1;
    }
    if (admitted == kWaitTableCapacity) break;
    Check(std::chrono::steady_clock::now() < capacity_deadline,
          "wait-table admission deadline");
    std::this_thread::yield();
  }
  alignas(4) int32_t overflow_word = 0;
  aim_bionic_errno_store(0);
  Check(wait(&overflow_word, 0, 1000000LL) == -1 &&
            aim_bionic_errno_load() == 11,
        "257-address wait-table exhaustion EAGAIN");
  for (size_t index = 0; index < kWaitTableCapacity; ++index) {
    Check(wake_one(&capacity_words[index]) == 1,
          "capacity waiter wake count");
  }
  for (std::thread& thread : capacity_threads) thread.join();
  for (long result : capacity_results) {
    Check(result == 0, "capacity waiter result");
  }

  aim_bionic_errno_store(0);
  Check(readable(reinterpret_cast<const void*>(gettid)) == -1 &&
            aim_bionic_errno_load() == 22,
        "libunwind readable probe EINVAL");
  void* guard = mmap(nullptr, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
  Check(guard != MAP_FAILED, "guard map");
  aim_bionic_errno_store(0);
  Check(getrandom(guard, 1, 0) == -1 &&
            aim_bionic_errno_load() == 14,
        "getrandom guard EFAULT");
  aim_bionic_errno_store(0);
  Check(wake_one(static_cast<int32_t*>(guard)) == -1 &&
            aim_bionic_errno_load() == 14,
        "futex wake guard EFAULT");
  aim_bionic_errno_store(0);
  Check(readable(guard) == -1 && aim_bionic_errno_load() == 14,
        "libunwind guard probe EFAULT");
  Check(munmap(guard, 16384) == 0, "guard unmap");
  aim_bionic_errno_store(0);
  Check(wake_one(static_cast<int32_t*>(guard)) == -1 &&
            aim_bionic_errno_load() == 14,
        "futex wake unmapped EFAULT");
  aim_bionic_errno_store(0);
  Check(readable(guard) == -1 && aim_bionic_errno_load() == 14,
        "libunwind unmapped probe EFAULT");
  aim_bionic_errno_store(0);
  Check(readable(reinterpret_cast<const void*>(UINTPTR_MAX - 3)) == -1 &&
            aim_bionic_errno_load() == 14,
        "libunwind overflow probe EFAULT");

  aim_bionic_errno_store(0);
  Check(unknown() == -1 && aim_bionic_errno_load() == 38,
        "unknown syscall ENOSYS");
  aim_bionic_errno_store(0);
  Check(bad_futex(&word) == -1 && aim_bionic_errno_load() == 22,
        "unknown futex operation EINVAL");
  Check(errno == EDOM, "host errno preserved");

  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android ELF");
  std::fprintf(stderr,
               "bionic-syscall-facade: ELF PASS gettid=threads futex="
               "wait+wake-one+wake-all timeout=monotonic-spurious "
               "getrandom=host-csprng "
               "sched-affinity=host-topology+target-validation+restriction-ENOSYS "
               "capacity=257 invalid-wake=EFAULT rt_sigprocmask=readability "
               "rt_tgsigqueueinfo=virtual-tid-delivery "
               "unknown=closed\n");
  return 0;
}
