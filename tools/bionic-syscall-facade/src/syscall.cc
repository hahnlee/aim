#include "aim_bionic_syscall.h"

#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach/semaphore.h>
#include <os/lock.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <mutex>
#include <unordered_map>

extern "C" void aim_bionic_errno_store(int32_t android_errno);

namespace {

constexpr uint64_t kFutex = 98;
constexpr uint64_t kGetpid = 172;
constexpr uint64_t kSchedSetaffinity = 122;
constexpr uint64_t kSchedGetaffinity = 123;
constexpr uint64_t kTgkill = 131;
constexpr uint64_t kRtSigprocmask = 135;
constexpr uint64_t kGettid = 178;
constexpr uint64_t kRtTgSigqueueinfo = 240;
constexpr uint64_t kGetrandom = 278;
constexpr uint32_t kFutexWait = 0;
constexpr uint32_t kFutexWake = 1;
constexpr uint32_t kFutexWaitBitset = 9;
constexpr uint32_t kFutexWakeBitset = 10;
constexpr uint32_t kFutexPrivateFlag = 128;
constexpr uint32_t kFutexClockRealtime = 256;
constexpr uint32_t kFutexCommandMask = 0x7f;
constexpr uint32_t kFutexBitsetMatchAny = UINT32_MAX;
constexpr int32_t kAndroidEagain = 11;
constexpr int32_t kAndroidEfault = 14;
constexpr int32_t kAndroidEinval = 22;
constexpr int32_t kAndroidEnosys = 38;
constexpr int32_t kAndroidEtimedout = 110;
constexpr uint32_t kGrndNonblock = 0x1;
constexpr uint32_t kGrndRandom = 0x2;
constexpr uint32_t kGrndInsecure = 0x4;
constexpr size_t kWaitEntryCount = 257;

struct AndroidTimespec {
  int64_t seconds;
  int64_t nanoseconds;
};

struct WaitEntry {
  const int32_t *address = nullptr;
  size_t waiters = 0;
  size_t wake_tokens = 0;
  semaphore_t semaphore = SEMAPHORE_NULL;
};

os_unfair_lock g_wait_lock = OS_UNFAIR_LOCK_INIT;
bool g_wait_initialized = false;
WaitEntry g_wait_entries[kWaitEntryCount];
std::atomic<uint32_t> g_next_tid{1};
// These flags are intentionally initialized before any guest thread can enter
// the syscall seam. Reading getenv lazily from futex/syscall code can contend
// with the suspended thread holding libc's environment lock during a GC stop.
// They are immutable for the lifetime of the process.
bool g_debug_signal_syscall = false;
bool g_debug_futex = false;
bool g_debug_syscall = false;
std::atomic<uint32_t> g_signal_tgkill_emitted{0};
std::atomic<uint32_t> g_signal_queued_emitted{0};

void DebugPrintf(const char *format, ...) {
  char buffer[512];
  va_list arguments;
  va_start(arguments, format);
  const int length = std::vsnprintf(buffer, sizeof(buffer), format, arguments);
  va_end(arguments);
  if (length > 0) {
    (void)write(STDERR_FILENO, buffer,
                std::min(static_cast<size_t>(length), sizeof(buffer) - 1));
  }
}

__attribute__((constructor)) void InitializeDebugFlags() {
  g_debug_signal_syscall = std::getenv("AIM_DEBUG_SIGNAL_SYSCALL") != nullptr;
  g_debug_futex = std::getenv("AIM_DEBUG_FUTEX") != nullptr;
  g_debug_syscall = std::getenv("AIM_DEBUG_SYSCALL") != nullptr;
}

struct ThreadRegistry {
  std::mutex mutex;
  std::unordered_map<uint32_t, pthread_t> threads;
};

ThreadRegistry &Threads() {
  static ThreadRegistry *registry = new ThreadRegistry();
  return *registry;
}

void TraceSignalSyscall(const char *name, const uint64_t *arguments) {
  std::atomic<uint32_t> &emitted =
      std::strcmp(name, "rt_tgsigqueueinfo") == 0
          ? g_signal_queued_emitted
          : g_signal_tgkill_emitted;
  if (!g_debug_signal_syscall ||
      emitted.fetch_add(1, std::memory_order_relaxed) >= 128)
    return;
  uint32_t information_words[8] = {};
  if (std::strcmp(name, "rt_tgsigqueueinfo") == 0 && arguments[4] != 0) {
    memcpy(information_words, reinterpret_cast<const void *>(arguments[4]),
           sizeof(information_words));
  }
  char message[256];
  const int length = std::snprintf(
      message, sizeof(message),
      "DARWIN signal syscall: %s pid=%llu tid=%llu signal=%llu info=%p "
      "words=%08x,%08x,%08x,%08x,%08x,%08x,%08x,%08x\n", name,
      static_cast<unsigned long long>(arguments[1]),
      static_cast<unsigned long long>(arguments[2]),
      static_cast<unsigned long long>(arguments[3]),
      reinterpret_cast<void *>(arguments[4]), information_words[0],
      information_words[1], information_words[2], information_words[3],
      information_words[4], information_words[5], information_words[6],
      information_words[7]);
  if (length > 0) {
    (void)write(STDERR_FILENO, message,
                std::min(static_cast<size_t>(length), sizeof(message) - 1));
  }
}

int HostSignal(int android_signal) {
  // Match the injective Android-to-Darwin signal map used by sigaction and
  // pthread_kill; syscall delivery must not alias SIGPWR onto SIGUSR1.
  static const unsigned char map[32] = {
      0,        SIGHUP,  SIGINT,  SIGQUIT, SIGILL,  SIGTRAP,   SIGABRT,
      SIGBUS,   SIGFPE,  SIGKILL, SIGUSR1, SIGSEGV, SIGUSR2,   SIGPIPE,
      SIGALRM,  SIGTERM, SIGEMT,  SIGCHLD, SIGCONT, SIGSTOP,   SIGTSTP,
      SIGTTIN,  SIGTTOU, SIGURG,  SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF,
      SIGWINCH, SIGIO,   SIGINFO, SIGSYS};
  return android_signal > 0 && android_signal < 32 ? map[android_signal] : 0;
}

struct ThreadIdentity {
  uint32_t tid = 0;
  ~ThreadIdentity() {
    if (tid == 0)
      return;
    ThreadRegistry &registry = Threads();
    std::lock_guard<std::mutex> lock(registry.mutex);
    registry.threads.erase(tid);
  }
};

thread_local ThreadIdentity g_thread_identity;

void InitializeWaitEntries() {
  if (g_wait_initialized)
    return;
  for (WaitEntry &entry : g_wait_entries) {
    if (semaphore_create(mach_task_self(), &entry.semaphore, SYNC_POLICY_FIFO,
                         0) != KERN_SUCCESS) {
      std::abort();
    }
  }
  g_wait_initialized = true;
}

bool HasRangeProtection(const void *pointer, size_t length,
                        vm_prot_t required) {
  const uintptr_t begin = reinterpret_cast<uintptr_t>(pointer);
  if (begin == 0 || length == 0 || begin > UINTPTR_MAX - length)
    return false;
  const uintptr_t end = begin + length;
  mach_vm_address_t region = static_cast<mach_vm_address_t>(begin);
  mach_vm_size_t region_size = 0;
  vm_region_basic_info_data_64_t info{};
  mach_msg_type_number_t count = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t object = MACH_PORT_NULL;
  const kern_return_t status = mach_vm_region(
      mach_task_self(), &region, &region_size, VM_REGION_BASIC_INFO_64,
      reinterpret_cast<vm_region_info_t>(&info), &count, &object);
  if (object != MACH_PORT_NULL) {
    (void)mach_port_deallocate(mach_task_self(), object);
  }
  if (status != KERN_SUCCESS || (info.protection & required) != required ||
      region > begin || region_size > UINT64_MAX - region) {
    return false;
  }
  return end <= region + region_size;
}

bool IsReadableRange(const void *pointer, size_t length) {
  return HasRangeProtection(pointer, length, VM_PROT_READ);
}

bool IsWritableRange(const void *pointer, size_t length) {
  return HasRangeProtection(pointer, length, VM_PROT_WRITE);
}

WaitEntry *FindWaitEntry(const int32_t *address, bool create) {
  WaitEntry *empty = nullptr;
  for (WaitEntry &entry : g_wait_entries) {
    if (entry.address == address)
      return &entry;
    if (empty == nullptr && entry.address == nullptr && entry.waiters == 0 &&
        entry.wake_tokens == 0) {
      empty = &entry;
    }
  }
  if (create && empty != nullptr)
    empty->address = address;
  return create ? empty : nullptr;
}

void ReleaseEmptyEntry(WaitEntry *entry) {
  if (entry->waiters == 0 && entry->wake_tokens == 0)
    entry->address = nullptr;
}

long Fail(int32_t android_errno) {
  aim_bionic_errno_store(android_errno);
  return -1;
}

long GetTid() {
  if (g_thread_identity.tid != 0)
    return g_thread_identity.tid;
  const uint32_t candidate = g_next_tid.fetch_add(1, std::memory_order_relaxed);
  if (candidate == 0 || candidate > INT32_MAX)
    return Fail(kAndroidEnosys);
  ThreadRegistry &registry = Threads();
  try {
    std::lock_guard<std::mutex> lock(registry.mutex);
    registry.threads.emplace(candidate, pthread_self());
  } catch (...) {
    return Fail(kAndroidEnosys);
  }
  g_thread_identity.tid = candidate;
  return static_cast<long>(candidate);
}

long GetPid() { return static_cast<long>(getpid()); }

size_t OnlineCpuCount() {
  const long count = sysconf(_SC_NPROCESSORS_ONLN);
  return count > 0 ? static_cast<size_t>(count) : 1;
}

bool AffinityTargetExists(int32_t tid) {
  if (tid == 0) return true;
  if (GetTid() < 0) return false;
  ThreadRegistry &registry = Threads();
  std::lock_guard<std::mutex> lock(registry.mutex);
  return registry.threads.find(static_cast<uint32_t>(tid)) != registry.threads.end();
}

long SchedGetaffinity(const uint64_t *arguments) {
  const int32_t tid = static_cast<int32_t>(arguments[1]);
  const uint64_t capacity = arguments[2];
  void *mask = reinterpret_cast<void *>(arguments[3]);
  if (tid < 0 || capacity == 0 || capacity > SIZE_MAX)
    return Fail(kAndroidEinval);
  const size_t count = OnlineCpuCount();
  const size_t mask_size = ((count + 63) / 64) * sizeof(uint64_t);
  if (capacity < mask_size) return Fail(kAndroidEinval);
  if (!AffinityTargetExists(tid)) return Fail(3);  // Android ESRCH.
  if (!IsWritableRange(mask, mask_size))
    return Fail(kAndroidEfault);
  std::memset(mask, 0, mask_size);
  auto *bytes = static_cast<uint8_t *>(mask);
  for (size_t cpu = 0; cpu < count; ++cpu)
    bytes[cpu / 8] |= static_cast<uint8_t>(1u << (cpu % 8));
  // Raw Linux syscall writes/returns only the kernel mask size.
  return static_cast<long>(mask_size);
}

long SchedSetaffinity(const uint64_t *arguments) {
  const int32_t tid = static_cast<int32_t>(arguments[1]);
  const uint64_t capacity = arguments[2];
  const void *mask = reinterpret_cast<const void *>(arguments[3]);
  if (tid < 0 || capacity == 0 || capacity > SIZE_MAX)
    return Fail(kAndroidEinval);
  if (!AffinityTargetExists(tid)) return Fail(3);  // Android ESRCH.
  if (!IsReadableRange(mask, static_cast<size_t>(capacity)))
    return Fail(kAndroidEfault);
  const auto *bytes = static_cast<const uint8_t *>(mask);
  const size_t count = OnlineCpuCount();
  size_t selected = 0;
  for (size_t cpu = 0; cpu < count; ++cpu) {
    if (cpu / 8 < capacity && (bytes[cpu / 8] & (1u << (cpu % 8))))
      ++selected;
  }
  if (selected == 0)
    return Fail(kAndroidEinval);
  // Darwin affinity tags are cache-sharing hints, not CPU pinning. Report
  // unavailable for a restriction instead of claiming an unapplied binding.
  // An unrestricted mask needs no host operation; offline bits are ignored.
  if (selected != count) return Fail(kAndroidEnosys);
  return 0;
}

long Tgkill(const uint64_t *arguments) {
  TraceSignalSyscall("tgkill", arguments);
  const int process = static_cast<int>(arguments[1]);
  const int tid = static_cast<int>(arguments[2]);
  const int signal_number = static_cast<int>(arguments[3]);
  const int host_signal = HostSignal(signal_number);
  if (process != getpid() || tid <= 0 || host_signal == 0) {
    return Fail(kAndroidEinval);
  }
  pthread_t target{};
  {
    ThreadRegistry &registry = Threads();
    std::lock_guard<std::mutex> lock(registry.mutex);
    const auto found = registry.threads.find(static_cast<uint32_t>(tid));
    if (found == registry.threads.end())
      return Fail(3);
    target = found->second;
  }
  const int result = pthread_kill(target, host_signal);
  return result == 0 ? 0 : Fail(result);
}

long RtTgSigqueueinfo(const uint64_t *arguments) {
  TraceSignalSyscall("rt_tgsigqueueinfo", arguments);
  const int process = static_cast<int>(arguments[1]);
  const int tid = static_cast<int>(arguments[2]);
  const int signal_number = static_cast<int>(arguments[3]);
  const void *information = reinterpret_cast<const void *>(arguments[4]);
  if (process != getpid() || tid <= 0 || signal_number <= 0 ||
      information == nullptr) {
    return Fail(kAndroidEinval);
  }
  const int host_signal = HostSignal(signal_number);
  if (host_signal == 0)
    return Fail(kAndroidEinval);
  pthread_t target{};
  {
    ThreadRegistry &registry = Threads();
    std::lock_guard<std::mutex> lock(registry.mutex);
    const auto found = registry.threads.find(static_cast<uint32_t>(tid));
    if (found == registry.threads.end())
      return Fail(3);
    target = found->second;
  }
  // Darwin has no rt_tgsigqueueinfo equivalent that accepts Linux siginfo_t.
  // Chromium uses this syscall to target the current thread while preserving
  // Android signal semantics. Deliver the translated signal to the registered
  // pthread; its installed handler reconstructs the host-side signal context.
  const int result = pthread_kill(target, host_signal);
  return result == 0 ? 0 : Fail(result);
}

long GetRandom(const uint64_t *arguments) {
  void *buffer = reinterpret_cast<void *>(arguments[1]);
  const uint64_t length = arguments[2];
  const uint32_t flags = static_cast<uint32_t>(arguments[3]);
  constexpr uint32_t kKnownFlags = kGrndNonblock | kGrndRandom | kGrndInsecure;
  if ((flags & ~kKnownFlags) != 0 ||
      (flags & (kGrndRandom | kGrndInsecure)) ==
          (kGrndRandom | kGrndInsecure) ||
      length > static_cast<uint64_t>(std::numeric_limits<long>::max())) {
    return Fail(kAndroidEinval);
  }
  if (length == 0)
    return 0;
  if (!IsWritableRange(buffer, static_cast<size_t>(length))) {
    return Fail(kAndroidEfault);
  }
  // Darwin's arc4random_buf is backed by the host CSPRNG and cannot expose a
  // Linux entropy-pool-not-ready state. Consequently GRND_NONBLOCK has the
  // same successful result as a blocking request while preserving secure
  // randomness and never forwarding an Android syscall number to Darwin.
  arc4random_buf(buffer, static_cast<size_t>(length));
  return static_cast<long>(length);
}

bool MonotonicDeadline(const AndroidTimespec &relative, timespec *deadline) {
  if (relative.seconds < 0 || relative.nanoseconds < 0 ||
      relative.nanoseconds >= 1000000000LL ||
      clock_gettime(CLOCK_MONOTONIC, deadline) != 0) {
    return false;
  }
  if (relative.seconds >
      std::numeric_limits<time_t>::max() - deadline->tv_sec) {
    return false;
  }
  deadline->tv_sec += static_cast<time_t>(relative.seconds);
  deadline->tv_nsec += static_cast<long>(relative.nanoseconds);
  if (deadline->tv_nsec >= 1000000000L) {
    if (deadline->tv_sec == std::numeric_limits<time_t>::max())
      return false;
    ++deadline->tv_sec;
    deadline->tv_nsec -= 1000000000L;
  }
  return true;
}

bool RemainingUntil(clockid_t clock, const timespec &deadline,
                    timespec *remaining) {
  timespec now{};
  if (clock_gettime(clock, &now) != 0)
    return false;
  if (now.tv_sec > deadline.tv_sec ||
      (now.tv_sec == deadline.tv_sec && now.tv_nsec >= deadline.tv_nsec)) {
    remaining->tv_sec = 0;
    remaining->tv_nsec = 0;
    return false;
  }
  remaining->tv_sec = deadline.tv_sec - now.tv_sec;
  remaining->tv_nsec = deadline.tv_nsec - now.tv_nsec;
  if (remaining->tv_nsec < 0) {
    --remaining->tv_sec;
    remaining->tv_nsec += 1000000000L;
  }
  return true;
}

long FutexWait(const uint64_t *arguments, bool absolute_timeout,
               clockid_t timeout_clock) {
  auto *address = reinterpret_cast<int32_t *>(arguments[1]);
  const int32_t expected = static_cast<int32_t>(arguments[3]);
  const void *timeout_pointer = reinterpret_cast<const void *>(arguments[4]);
  const bool debug = g_debug_futex;
  if (address == nullptr || (reinterpret_cast<uintptr_t>(address) & 3) != 0) {
    return Fail(kAndroidEinval);
  }
  // FUTEX_WAIT ignores uaddr2 and val3. Variadic syscall call sites commonly
  // leave their backing argument registers unspecified, so validating those
  // values would reject otherwise valid Linux calls.
  if (!IsReadableRange(address, sizeof(*address)) ||
      (timeout_pointer != nullptr &&
       !IsReadableRange(timeout_pointer, sizeof(AndroidTimespec)))) {
    return Fail(kAndroidEfault);
  }
  if (debug) {
    char name[64]{};
    AndroidTimespec debug_timeout{};
    if (timeout_pointer != nullptr)
      memcpy(&debug_timeout, timeout_pointer, sizeof(debug_timeout));
    (void)pthread_getname_np(pthread_self(), name, sizeof(name));
    DebugPrintf(
                 "DARWIN futex: wait enter thread=%s address=%p expected=%d "
                 "current=%d absolute=%d timeout=%p value=%lld.%09lld\n",
                 name, static_cast<void *>(address), expected,
                 __atomic_load_n(address, __ATOMIC_RELAXED),
                 absolute_timeout ? 1 : 0, timeout_pointer,
                 static_cast<long long>(debug_timeout.seconds),
                 static_cast<long long>(debug_timeout.nanoseconds));
  }
  const bool timed = timeout_pointer != nullptr;
  timespec deadline{};
  if (timed) {
    AndroidTimespec timeout{};
    memcpy(&timeout, timeout_pointer, sizeof(timeout));
    if (absolute_timeout) {
      if (timeout.seconds < 0 || timeout.nanoseconds < 0 ||
          timeout.nanoseconds >= 1000000000LL ||
          timeout.seconds > std::numeric_limits<time_t>::max()) {
        return Fail(kAndroidEinval);
      }
      deadline.tv_sec = static_cast<time_t>(timeout.seconds);
      deadline.tv_nsec = static_cast<long>(timeout.nanoseconds);
    } else if (!MonotonicDeadline(timeout, &deadline)) {
      return Fail(kAndroidEinval);
    }
  }

  os_unfair_lock_lock(&g_wait_lock);
  InitializeWaitEntries();
  if (__atomic_load_n(address, __ATOMIC_SEQ_CST) != expected) {
    os_unfair_lock_unlock(&g_wait_lock);
    return Fail(kAndroidEagain);
  }
  WaitEntry *entry = FindWaitEntry(address, true);
  if (entry == nullptr) {
    os_unfair_lock_unlock(&g_wait_lock);
    return Fail(kAndroidEagain);
  }
  ++entry->waiters;
  os_unfair_lock_unlock(&g_wait_lock);

  long result = 0;
  for (;;) {
    kern_return_t wait_status = KERN_SUCCESS;
    if (timed) {
      timespec remaining{};
      if (!RemainingUntil(timeout_clock, deadline, &remaining)) {
        wait_status = KERN_OPERATION_TIMED_OUT;
      } else {
        const mach_timespec_t timeout{
            static_cast<unsigned int>(remaining.tv_sec),
            static_cast<clock_res_t>(remaining.tv_nsec)};
        wait_status = semaphore_timedwait(entry->semaphore, timeout);
      }
    } else {
      wait_status = semaphore_wait(entry->semaphore);
    }

    os_unfair_lock_lock(&g_wait_lock);
    if (entry->wake_tokens != 0) {
      --entry->wake_tokens;
      if (wait_status != KERN_SUCCESS) {
        const mach_timespec_t no_wait{0, 0};
        (void)semaphore_timedwait(entry->semaphore, no_wait);
      }
      break;
    }
    if (wait_status == KERN_SUCCESS || wait_status == KERN_ABORTED) {
      os_unfair_lock_unlock(&g_wait_lock);
      continue;
    }
    result = Fail(wait_status == KERN_OPERATION_TIMED_OUT ? kAndroidEtimedout
                                                          : kAndroidEinval);
    break;
  }
  --entry->waiters;
  ReleaseEmptyEntry(entry);
  os_unfair_lock_unlock(&g_wait_lock);
  if (debug) {
    char name[64]{};
    (void)pthread_getname_np(pthread_self(), name, sizeof(name));
    DebugPrintf(
                 "DARWIN futex: wait leave thread=%s address=%p result=%ld "
                 "current=%d\n",
                 name, static_cast<void *>(address), result,
                 __atomic_load_n(address, __ATOMIC_RELAXED));
  }
  return result;
}

long FutexWake(const uint64_t *arguments) {
  auto *address = reinterpret_cast<int32_t *>(arguments[1]);
  const int32_t requested = static_cast<int32_t>(arguments[3]);
  const bool debug = g_debug_futex;
  if (address == nullptr || (reinterpret_cast<uintptr_t>(address) & 3) != 0 ||
      requested < 0) {
    return Fail(kAndroidEinval);
  }
  if (!IsReadableRange(address, sizeof(*address))) {
    return Fail(kAndroidEfault);
  }
  os_unfair_lock_lock(&g_wait_lock);
  InitializeWaitEntries();
  WaitEntry *entry = FindWaitEntry(address, false);
  if (entry == nullptr) {
    os_unfair_lock_unlock(&g_wait_lock);
    if (debug) {
      char name[64]{};
      (void)pthread_getname_np(pthread_self(), name, sizeof(name));
      DebugPrintf(
                   "DARWIN futex: wake thread=%s address=%p requested=%d "
                   "selected=0 no-entry current=%d\n",
                   name, static_cast<void *>(address), requested,
                   __atomic_load_n(address, __ATOMIC_RELAXED));
    }
    return 0;
  }
  if (entry->wake_tokens > entry->waiters)
    std::abort();
  const size_t eligible = entry->waiters - entry->wake_tokens;
  const size_t selected = std::min(static_cast<size_t>(requested), eligible);
  entry->wake_tokens += selected;
  for (size_t index = 0; index < selected; ++index)
    (void)semaphore_signal(entry->semaphore);
  os_unfair_lock_unlock(&g_wait_lock);
  if (debug) {
    char name[64]{};
    (void)pthread_getname_np(pthread_self(), name, sizeof(name));
    DebugPrintf(
                 "DARWIN futex: wake thread=%s address=%p requested=%d "
                 "selected=%zu current=%d\n",
                 name, static_cast<void *>(address), requested, selected,
                 __atomic_load_n(address, __ATOMIC_RELAXED));
  }
  return static_cast<long>(selected);
}

long Futex(const uint64_t *arguments) {
  const uint32_t operation = static_cast<uint32_t>(arguments[2]);
  const uint32_t command = operation & kFutexCommandMask;
  const uint32_t flags = operation & ~kFutexCommandMask;
  if ((flags & ~(kFutexPrivateFlag | kFutexClockRealtime)) != 0)
    return Fail(kAndroidEinval);
  if (command == kFutexWait && (flags & kFutexClockRealtime) == 0)
    return FutexWait(arguments, false, CLOCK_MONOTONIC);
  if (command == kFutexWaitBitset &&
      static_cast<uint32_t>(arguments[6]) == kFutexBitsetMatchAny) {
    return FutexWait(arguments, true,
                     (operation & kFutexClockRealtime) != 0 ? CLOCK_REALTIME
                                                            : CLOCK_MONOTONIC);
  }
  if (command == kFutexWake && (flags & kFutexClockRealtime) == 0)
    return FutexWake(arguments);
  if (command == kFutexWakeBitset && (flags & kFutexClockRealtime) == 0 &&
      static_cast<uint32_t>(arguments[6]) == kFutexBitsetMatchAny) {
    return FutexWake(arguments);
  }
  if (g_debug_futex) {
    DebugPrintf(
                 "DARWIN futex: unsupported address=%p operation=%#x "
                 "value=%#llx timeout=%p address2=%p value3=%#llx\n",
                 reinterpret_cast<void *>(arguments[1]), operation,
                 static_cast<unsigned long long>(arguments[3]),
                 reinterpret_cast<void *>(arguments[4]),
                 reinterpret_cast<void *>(arguments[5]),
                 static_cast<unsigned long long>(arguments[6]));
  }
  return Fail(kAndroidEinval);
}

long ReadabilityProbe(const uint64_t *arguments) {
  if (static_cast<int32_t>(arguments[1]) != -1 || arguments[2] == 0 ||
      arguments[3] != 0 || arguments[4] != 8) {
    return Fail(kAndroidEinval);
  }
  return Fail(IsReadableRange(reinterpret_cast<const void *>(arguments[2]), 8)
                  ? kAndroidEinval
                  : kAndroidEfault);
}

} // namespace

extern "C" int aim_bionic_affinity_get(int tid, size_t capacity, void *mask) {
  const int saved_errno = errno;
  const uint64_t arguments[] = {kSchedGetaffinity, static_cast<uint64_t>(tid),
                               capacity, reinterpret_cast<uint64_t>(mask)};
  long result;
  if (capacity != 0 && !IsWritableRange(mask, capacity)) {
    result = Fail(kAndroidEfault);
  } else {
    result = SchedGetaffinity(arguments);
    if (result >= 0 && static_cast<size_t>(result) < capacity)
      std::memset(static_cast<uint8_t *>(mask) + result, 0, capacity - result);
  }
  errno = saved_errno;
  return result < 0 ? -1 : 0;
}

extern "C" int aim_bionic_affinity_set(int tid, size_t capacity, const void *mask) {
  const int saved_errno = errno;
  const uint64_t arguments[] = {kSchedSetaffinity, static_cast<uint64_t>(tid),
                               capacity, reinterpret_cast<uint64_t>(mask)};
  const long result = SchedSetaffinity(arguments);
  errno = saved_errno;
  return static_cast<int>(result);
}

extern "C" long aim_bionic_syscall_captured(const uint64_t *registers,
                                                   const uint8_t *stack) {
  const int saved_errno = errno;
  if (g_debug_syscall) {
    DebugPrintf("DARWIN syscall entry regs=%p stack=%p nr=%llu\n",
                 static_cast<const void *>(registers),
                 static_cast<const void *>(stack),
                 registers == nullptr ? 0ULL : static_cast<unsigned long long>(registers[0]));
  }
  long result = -1;
  if (registers == nullptr || stack == nullptr) {
    result = Fail(kAndroidEinval);
  } else if (registers[0] == kGettid) {
    result = GetTid();
  } else if (registers[0] == kGetpid) {
    result = GetPid();
  } else if (registers[0] == kTgkill) {
    result = Tgkill(registers);
  } else if (registers[0] == kFutex) {
    result = Futex(registers);
  } else if (registers[0] == kSchedSetaffinity) {
    result = SchedSetaffinity(registers);
    if (g_debug_syscall)
      DebugPrintf("DARWIN syscall sched_setaffinity tid=%llu size=%llu mask=%p result=%ld\n",
                   static_cast<unsigned long long>(registers[1]),
                   static_cast<unsigned long long>(registers[2]),
                   reinterpret_cast<const void*>(registers[3]), result);
  } else if (registers[0] == kSchedGetaffinity) {
    result = SchedGetaffinity(registers);
    if (g_debug_syscall)
      DebugPrintf("DARWIN syscall sched_getaffinity tid=%llu size=%llu mask=%p result=%ld\n",
                   static_cast<unsigned long long>(registers[1]),
                   static_cast<unsigned long long>(registers[2]),
                   reinterpret_cast<const void*>(registers[3]), result);
  } else if (registers[0] == kRtSigprocmask) {
    result = ReadabilityProbe(registers);
  } else if (registers[0] == kRtTgSigqueueinfo) {
    result = RtTgSigqueueinfo(registers);
  } else if (registers[0] == kGetrandom) {
    result = GetRandom(registers);
  } else {
    result = Fail(kAndroidEnosys);
  }
  errno = saved_errno;
  return result;
}

extern "C" size_t
aim_bionic_syscall_waiter_count(const int32_t *address) {
  const int saved_errno = errno;
  os_unfair_lock_lock(&g_wait_lock);
  InitializeWaitEntries();
  const WaitEntry *entry = FindWaitEntry(address, false);
  const size_t result = entry == nullptr ? 0 : entry->waiters;
  os_unfair_lock_unlock(&g_wait_lock);
  errno = saved_errno;
  return result;
}

extern "C" int aim_bionic_gettid() {
  const int saved_errno = errno;
  const long result = GetTid();
  errno = saved_errno;
  return static_cast<int>(result);
}

extern "C" void
aim_bionic_syscall_spurious_wake(const int32_t *address) {
  const int saved_errno = errno;
  os_unfair_lock_lock(&g_wait_lock);
  InitializeWaitEntries();
  WaitEntry *entry = FindWaitEntry(address, false);
  if (entry != nullptr) {
    for (size_t index = 0; index < entry->waiters; ++index)
      (void)semaphore_signal(entry->semaphore);
  }
  os_unfair_lock_unlock(&g_wait_lock);
  errno = saved_errno;
}

extern "C" AimBionicSyscallFunction
aim_bionic_syscall_resolve(const char *soname, const char *symbol,
                                  const char *version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      strcmp(soname, "libc.so") != 0 || strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
  if (strcmp(symbol, "gettid") == 0) {
    return reinterpret_cast<AimBionicSyscallFunction>(
        aim_bionic_gettid);
  }
  if (strcmp(symbol, "syscall") != 0)
    return nullptr;
  return reinterpret_cast<AimBionicSyscallFunction>(
      aim_bionic_syscall);
}

extern "C" const char *
aim_bionic_syscall_capability(const char *capability) {
  if (capability == nullptr)
    return "invalid-capability";
  if (strcmp(capability, "android-aapcs64-varargs-capture") == 0 ||
      strcmp(capability, "provider-owned-stable-tid") == 0 ||
      strcmp(capability, "rt-tgsigqueueinfo-thread-delivery") == 0 ||
      strcmp(capability, "host-csprng-getrandom") == 0 ||
      strcmp(capability, "futex-private-wait-wake") == 0 ||
      strcmp(capability, "libunwind-readability-probe") == 0) {
    return "supported";
  }
  return "unsupported";
}
