#include "darwin_art_bionic_pthread.h"

#include <pthread.h>
#include <signal.h>

#include <atomic>
#include <array>
#include <climits>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <new>
#include <string_view>
#include <chrono>
#include <unordered_map>
#include <unordered_set>

extern "C" void darwin_art_bionic_errno_store(int32_t android_errno);
extern "C" __attribute__((weak)) int darwin_art_attach_native_thread(void) {
  return 0;
}
extern "C" __attribute__((weak)) void darwin_art_detach_native_thread(void) {}

namespace {
thread_local bool g_art_provider_attachment = false;

void DetachArtProviderThread(void*) {
  if (g_art_provider_attachment && darwin_art_detach_native_thread != nullptr) {
    darwin_art_detach_native_thread();
    g_art_provider_attachment = false;
  }
}

static_assert(sizeof(DarwinArtAndroidPthread) == 8);
static_assert(sizeof(DarwinArtAndroidPthreadKey) == 4);
static_assert(sizeof(DarwinArtAndroidPthreadOnce) == 4);
static_assert(sizeof(DarwinArtAndroidPthreadMutexAttr) == 8);
static_assert(sizeof(DarwinArtAndroidPthreadAttr) == 56);
static_assert(sizeof(DarwinArtAndroidPthreadMutex) == 40);
static_assert(sizeof(DarwinArtAndroidPthreadCond) == 48);
static_assert(sizeof(DarwinArtAndroidPthreadRwlock) == 56);

constexpr int kAndroidEsrch = 3;
constexpr int kAndroidEagain = 11;
constexpr int kAndroidEnomem = 12;
constexpr int kAndroidEbusy = 16;
constexpr int kAndroidEinval = 22;
constexpr int kAndroidEoverflow = 75;
constexpr unsigned kAndroidSemValueMax = UINT32_C(0x3fffffff);
constexpr int kAndroidEdeadlk = 35;
constexpr int kAndroidEnotsup = 95;
constexpr int kAndroidEtimedout = 110;
constexpr uint32_t kAndroidKeyValid = UINT32_C(0x80000000);
constexpr uint32_t kAndroidKeySlots = 128;
constexpr int kAndroidDestructorIterations = 4;
constexpr uint16_t kAndroidMutexDestroyed = UINT16_C(0xffff);
constexpr uint16_t kAndroidMutexSharedMask = UINT16_C(0x2000);
constexpr uint16_t kAndroidMutexTypeShift = 14;
constexpr int64_t kAndroidMutexAttrTypeMask = INT64_C(0x000f);
constexpr int64_t kAndroidMutexAttrSharedMask = INT64_C(0x0010);
constexpr int64_t kAndroidMutexAttrProtocolMask = INT64_C(0x0020);
constexpr int64_t kAndroidMutexAttrKnownMask = INT64_C(0x003f);
constexpr uint32_t kAndroidCondSharedMask = UINT32_C(0x0001);
constexpr uint32_t kAndroidCondClockMask = UINT32_C(0x0002);
constexpr uint32_t kAndroidCondCounterStep = UINT32_C(0x0004);
constexpr uint32_t kAndroidCondDestroyed = UINT32_C(0xdeadc04d);
constexpr uint32_t kAndroidRwlockPendingReaders = UINT32_C(0x00000001);
constexpr uint32_t kAndroidRwlockPendingWriters = UINT32_C(0x00000002);
constexpr uint32_t kAndroidRwlockReaderStep = UINT32_C(0x00000004);
constexpr uint32_t kAndroidRwlockWriterOwned = UINT32_C(0x80000000);
constexpr size_t kAndroidRwlockMaxReaders = (UINT32_C(1) << 29) - 1;
constexpr int32_t kOnceNotStarted = 0;
constexpr int32_t kOnceUnderway = 1;
constexpr int32_t kOnceComplete = 2;

int AndroidError(int error) {
  if (error == 0) return 0;
  if (error == EBUSY) return kAndroidEbusy;
  if (error == EINVAL) return kAndroidEinval;
  if (error == ENOMEM) return kAndroidEnomem;
  if (error == EAGAIN) return kAndroidEagain;
  if (error == EDEADLK) return kAndroidEdeadlk;
  if (error == ETIMEDOUT) return kAndroidEtimedout;
  if (error == EPERM) return 1;
  if (error == ESRCH) return kAndroidEsrch;
  if (error == ENOTSUP) return kAndroidEnotsup;
  return kAndroidEinval;
}

int HostSignal(int android_signal) {
  // This must remain the same one-to-one map used by process-state's signal
  // trampoline so pthread_kill and sigaction agree on the guest signo.
  static constexpr unsigned char kMap[32] = {
      0,        SIGHUP,  SIGINT,  SIGQUIT, SIGILL,  SIGTRAP,   SIGABRT,
      SIGBUS,   SIGFPE,  SIGKILL, SIGUSR1, SIGSEGV, SIGUSR2,   SIGPIPE,
      SIGALRM,  SIGTERM, SIGEMT,  SIGCHLD, SIGCONT, SIGSTOP,   SIGTSTP,
      SIGTTIN,  SIGTTOU, SIGURG,  SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF,
      SIGWINCH, SIGIO,   SIGINFO, SIGSYS};
  return android_signal > 0 && android_signal < 32 ? kMap[android_signal] : 0;
}

int AndroidSignal(int host_signal) {
  for (int android_signal = 1; android_signal < 32; ++android_signal) {
    if (HostSignal(android_signal) == host_signal) return android_signal;
  }
  return 0;
}

// Resolve this opt-in once, before guest threads can receive signals. getenv
// takes a libc lock on Darwin: a suspended thread can hold it while the sender
// needs pthread_kill to deliver the restart signal. Even a disabled diagnostic
// must not acquire that lock on the signal/mask path.
const bool kTracePthreadSignals =
    std::getenv("DARWIN_ART_DEBUG_PTHREAD_SIGNALS") != nullptr;

void TraceSignalMask(const char* phase, uint64_t token) {
  if (!kTracePthreadSignals) return;
  static std::atomic<unsigned> observed{0};
  if (observed.fetch_add(1) >= 150) return;
  sigset_t mask{};
  struct sigaction action{};
  pthread_sigmask(SIG_SETMASK, nullptr, &mask);
  sigaction(SIGINFO, nullptr, &action);
  std::fprintf(stderr, "DARWIN pthread mask phase=%s token=%llu mask=%x siginfo_blocked=%d handler=%p flags=%x\n",
      phase, static_cast<unsigned long long>(token), mask,
      sigismember(&mask, SIGINFO), reinterpret_cast<void*>(action.sa_handler), action.sa_flags);
}

struct KeyEntry {
  uint32_t slot{};
  uint64_t generation{};
  DarwinArtAndroidTlsDestructor destructor{};
};

struct KeySlot {
  uint64_t generation{};
  std::shared_ptr<KeyEntry> active;
};

struct ThreadTlsValue {
  uint64_t generation{};
  void* value{};
};

struct ThreadTlsState {
  ThreadTlsValue values[kAndroidKeySlots];
  // A host/ART-created thread is registered lazily by pthread_self().  Keep
  // only its token here so the host TLS destructor can retire the external
  // mapping after guest TLS destructors have run; the registry owns the
  // ThreadEntry itself.
  DarwinArtAndroidPthread foreign_thread_token{};
};

struct MutexEntry {
  enum class Kind { kNormal = 0, kRecursive = 1, kErrorcheck = 2 };
  pthread_mutex_t host{};
  std::mutex lifecycle_mutex;
  std::condition_variable lifecycle_condition;
  enum class Lifecycle { kAlive, kDestroying, kDestroyed };
  Lifecycle lifecycle{Lifecycle::kAlive};
  size_t active_operations{};
  bool host_initialized{};
  Kind kind{Kind::kNormal};

  ~MutexEntry() {
    if (host_initialized) {
      pthread_mutex_destroy(&host);
    }
  }
};

struct OnceEntry {
  std::mutex mutex;
  std::condition_variable condition;
  int state{kOnceNotStarted};
};

struct CondEntry {
  pthread_cond_t host{};
  std::mutex lifecycle_mutex;
  std::condition_variable lifecycle_condition;
  enum class Lifecycle { kAlive, kDestroying, kDestroyed };
  Lifecycle lifecycle{Lifecycle::kAlive};
  size_t active_operations{};
  size_t waiters{};
  bool monotonic{};
  bool host_initialized{};

  ~CondEntry() {
    if (host_initialized) pthread_cond_destroy(&host);
  }
};

struct RwlockEntry {
  std::mutex mutex;
  std::condition_variable readers_condition;
  std::condition_variable writers_condition;
  enum class Lifecycle { kAlive, kDestroyed };
  Lifecycle lifecycle{Lifecycle::kAlive};
  size_t active_readers{};
  size_t pending_readers{};
  size_t pending_writers{};
  DarwinArtAndroidPthread writer_owner{};
};

struct ThreadEntry {
  enum class JoinState {
    kNotJoined,
    kExitedNotJoined,
    kJoined,
    kDetached,
  };
  std::mutex mutex;
  std::condition_variable startup_condition;
  DarwinArtAndroidPthread token{};
  pthread_t host{};
  bool provider_owned{true};
  bool published{};
  bool host_exited{};
  bool host_detached{};
  JoinState join_state{JoinState::kNotJoined};
  DarwinArtAndroidThreadRoutine routine{};
  void* argument{};
  void* return_value{};
};

struct ConditionShard {
  std::mutex mutex;
  std::unordered_map<DarwinArtAndroidPthreadCond*, std::shared_ptr<CondEntry>>
      entries;
};

constexpr size_t kConditionShardCount = 64;

size_t ConditionShardIndex(const DarwinArtAndroidPthreadCond* address) {
  uintptr_t value = reinterpret_cast<uintptr_t>(address);
  value ^= value >> 17;
  value *= UINT64_C(0xed5ad4bb);
  value ^= value >> 11;
  return value % kConditionShardCount;
}

struct ProviderState {
  std::mutex keys_mutex;
  std::mutex mutexes_mutex;
  std::mutex once_mutex;
  std::mutex rwlocks_mutex;
  std::mutex threads_mutex;
  KeySlot key_slots[kAndroidKeySlots];
  std::unordered_set<ThreadTlsState*> thread_tls_states;
  std::unordered_map<DarwinArtAndroidPthreadMutex*, std::shared_ptr<MutexEntry>>
      mutexes;
  std::unordered_map<DarwinArtAndroidPthreadOnce*, std::shared_ptr<OnceEntry>>
      once_controls;
  std::array<ConditionShard, kConditionShardCount> conditions;
  std::unordered_map<DarwinArtAndroidPthreadRwlock*,
                     std::shared_ptr<RwlockEntry>>
      rwlocks;
  std::unordered_map<DarwinArtAndroidPthread, std::shared_ptr<ThreadEntry>>
      threads;
};

ProviderState& State() {
  // Intentionally process-lifetime: Darwin TLS destructors can run after
  // ordinary C++ static destruction has begun.
  static ProviderState* state = new ProviderState();
  return *state;
}

thread_local std::unordered_map<DarwinArtAndroidPthreadMutex*,
                                std::shared_ptr<MutexEntry>>
    mutex_lookup_cache;
thread_local std::unordered_map<DarwinArtAndroidPthreadCond*,
                                std::shared_ptr<CondEntry>>
    cond_lookup_cache;

void RemoveThreadEntry(const std::shared_ptr<ThreadEntry>& entry) {
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.threads_mutex);
  const auto found = state.threads.find(entry->token);
  if (found != state.threads.end() && found->second.get() == entry.get()) {
    state.threads.erase(found);
  }
}

std::shared_ptr<ThreadEntry> FindThreadEntry(DarwinArtAndroidPthread token) {
  ProviderState& state = State();
  std::shared_ptr<ThreadEntry> entry;
  {
    std::lock_guard<std::mutex> lock(state.threads_mutex);
    const auto found = state.threads.find(token);
    if (found == state.threads.end()) return nullptr;
    entry = found->second;
  }
  std::lock_guard<std::mutex> lock(entry->mutex);
  return entry->published ? entry : nullptr;
}

void RetireForeignThreadEntry(DarwinArtAndroidPthread token);

std::atomic<uint64_t>& NextThreadToken() {
  static std::atomic<uint64_t> value{1};
  return value;
}

thread_local uint64_t current_thread_token = 0;

struct CleanupRecord {
  CleanupRecord* previous;
  void (*routine)(void*);
  void* argument;
};
thread_local CleanupRecord* current_cleanup = nullptr;

pthread_key_t& HostThreadTlsKey() {
  static pthread_key_t key{};
  return key;
}

pthread_once_t& HostThreadTlsKeyOnce() {
  static pthread_once_t once = PTHREAD_ONCE_INIT;
  return once;
}

pthread_key_t& OwnedThreadCleanupKey() {
  static pthread_key_t key{};
  return key;
}

pthread_once_t& OwnedThreadCleanupKeyOnce() {
  static pthread_once_t once = PTHREAD_ONCE_INIT;
  return once;
}

thread_local ThreadTlsState* tls_destructor_state = nullptr;

void HostThreadTlsDestructor(void* opaque);

void OwnedThreadCleanup(void* opaque) {
  auto* owner = static_cast<std::shared_ptr<ThreadEntry>*>(opaque);
  std::shared_ptr<ThreadEntry> entry = *owner;
  delete owner;
  bool remove = false;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    entry->host_exited = true;
    remove = entry->join_state == ThreadEntry::JoinState::kDetached &&
             entry->host_detached;
  }
  if (remove) RemoveThreadEntry(entry);
}

void InitializeOwnedThreadCleanupKey() {
  const int result =
      pthread_key_create(&OwnedThreadCleanupKey(), &OwnedThreadCleanup);
  if (result != 0) std::abort();
}

void InitializeHostThreadTlsKey() {
  const int result =
      pthread_key_create(&HostThreadTlsKey(), &HostThreadTlsDestructor);
  if (result != 0) std::abort();
}

void* HostOwnedThreadStart(void* opaque) {
  std::unique_ptr<std::shared_ptr<ThreadEntry>> owner(
      static_cast<std::shared_ptr<ThreadEntry>*>(opaque));
  std::shared_ptr<ThreadEntry> entry = *owner;
  pthread_once(&OwnedThreadCleanupKeyOnce(), &InitializeOwnedThreadCleanupKey);
  if (pthread_setspecific(OwnedThreadCleanupKey(), owner.get()) != 0) {
    std::abort();
  }
  owner.release();
  {
    std::unique_lock<std::mutex> lock(entry->mutex);
    while (!entry->published) entry->startup_condition.wait(lock);
  }
  current_thread_token = static_cast<uint64_t>(entry->token);
  TraceSignalMask("before-ART-attach", current_thread_token);
  if (darwin_art_attach_native_thread != nullptr) {
    g_art_provider_attachment = darwin_art_attach_native_thread() > 0;
  }
  TraceSignalMask("after-ART-attach", current_thread_token);
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN pthread thread-start token=%llu routine=%p argument=%p\n",
                 static_cast<unsigned long long>(entry->token),
                 reinterpret_cast<void*>(entry->routine), entry->argument);
    if (entry->argument != nullptr) {
      uintptr_t words[16] = {};
      std::memcpy(words, entry->argument, sizeof(words));
      std::fprintf(stderr,
                   "DARWIN pthread thread-argument token=%llu "
                   "q0=%#llx q1=%#llx q2=%#llx q3=%#llx q4=%#llx q5=%#llx "
                   "q6=%#llx q7=%#llx q8=%#llx q9=%#llx q10=%#llx q11=%#llx "
                   "q12=%#llx q13=%#llx q14=%#llx q15=%#llx\n",
                   static_cast<unsigned long long>(entry->token),
                   static_cast<unsigned long long>(words[0]),
                   static_cast<unsigned long long>(words[1]),
                   static_cast<unsigned long long>(words[2]),
                   static_cast<unsigned long long>(words[3]),
                   static_cast<unsigned long long>(words[4]),
                   static_cast<unsigned long long>(words[5]),
                   static_cast<unsigned long long>(words[6]),
                   static_cast<unsigned long long>(words[7]),
                   static_cast<unsigned long long>(words[8]),
                   static_cast<unsigned long long>(words[9]),
                   static_cast<unsigned long long>(words[10]),
                   static_cast<unsigned long long>(words[11]),
                   static_cast<unsigned long long>(words[12]),
                   static_cast<unsigned long long>(words[13]),
                   static_cast<unsigned long long>(words[14]),
                   static_cast<unsigned long long>(words[15]));
    }
  }
  void* result = nullptr;
  pthread_cleanup_push(&DetachArtProviderThread, nullptr);
  result = entry->routine(entry->argument);
  pthread_cleanup_pop(1);
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    char thread_name[64] = {};
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    std::fprintf(stderr,
                 "DARWIN pthread thread-exit token=%llu thread=%s routine=%p "
                 "result=%p\n",
                 static_cast<unsigned long long>(entry->token), thread_name,
                 reinterpret_cast<void*>(entry->routine), result);
  }
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    entry->return_value = result;
    if (entry->join_state == ThreadEntry::JoinState::kNotJoined) {
      entry->join_state = ThreadEntry::JoinState::kExitedNotJoined;
    }
  }
  return result;
}

ThreadTlsState* GetThreadTlsState(bool create) {
  if (tls_destructor_state != nullptr) return tls_destructor_state;
  pthread_once(&HostThreadTlsKeyOnce(), &InitializeHostThreadTlsKey);
  auto* thread_state =
      static_cast<ThreadTlsState*>(pthread_getspecific(HostThreadTlsKey()));
  if (thread_state != nullptr || !create) return thread_state;
  thread_state = new (std::nothrow) ThreadTlsState{};
  if (thread_state == nullptr) return nullptr;
  if (pthread_setspecific(HostThreadTlsKey(), thread_state) != 0) {
    delete thread_state;
    return nullptr;
  }
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  state.thread_tls_states.insert(thread_state);
  return thread_state;
}

DarwinArtAndroidPthread CurrentThreadToken() {
  if (current_thread_token == 0) {
    ThreadTlsState* thread_state = nullptr;
    try {
      thread_state = GetThreadTlsState(true);
    } catch (...) {
      // pthread_self has no failure return. Do not let allocator exceptions
      // cross the C ABI or return an identity that other threads cannot find.
      std::abort();
    }
    if (thread_state != nullptr && thread_state->foreign_thread_token != 0) {
      current_thread_token = thread_state->foreign_thread_token;
    } else if (thread_state != nullptr) {
      try {
        auto entry = std::make_shared<ThreadEntry>();
        entry->provider_owned = false;
        entry->host = pthread_self();
        entry->published = true;
        ProviderState& state = State();
        std::lock_guard<std::mutex> lock(state.threads_mutex);
        for (;;) {
          const uint64_t raw_token =
              NextThreadToken().fetch_add(1, std::memory_order_relaxed);
          if (raw_token == 0) continue;
          entry->token = static_cast<DarwinArtAndroidPthread>(raw_token);
          const auto [position, inserted] =
              state.threads.emplace(entry->token, entry);
          (void)position;
          if (inserted) break;
        }
        thread_state->foreign_thread_token = entry->token;
        current_thread_token = static_cast<uint64_t>(entry->token);
      } catch (...) {
        // pthread_self has no failure return. Never publish an unregistered
        // token: a foreign GC thread receiving ESRCH would otherwise be
        // mistaken for an exited thread. Allocation failure is process-fatal
        // rather than silently violating the identity contract.
        std::abort();
      }
    } else {
      // Registration requires the host TLS state. Do not return a token which
      // cannot be found by another thread.
      std::abort();
    }
  }
  return static_cast<DarwinArtAndroidPthread>(current_thread_token);
}

void RetireForeignThreadEntry(DarwinArtAndroidPthread token) {
  std::shared_ptr<ThreadEntry> entry = FindThreadEntry(token);
  if (entry == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (entry->provider_owned) return;
    entry->host_exited = true;
    entry->published = false;
  }
  RemoveThreadEntry(entry);
}

bool DecodeKey(DarwinArtAndroidPthreadKey key, uint32_t* slot_out) {
  const uint32_t raw = static_cast<uint32_t>(key);
  if ((raw & kAndroidKeyValid) == 0) return false;
  const uint32_t slot = raw & ~kAndroidKeyValid;
  if (slot >= kAndroidKeySlots) return false;
  *slot_out = slot;
  return true;
}

void HostThreadTlsDestructor(void* opaque) {
  auto* thread_state = static_cast<ThreadTlsState*>(opaque);
  tls_destructor_state = thread_state;
  // Bionic runs at most PTHREAD_DESTRUCTOR_ITERATIONS passes. Clear each value
  // before invoking its callback so reinsertion schedules a later pass.
  for (int iteration = 0; iteration < kAndroidDestructorIterations;
       ++iteration) {
    struct Pending {
      DarwinArtAndroidTlsDestructor destructor;
      void* value;
    };
    Pending pending[kAndroidKeySlots];
    size_t count = 0;
    {
      ProviderState& state = State();
      std::lock_guard<std::mutex> lock(state.keys_mutex);
      for (uint32_t slot = 0; slot < kAndroidKeySlots; ++slot) {
        const std::shared_ptr<KeyEntry>& entry = state.key_slots[slot].active;
        ThreadTlsValue& value = thread_state->values[slot];
        if (entry != nullptr && value.value != nullptr &&
            value.generation == entry->generation &&
            entry->destructor != nullptr) {
          pending[count++] = Pending{entry->destructor, value.value};
          value.value = nullptr;
        }
      }
    }
    if (count == 0) break;
    for (size_t index = 0; index < count; ++index) {
      // Android arm64 and Darwin arm64 agree for this fixed one-pointer
      // callback. Arbitrary callback signatures remain out of scope.
      pending[index].destructor(pending[index].value);
    }
  }
  const DarwinArtAndroidPthread foreign_token =
      thread_state->foreign_thread_token;
  if (foreign_token != 0) {
    // Retire only after all emulated Android TLS destructors have run, so a
    // destructor can still use pthread_self()/pthread_kill(self, 0).
    RetireForeignThreadEntry(foreign_token);
    thread_state->foreign_thread_token = 0;
    current_thread_token = 0;
  }
  tls_destructor_state = nullptr;
  ProviderState& state = State();
  {
    std::lock_guard<std::mutex> lock(state.keys_mutex);
    state.thread_tls_states.erase(thread_state);
  }
  delete thread_state;
}

uint16_t AndroidMutexState(const DarwinArtAndroidPthreadMutex* mutex) {
  uint16_t value = 0;
  std::memcpy(&value, mutex, sizeof(value));
  return value;
}

void SetAndroidMutexState(DarwinArtAndroidPthreadMutex* mutex, uint16_t value) {
  std::memcpy(mutex, &value, sizeof(value));
}

int ParseAndroidMutexAttributes(
    const DarwinArtAndroidPthreadMutexAttr* attributes,
    MutexEntry::Kind* kind_out) {
  if (attributes == nullptr) {
    *kind_out = MutexEntry::Kind::kNormal;
    return 0;
  }
  int64_t value = 0;
  std::memcpy(&value, attributes, sizeof(value));
  if (value == -1) return kAndroidEinval;
  if ((value & kAndroidMutexAttrSharedMask) != 0 ||
      (value & kAndroidMutexAttrProtocolMask) != 0) {
    return kAndroidEnotsup;
  }
  if ((value & ~kAndroidMutexAttrKnownMask) != 0) return kAndroidEinval;
  switch (value & kAndroidMutexAttrTypeMask) {
    case 0:
      *kind_out = MutexEntry::Kind::kNormal;
      return 0;
    case 1:
      *kind_out = MutexEntry::Kind::kRecursive;
      return 0;
    case 2:
      *kind_out = MutexEntry::Kind::kErrorcheck;
      return 0;
    default:
      return kAndroidEinval;
  }
}

int InitializeHostMutex(const std::shared_ptr<MutexEntry>& entry,
                        MutexEntry::Kind kind) {
  pthread_mutexattr_t attributes;
  const int initialized = pthread_mutexattr_init(&attributes);
  if (initialized != 0) return AndroidError(initialized);
  int result = 0;
  if (kind == MutexEntry::Kind::kRecursive) {
    result = pthread_mutexattr_settype(&attributes, PTHREAD_MUTEX_RECURSIVE);
  } else if (kind == MutexEntry::Kind::kErrorcheck) {
    result = pthread_mutexattr_settype(&attributes, PTHREAD_MUTEX_ERRORCHECK);
  }
  if (result == 0) {
    result = pthread_mutex_init(&entry->host, &attributes);
    if (result == 0) entry->host_initialized = true;
  }
  const int destroyed = pthread_mutexattr_destroy(&attributes);
  if (result == 0 && destroyed != 0) result = destroyed;
  if (result != 0) return AndroidError(result);
  entry->kind = kind;
  return 0;
}

uint32_t AndroidCondState(const DarwinArtAndroidPthreadCond* cond) {
  uint32_t value = 0;
  std::memcpy(&value, cond, sizeof(value));
  return value;
}

void SetAndroidCondState(DarwinArtAndroidPthreadCond* cond, uint32_t value) {
  std::memcpy(cond, &value, sizeof(value));
}

void SetAndroidCondWaiters(DarwinArtAndroidPthreadCond* cond, uint32_t value) {
  std::memcpy(reinterpret_cast<unsigned char*>(cond) + sizeof(uint32_t),
              &value,
              sizeof(value));
}

void SetAndroidRwlockVisible(DarwinArtAndroidPthreadRwlock* visible,
                             const RwlockEntry& entry) {
  uint32_t state = 0;
  if (entry.writer_owner != 0) {
    state |= kAndroidRwlockWriterOwned;
  } else {
    state |= static_cast<uint32_t>(entry.active_readers) *
             kAndroidRwlockReaderStep;
  }
  if (entry.pending_readers != 0) state |= kAndroidRwlockPendingReaders;
  if (entry.pending_writers != 0) state |= kAndroidRwlockPendingWriters;
  const int32_t writer_tid = static_cast<int32_t>(entry.writer_owner);
  std::memcpy(visible, &state, sizeof(state));
  std::memcpy(reinterpret_cast<unsigned char*>(visible) + sizeof(state),
              &writer_tid,
              sizeof(writer_tid));
}

std::shared_ptr<MutexEntry> FindOrCreateMutex(
    DarwinArtAndroidPthreadMutex* mutex,
    int* error_out) {
  if (mutex == nullptr) {
    *error_out = kAndroidEinval;
    return nullptr;
  }
  const auto cached_mutex = mutex_lookup_cache.find(mutex);
  if (cached_mutex != mutex_lookup_cache.end()) {
    const std::shared_ptr<MutexEntry> cached = cached_mutex->second;
    std::lock_guard<std::mutex> lifecycle_lock(cached->lifecycle_mutex);
    if (cached->lifecycle != MutexEntry::Lifecycle::kDestroyed ||
        AndroidMutexState(mutex) == kAndroidMutexDestroyed) {
      *error_out = 0;
      return cached;
    }
  }
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.mutexes_mutex);
  const auto found = state.mutexes.find(mutex);
  if (found != state.mutexes.end()) {
    const std::shared_ptr<MutexEntry> existing = found->second;
    std::lock_guard<std::mutex> lifecycle_lock(existing->lifecycle_mutex);
    if (existing->lifecycle != MutexEntry::Lifecycle::kDestroyed ||
        AndroidMutexState(mutex) == kAndroidMutexDestroyed) {
      *error_out = 0;
      mutex_lookup_cache[mutex] = existing;
      return existing;
    }
    // libc++'s Android std::mutex uses the all-zero static initializer. A
    // freshly constructed object may therefore reuse the address of a mutex
    // whose prior lifetime ended without calling pthread_mutex_init. Do not
    // let the side table's destroyed generation poison that new lifetime.
  }
  const uint16_t visible = AndroidMutexState(mutex);
  if (visible == kAndroidMutexDestroyed) {
    *error_out = kAndroidEbusy;
    return nullptr;
  }
  if ((visible & kAndroidMutexSharedMask) != 0) {
    *error_out = kAndroidEnotsup;
    return nullptr;
  }
  const uint16_t type = visible >> kAndroidMutexTypeShift;
  if (type > 2) {
    *error_out = kAndroidEnotsup;
    return nullptr;
  }
  const uint16_t expected = static_cast<uint16_t>(type << kAndroidMutexTypeShift);
  if (visible != expected) {
    *error_out = kAndroidEinval;
    return nullptr;
  }
  const unsigned char* bytes = reinterpret_cast<const unsigned char*>(mutex);
  for (size_t index = sizeof(uint16_t); index < sizeof(*mutex); ++index) {
    if (bytes[index] != 0) {
      *error_out = kAndroidEinval;
      return nullptr;
    }
  }
  auto entry = std::make_shared<MutexEntry>();
  const int result = InitializeHostMutex(
      entry, static_cast<MutexEntry::Kind>(type));
  if (result != 0 && std::getenv("DARWIN_ART_PTHREAD_TRACE") != nullptr) {
    *error_out = result;
    return nullptr;
  }
  state.mutexes[mutex] = entry;
  mutex_lookup_cache[mutex] = entry;
  *error_out = 0;
  return entry;
}

std::shared_ptr<CondEntry> FindOrCreateCond(
    DarwinArtAndroidPthreadCond* cond,
    int* error_out) {
  if (cond == nullptr) {
    *error_out = kAndroidEinval;
    return nullptr;
  }
  const auto cached_cond = cond_lookup_cache.find(cond);
  if (cached_cond != cond_lookup_cache.end()) {
    const std::shared_ptr<CondEntry> cached = cached_cond->second;
    std::lock_guard<std::mutex> lifecycle_lock(cached->lifecycle_mutex);
    if (cached->lifecycle != CondEntry::Lifecycle::kDestroyed ||
        AndroidCondState(cond) == kAndroidCondDestroyed) {
      *error_out = 0;
      return cached;
    }
  }
  ProviderState& state = State();
  ConditionShard& shard = state.conditions[ConditionShardIndex(cond)];
  std::lock_guard<std::mutex> lock(shard.mutex);
  const auto found = shard.entries.find(cond);
  if (found != shard.entries.end()) {
    const std::shared_ptr<CondEntry> existing = found->second;
    std::lock_guard<std::mutex> lifecycle_lock(existing->lifecycle_mutex);
    if (existing->lifecycle != CondEntry::Lifecycle::kDestroyed ||
        AndroidCondState(cond) == kAndroidCondDestroyed) {
      *error_out = 0;
      cond_lookup_cache[cond] = existing;
      return existing;
    }
    // Match the mutex generation rule above for a zero-initialized condition
    // variable constructed at an address from an earlier object lifetime.
  }
  const uint32_t visible = AndroidCondState(cond);
  if (visible == kAndroidCondDestroyed) {
    *error_out = kAndroidEbusy;
    return nullptr;
  }
  if ((visible & kAndroidCondSharedMask) != 0) {
    *error_out = kAndroidEnotsup;
    return nullptr;
  }
  if ((visible & ~(kAndroidCondSharedMask | kAndroidCondClockMask)) != 0) {
    *error_out = kAndroidEinval;
    return nullptr;
  }
  const unsigned char* bytes = reinterpret_cast<const unsigned char*>(cond);
  for (size_t index = sizeof(uint32_t); index < sizeof(*cond); ++index) {
    if (bytes[index] != 0) {
      *error_out = kAndroidEinval;
      return nullptr;
    }
  }
  auto entry = std::make_shared<CondEntry>();
  const int result = pthread_cond_init(&entry->host, nullptr);
  if (result != 0 && std::getenv("DARWIN_ART_PTHREAD_TRACE") != nullptr) {
    *error_out = AndroidError(result);
    return nullptr;
  }
  entry->host_initialized = true;
  entry->monotonic = (visible & kAndroidCondClockMask) != 0;
  shard.entries[cond] = entry;
  cond_lookup_cache[cond] = entry;
  *error_out = 0;
  return entry;
}

int BeginCondWait(const std::shared_ptr<CondEntry>& cond,
                  const std::shared_ptr<MutexEntry>& mutex) {
  std::scoped_lock lock(cond->lifecycle_mutex, mutex->lifecycle_mutex);
  if (cond->lifecycle != CondEntry::Lifecycle::kAlive ||
      mutex->lifecycle != MutexEntry::Lifecycle::kAlive) {
    return kAndroidEbusy;
  }
  ++cond->active_operations;
  ++cond->waiters;
  ++mutex->active_operations;
  return 0;
}

void EndCondWait(DarwinArtAndroidPthreadCond* visible_cond,
                 const std::shared_ptr<CondEntry>& cond,
                 const std::shared_ptr<MutexEntry>& mutex) {
  std::scoped_lock lock(cond->lifecycle_mutex, mutex->lifecycle_mutex);
  --cond->active_operations;
  --cond->waiters;
  --mutex->active_operations;
  SetAndroidCondWaiters(visible_cond, static_cast<uint32_t>(cond->waiters));
  cond->lifecycle_condition.notify_all();
  mutex->lifecycle_condition.notify_all();
}

timespec MonotonicRelativeTimeout(const timespec& absolute) {
  timespec now{};
  clock_gettime(CLOCK_MONOTONIC, &now);
  timespec relative{};
  relative.tv_sec = absolute.tv_sec - now.tv_sec;
  relative.tv_nsec = absolute.tv_nsec - now.tv_nsec;
  if (relative.tv_nsec < 0) {
    --relative.tv_sec;
    relative.tv_nsec += 1000000000L;
  }
  if (relative.tv_sec < 0) return timespec{0, 0};
  return relative;
}

int CondWait(DarwinArtAndroidPthreadCond* visible_cond,
             DarwinArtAndroidPthreadMutex* visible_mutex,
             const timespec* absolute_timeout) {
  if (absolute_timeout != nullptr &&
      (absolute_timeout->tv_sec < 0 || absolute_timeout->tv_nsec < 0 ||
       absolute_timeout->tv_nsec >= 1000000000L)) {
    return kAndroidEinval;
  }
  int error = 0;
  std::shared_ptr<CondEntry> cond = FindOrCreateCond(visible_cond, &error);
  if (cond == nullptr) return error;
  std::shared_ptr<MutexEntry> mutex = FindOrCreateMutex(visible_mutex, &error);
  if (mutex == nullptr) return error;
  error = BeginCondWait(cond, mutex);
  if (error != 0) return error;
  {
    std::lock_guard<std::mutex> lock(cond->lifecycle_mutex);
    SetAndroidCondWaiters(visible_cond, static_cast<uint32_t>(cond->waiters));
  }
  int result = 0;
  if (absolute_timeout == nullptr) {
    result = pthread_cond_wait(&cond->host, &mutex->host);
  } else if (cond->monotonic) {
    const timespec relative = MonotonicRelativeTimeout(*absolute_timeout);
    result = pthread_cond_timedwait_relative_np(
        &cond->host, &mutex->host, &relative);
  } else {
    result = pthread_cond_timedwait(
        &cond->host, &mutex->host, absolute_timeout);
  }
  EndCondWait(visible_cond, cond, mutex);
  const int android_result = AndroidError(result);
  if (android_result != 0 &&
      std::getenv("DARWIN_ART_PTHREAD_TRACE") != nullptr) {
    static std::atomic<uint32_t> failure_count{0};
    const uint32_t ordinal =
        failure_count.fetch_add(1, std::memory_order_relaxed);
    if (ordinal < 32) {
      std::fprintf(stderr,
                   "DARWIN pthread cond_wait failure cond=%p mutex=%p "
                   "timed=%d monotonic=%d host=%d android=%d deadline=%lld.%09ld\n",
                   static_cast<void*>(visible_cond),
                   static_cast<void*>(visible_mutex),
                   absolute_timeout != nullptr,
                   cond->monotonic,
                   result,
                   android_result,
                   absolute_timeout == nullptr
                       ? 0LL
                       : static_cast<long long>(absolute_timeout->tv_sec),
                   absolute_timeout == nullptr ? 0L : absolute_timeout->tv_nsec);
    }
  }
  return android_result;
}

int CondPulse(DarwinArtAndroidPthreadCond* visible_cond, bool broadcast) {
  int error = 0;
  std::shared_ptr<CondEntry> cond = FindOrCreateCond(visible_cond, &error);
  if (cond == nullptr) return error;
  {
    std::lock_guard<std::mutex> lock(cond->lifecycle_mutex);
    if (cond->lifecycle != CondEntry::Lifecycle::kAlive) return kAndroidEbusy;
    ++cond->active_operations;
    if (cond->waiters != 0) {
      SetAndroidCondState(
          visible_cond,
          AndroidCondState(visible_cond) + kAndroidCondCounterStep);
    }
  }
  const int result = broadcast ? pthread_cond_broadcast(&cond->host)
                               : pthread_cond_signal(&cond->host);
  {
    std::lock_guard<std::mutex> lock(cond->lifecycle_mutex);
    --cond->active_operations;
    if (cond->active_operations == 0) cond->lifecycle_condition.notify_all();
  }
  return AndroidError(result);
}

std::shared_ptr<RwlockEntry> FindOrCreateRwlock(
    DarwinArtAndroidPthreadRwlock* rwlock,
    int* error_out) {
  if (rwlock == nullptr) {
    *error_out = kAndroidEinval;
    return nullptr;
  }
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.rwlocks_mutex);
  const auto found = state.rwlocks.find(rwlock);
  if (found != state.rwlocks.end()) {
    const std::shared_ptr<RwlockEntry> existing = found->second;
    std::lock_guard<std::mutex> entry_lock(existing->mutex);
    if (existing->lifecycle != RwlockEntry::Lifecycle::kDestroyed) {
      *error_out = 0;
      return existing;
    }
    // Bionic leaves an idle destroyed rwlock as zero bytes, so a subsequent
    // zero-initialized object at the same address is a new side-table
    // generation. Use-after-destroy is undefined and need not stay poisoned.
  }
  const unsigned char* bytes = reinterpret_cast<const unsigned char*>(rwlock);
  // Bionic's pshared byte is offset 8. This slice accepts only the all-zero
  // PTHREAD_RWLOCK_INITIALIZER and never fabricates process-shared behavior.
  if (bytes[8] != 0) {
    *error_out = kAndroidEnotsup;
    return nullptr;
  }
  for (size_t index = 0; index < sizeof(*rwlock); ++index) {
    if (bytes[index] != 0) {
      *error_out = kAndroidEinval;
      return nullptr;
    }
  }
  auto entry = std::make_shared<RwlockEntry>();
  state.rwlocks[rwlock] = entry;
  *error_out = 0;
  return entry;
}

template <typename Operation>
int WithLiveMutex(const std::shared_ptr<MutexEntry>& entry,
                  Operation operation,
                  bool permit_while_destroying = false) {
  {
    std::unique_lock<std::mutex> lock(entry->lifecycle_mutex);
    while (!permit_while_destroying &&
           entry->lifecycle == MutexEntry::Lifecycle::kDestroying) {
      entry->lifecycle_condition.wait(lock);
    }
    if (entry->lifecycle == MutexEntry::Lifecycle::kDestroyed) {
      return kAndroidEbusy;
    }
    ++entry->active_operations;
  }
  const int result = operation(&entry->host);
  {
    std::lock_guard<std::mutex> lock(entry->lifecycle_mutex);
    --entry->active_operations;
    if (entry->active_operations == 0) {
      entry->lifecycle_condition.notify_all();
    }
  }
  return AndroidError(result);
}

}  // namespace

extern "C" DarwinArtAndroidPthread darwin_art_bionic_pthread_self(void) {
  return CurrentThreadToken();
}

extern "C" int darwin_art_bionic_pthread_create(
    DarwinArtAndroidPthread* thread,
    const void* android_attributes,
    DarwinArtAndroidThreadRoutine routine,
    void* argument) {
  if (thread == nullptr || routine == nullptr) return kAndroidEinval;
  const auto* attributes =
      static_cast<const DarwinArtAndroidPthreadAttr*>(android_attributes);
  std::shared_ptr<ThreadEntry> entry;
  try {
    entry = std::make_shared<ThreadEntry>();
  } catch (const std::bad_alloc&) {
    return kAndroidEnomem;
  } catch (...) {
    return kAndroidEagain;
  }
  entry->routine = routine;
  entry->argument = argument;
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    char thread_name[64] = {};
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    std::fprintf(stderr,
                 "DARWIN pthread create thread=%s routine=%p argument=%p "
                 "caller=%p stack=%zu guard=%zu\n",
                 thread_name, reinterpret_cast<void*>(routine), argument,
                 __builtin_return_address(0),
                 attributes == nullptr ? 0 : attributes->stack_size,
                 attributes == nullptr ? 0 : attributes->guard_size);
  }
  auto* owner =
      new (std::nothrow) std::shared_ptr<ThreadEntry>(entry);
  if (owner == nullptr) return kAndroidEnomem;
  try {
    ProviderState& state = State();
    std::lock_guard<std::mutex> lock(state.threads_mutex);
    for (;;) {
      const uint64_t raw_token =
          NextThreadToken().fetch_add(1, std::memory_order_relaxed);
      if (raw_token == 0) continue;
      entry->token = static_cast<DarwinArtAndroidPthread>(raw_token);
      const auto [position, inserted] =
          state.threads.emplace(entry->token, entry);
      (void)position;
      if (inserted) break;
    }
  } catch (const std::bad_alloc&) {
    delete owner;
    return kAndroidEnomem;
  } catch (...) {
    delete owner;
    return kAndroidEagain;
  }
  pthread_attr_t host_attributes;
  pthread_attr_t* host_attributes_pointer = nullptr;
  if (attributes != nullptr) {
    if (pthread_attr_init(&host_attributes) != 0) {
      RemoveThreadEntry(entry);
      delete owner;
      return kAndroidEinval;
    }
    host_attributes_pointer = &host_attributes;
    if (attributes->stack_size != 0 &&
        pthread_attr_setstacksize(&host_attributes, attributes->stack_size) != 0) {
      pthread_attr_destroy(&host_attributes);
      RemoveThreadEntry(entry);
      delete owner;
      return kAndroidEinval;
    }
    pthread_attr_setguardsize(&host_attributes, attributes->guard_size);
  }
  const int result = pthread_create(&entry->host, host_attributes_pointer,
                                    &HostOwnedThreadStart, owner);
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN pthread create-result routine=%p result=%d host=%p\n",
                 reinterpret_cast<void*>(routine), result,
                 reinterpret_cast<void*>(entry->host));
  }
  if (host_attributes_pointer != nullptr) pthread_attr_destroy(&host_attributes);
  if (result != 0) {
    RemoveThreadEntry(entry);
    delete owner;
    return AndroidError(result);
  }
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    *thread = entry->token;
    entry->published = true;
  }
  entry->startup_condition.notify_one();
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_init(
    DarwinArtAndroidPthreadAttr* attr) {
  if (attr == nullptr) return kAndroidEinval;
  std::memset(attr, 0, sizeof(*attr));
  attr->guard_size = 4096;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_destroy(
    DarwinArtAndroidPthreadAttr* attr) {
  if (attr == nullptr) return kAndroidEinval;
  std::memset(attr, 0, sizeof(*attr));
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_getguardsize(
    const DarwinArtAndroidPthreadAttr* attr, size_t* size) {
  if (attr == nullptr || size == nullptr) return kAndroidEinval;
  *size = attr->guard_size;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_getstack(
    const DarwinArtAndroidPthreadAttr* attr, void** address, size_t* size) {
  if (attr == nullptr || address == nullptr || size == nullptr)
    return kAndroidEinval;
  *address = attr->stack_base;
  *size = attr->stack_size;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_getstacksize(
    const DarwinArtAndroidPthreadAttr* attr, size_t* size) {
  if (attr == nullptr || size == nullptr) return kAndroidEinval;
  *size = attr->stack_size;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_getschedparam(
    const DarwinArtAndroidPthreadAttr* attr, sched_param* param) {
  if (attr == nullptr || param == nullptr) return kAndroidEinval;
  param->sched_priority = attr->sched_priority;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_setdetachstate(
    DarwinArtAndroidPthreadAttr* attr, int state) {
  if (attr == nullptr || (state != 0 && state != 1)) return kAndroidEinval;
  attr->flags = (attr->flags & ~UINT32_C(1)) | static_cast<uint32_t>(state);
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_setguardsize(
    DarwinArtAndroidPthreadAttr* attr, size_t size) {
  if (attr == nullptr) return kAndroidEinval;
  attr->guard_size = size;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_setschedparam(
    DarwinArtAndroidPthreadAttr* attr, const sched_param* param) {
  if (attr == nullptr || param == nullptr) return kAndroidEinval;
  attr->sched_priority = param->sched_priority;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_setschedpolicy(
    DarwinArtAndroidPthreadAttr* attr, int policy) {
  if (attr == nullptr) return kAndroidEinval;
  attr->sched_policy = policy;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_attr_setscope(
    DarwinArtAndroidPthreadAttr* attr, int scope) {
  return attr != nullptr && scope == 0 ? 0 : kAndroidEnotsup;
}

extern "C" int darwin_art_bionic_pthread_attr_setstacksize(
    DarwinArtAndroidPthreadAttr* attr, size_t size) {
  if (attr == nullptr || size < 16384) return kAndroidEinval;
  attr->stack_size = size;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_getattr_np(
    DarwinArtAndroidPthread token, DarwinArtAndroidPthreadAttr* attr) {
  if (attr == nullptr) return kAndroidEinval;
  pthread_t host;
  if (token == CurrentThreadToken()) {
    host = pthread_self();
  } else {
    auto entry = FindThreadEntry(token);
    if (entry == nullptr) return kAndroidEsrch;
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (!entry->published || entry->host_exited) return kAndroidEsrch;
    std::memset(attr, 0, sizeof(*attr));
    attr->stack_size = pthread_get_stacksize_np(entry->host);
    void* stack_top = pthread_get_stackaddr_np(entry->host);
    attr->stack_base = stack_top == nullptr
                           ? nullptr
                           : static_cast<void*>(
                                 static_cast<char*>(stack_top) - attr->stack_size);
    attr->guard_size = 4096;
    return 0;
  }
  std::memset(attr, 0, sizeof(*attr));
  attr->stack_size = pthread_get_stacksize_np(host);
  // Darwin reports the initial stack address (the high, downward-growing
  // end), while Android pthread_attr_getstack() reports the low base of the
  // mapped interval.  Publishing Darwin's value unchanged makes Android
  // callers such as V8 believe every frame is outside the central stack.
  void* stack_top = pthread_get_stackaddr_np(host);
  attr->stack_base = stack_top == nullptr
                         ? nullptr
                         : static_cast<void*>(
                               static_cast<char*>(stack_top) - attr->stack_size);
  attr->guard_size = 4096;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_gettid_np(
    DarwinArtAndroidPthread token) {
  if (token == CurrentThreadToken() || FindThreadEntry(token) != nullptr)
    return static_cast<int>(static_cast<uint64_t>(token) & UINT32_C(0x7fffffff));
  return -1;
}

extern "C" int darwin_art_bionic_pthread_condattr_init(uint32_t* attr) {
  if (attr == nullptr) return kAndroidEinval;
  *attr = 0;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_condattr_destroy(uint32_t* attr) {
  if (attr == nullptr) return kAndroidEinval;
  *attr = UINT32_MAX;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_condattr_setclock(uint32_t* attr,
                                                            int clock_id) {
  if (attr == nullptr || (clock_id != 0 && clock_id != 1))
    return kAndroidEinval;
  *attr = clock_id == 1 ? kAndroidCondClockMask : 0;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutexattr_setprotocol(
    DarwinArtAndroidPthreadMutexAttr* attributes, int protocol) {
  if (attributes == nullptr) return kAndroidEinval;
  return protocol == 0 ? 0 : kAndroidEnotsup;
}

extern "C" int darwin_art_bionic_pthread_equal(
    DarwinArtAndroidPthread left, DarwinArtAndroidPthread right) {
  return left == right;
}

extern "C" int darwin_art_bionic_pthread_getschedparam(
    DarwinArtAndroidPthread token, int* policy, sched_param* param) {
  if (policy == nullptr || param == nullptr) return kAndroidEinval;
  if (token == CurrentThreadToken())
    return AndroidError(pthread_getschedparam(pthread_self(), policy, param));
  auto entry = FindThreadEntry(token);
  if (entry == nullptr) return kAndroidEsrch;
  std::lock_guard<std::mutex> lock(entry->mutex);
  if (!entry->published || entry->host_exited) return kAndroidEsrch;
  return AndroidError(pthread_getschedparam(entry->host, policy, param));
}

extern "C" int darwin_art_bionic_pthread_setschedparam(
    DarwinArtAndroidPthread token, int policy, const sched_param* param) {
  if (param == nullptr) return kAndroidEinval;
  if (token == CurrentThreadToken())
    return AndroidError(pthread_setschedparam(pthread_self(), policy, param));
  auto entry = FindThreadEntry(token);
  if (entry == nullptr) return kAndroidEsrch;
  std::lock_guard<std::mutex> lock(entry->mutex);
  if (!entry->published || entry->host_exited) return kAndroidEsrch;
  return AndroidError(pthread_setschedparam(entry->host, policy, param));
}

extern "C" [[noreturn]] void darwin_art_bionic_pthread_exit(void* value) {
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    char thread_name[64] = {};
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    std::fprintf(stderr,
                 "DARWIN pthread explicit-exit token=%llu thread=%s value=%p "
                 "caller=%p\n",
                 static_cast<unsigned long long>(CurrentThreadToken()),
                 thread_name, value, __builtin_return_address(0));
  }
  pthread_exit(value);
  __builtin_unreachable();
}

extern "C" int darwin_art_bionic_pthread_setname_np(
    DarwinArtAndroidPthread token, const char* name) {
  if (name == nullptr) return kAndroidEinval;
  if (token != CurrentThreadToken()) return kAndroidEnotsup;
  TraceSignalMask(name, token);
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN pthread setname token=%llu name=%s caller=%p\n",
                 static_cast<unsigned long long>(token), name,
                 __builtin_return_address(0));
  }
  return AndroidError(pthread_setname_np(name));
}

extern "C" int darwin_art_bionic_pthread_kill(
    DarwinArtAndroidPthread token, int signal_number) {
  if (signal_number < 0 || signal_number >= 32) return kAndroidEinval;
  const int host_signal = signal_number == 0 ? 0 : HostSignal(signal_number);
  if (signal_number != 0 && host_signal == 0) return kAndroidEinval;
  if (token == CurrentThreadToken())
    return AndroidError(pthread_kill(pthread_self(), host_signal));
  auto entry = FindThreadEntry(token);
  if (entry == nullptr) return kAndroidEsrch;
  std::lock_guard<std::mutex> lock(entry->mutex);
  if (!entry->published || entry->host_exited) return kAndroidEsrch;
  const int status = AndroidError(pthread_kill(entry->host, host_signal));
  if (kTracePthreadSignals) {
    static std::atomic<unsigned> observed{0};
    if (observed.fetch_add(1) < 50) {
      char name[64]{};
      pthread_getname_np(entry->host, name, sizeof(name));
      std::fprintf(stderr, "DARWIN pthread signal token=%llu name=%s guest=%d host=%d result=%d\n",
          static_cast<unsigned long long>(token), name, signal_number, host_signal, status);
    }
  }
  return status;
}

extern "C" int darwin_art_bionic_pthread_sigmask(
    int android_how, const uint64_t* android_set, uint64_t* android_old_set) {
  int host_how;
  switch (android_how) {
    case 0: host_how = SIG_BLOCK; break;
    case 1: host_how = SIG_UNBLOCK; break;
    case 2: host_how = SIG_SETMASK; break;
    default: return kAndroidEinval;
  }
  sigset_t host_set;
  sigset_t host_old;
  sigemptyset(&host_set);
  if (android_set != nullptr) {
    for (int signal_number = 1; signal_number < 32; ++signal_number) {
      if ((*android_set & (UINT64_C(1) << (signal_number - 1))) != 0 &&
          HostSignal(signal_number) != 0) {
        sigaddset(&host_set, HostSignal(signal_number));
      }
    }
  }
  const int result = pthread_sigmask(host_how,
                                     android_set == nullptr ? nullptr : &host_set,
                                     android_old_set == nullptr ? nullptr : &host_old);
  if (result != 0) return AndroidError(result);
  TraceSignalMask("guest-sigmask", current_thread_token);
  if (android_old_set != nullptr) {
    uint64_t result_set = 0;
    for (int host_signal = 1; host_signal < NSIG; ++host_signal) {
      const int android_signal = AndroidSignal(host_signal);
      if (android_signal != 0 && sigismember(&host_old, host_signal) == 1)
        result_set |= UINT64_C(1) << (android_signal - 1);
    }
    *android_old_set = result_set;
  }
  return 0;
}

extern "C" void darwin_art_bionic___pthread_cleanup_push(
    CleanupRecord* record, void (*routine)(void*), void* argument) {
  if (record == nullptr) return;
  record->previous = current_cleanup;
  record->routine = routine;
  record->argument = argument;
  current_cleanup = record;
}

extern "C" void darwin_art_bionic___pthread_cleanup_pop(
    CleanupRecord* record, int execute) {
  if (record == nullptr || current_cleanup != record) return;
  current_cleanup = record->previous;
  if (execute != 0 && record->routine != nullptr) record->routine(record->argument);
}

extern "C" int darwin_art_bionic_pthread_cond_init(
    DarwinArtAndroidPthreadCond* cond, const void* attr) {
  if (cond == nullptr) return kAndroidEinval;
  uint32_t visible = 0;
  if (attr != nullptr) {
    std::memcpy(&visible, attr, sizeof(visible));
    if (visible == UINT32_MAX ||
        (visible & ~(kAndroidCondSharedMask | kAndroidCondClockMask)) != 0) {
      return kAndroidEinval;
    }
    if ((visible & kAndroidCondSharedMask) != 0) return kAndroidEnotsup;
  }
  std::memset(cond, 0, sizeof(*cond));
  SetAndroidCondState(cond, visible);
  int error = 0;
  return FindOrCreateCond(cond, &error) == nullptr ? error : 0;
}

extern "C" int darwin_art_bionic_pthread_join(
    DarwinArtAndroidPthread token,
    void** return_value) {
  if (token == CurrentThreadToken()) return kAndroidEdeadlk;
  std::shared_ptr<ThreadEntry> entry = FindThreadEntry(token);
  if (entry == nullptr) return kAndroidEsrch;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (!entry->provider_owned) return kAndroidEinval;
    if (entry->join_state == ThreadEntry::JoinState::kDetached ||
        entry->join_state == ThreadEntry::JoinState::kJoined) {
      return kAndroidEinval;
    }
    entry->join_state = ThreadEntry::JoinState::kJoined;
  }
  void* host_return = nullptr;
  const int result = pthread_join(entry->host, &host_return);
  if (result != 0) return AndroidError(result);
  if (return_value != nullptr) *return_value = host_return;
  RemoveThreadEntry(entry);
  return 0;
}

extern "C" int darwin_art_bionic_pthread_detach(
    DarwinArtAndroidPthread token) {
  std::shared_ptr<ThreadEntry> entry = FindThreadEntry(token);
  if (entry == nullptr) return kAndroidEsrch;
  bool collect_exited = false;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (!entry->provider_owned) return kAndroidEinval;
    if (entry->join_state == ThreadEntry::JoinState::kNotJoined) {
      entry->join_state = ThreadEntry::JoinState::kDetached;
    } else if (entry->join_state ==
               ThreadEntry::JoinState::kExitedNotJoined) {
      entry->join_state = ThreadEntry::JoinState::kJoined;
      collect_exited = true;
    } else {
      return kAndroidEinval;
    }
  }
  if (collect_exited) {
    const int result = pthread_join(entry->host, nullptr);
    if (result != 0) return AndroidError(result);
    RemoveThreadEntry(entry);
    return 0;
  }
  const int result = pthread_detach(entry->host);
  bool remove = false;
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (result == 0) {
      entry->host_detached = true;
      remove = entry->host_exited;
    } else {
      entry->join_state = entry->host_exited
                              ? ThreadEntry::JoinState::kExitedNotJoined
                              : ThreadEntry::JoinState::kNotJoined;
    }
  }
  if (remove) RemoveThreadEntry(entry);
  return AndroidError(result);
}

extern "C" int darwin_art_bionic_pthread_key_create(
    DarwinArtAndroidPthreadKey* key,
    DarwinArtAndroidTlsDestructor destructor) {
  if (key == nullptr) return kAndroidEinval;
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  for (uint32_t slot = 0; slot < kAndroidKeySlots; ++slot) {
    KeySlot& key_slot = state.key_slots[slot];
    if (key_slot.active != nullptr) continue;
    auto entry = std::make_shared<KeyEntry>();
    entry->slot = slot;
    entry->generation = ++key_slot.generation;
    entry->destructor = destructor;
    key_slot.active = entry;
    *key = static_cast<int32_t>(kAndroidKeyValid | slot);
    return 0;
  }
  return kAndroidEagain;
}

extern "C" int darwin_art_bionic_pthread_key_delete(
    DarwinArtAndroidPthreadKey key) {
  uint32_t slot = 0;
  if (!DecodeKey(key, &slot)) return kAndroidEinval;
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  if (state.key_slots[slot].active == nullptr) return kAndroidEinval;
  // POSIX deletion does not call destructors. Per-thread values retain only a
  // generation number and raw application value, never an owning KeyEntry.
  // Slot reuse therefore cannot create a cycle or a stale host-key callback.
  state.key_slots[slot].active.reset();
  return 0;
}

extern "C" void* darwin_art_bionic_pthread_getspecific(
    DarwinArtAndroidPthreadKey key) {
  uint32_t slot = 0;
  if (!DecodeKey(key, &slot)) return nullptr;
  ThreadTlsState* thread_state = GetThreadTlsState(false);
  if (thread_state == nullptr) return nullptr;
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  const std::shared_ptr<KeyEntry>& entry = state.key_slots[slot].active;
  if (entry == nullptr ||
      thread_state->values[slot].generation != entry->generation) {
    return nullptr;
  }
  return thread_state->values[slot].value;
}

extern "C" int darwin_art_bionic_pthread_setspecific(
    DarwinArtAndroidPthreadKey key,
    const void* value) {
  uint32_t slot = 0;
  if (!DecodeKey(key, &slot)) return kAndroidEinval;
  ThreadTlsState* thread_state = GetThreadTlsState(value != nullptr);
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  const std::shared_ptr<KeyEntry>& entry = state.key_slots[slot].active;
  if (entry == nullptr) return kAndroidEinval;
  if (thread_state == nullptr) {
    // Clearing a valid key with no state is already satisfied.
    return value == nullptr ? 0 : kAndroidEnomem;
  }
  thread_state->values[slot].generation = entry->generation;
  thread_state->values[slot].value = const_cast<void*>(value);
  return 0;
}

extern "C" int darwin_art_bionic_pthread_once(
    DarwinArtAndroidPthreadOnce* once,
    DarwinArtAndroidOnceRoutine routine) {
  if (once == nullptr || routine == nullptr) return kAndroidEinval;
  const int32_t visible = __atomic_load_n(once, __ATOMIC_ACQUIRE);
  if (visible == kOnceComplete) return 0;
  if (visible != kOnceNotStarted && visible != kOnceUnderway) {
    return kAndroidEinval;
  }
  ProviderState& state = State();
  std::shared_ptr<OnceEntry> entry;
  {
    std::lock_guard<std::mutex> lock(state.once_mutex);
    const auto found = state.once_controls.find(once);
    if (found == state.once_controls.end()) {
      if (visible != kOnceNotStarted) return kAndroidEinval;
      entry = std::make_shared<OnceEntry>();
      state.once_controls.emplace(once, entry);
    } else {
      entry = found->second;
    }
  }
  std::unique_lock<std::mutex> lock(entry->mutex);
  while (entry->state == kOnceUnderway) entry->condition.wait(lock);
  if (entry->state == kOnceComplete) return 0;
  entry->state = kOnceUnderway;
  __atomic_store_n(once, kOnceUnderway, __ATOMIC_RELEASE);
  lock.unlock();
  routine();
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    const uintptr_t routine_address = reinterpret_cast<uintptr_t>(routine);
    constexpr uintptr_t kUnityCpuOnceOffset = 0x12eb474;
    if (routine_address >= kUnityCpuOnceOffset &&
        ((routine_address - kUnityCpuOnceOffset) & 0x3fff) == 0) {
      const uintptr_t image_base = routine_address - kUnityCpuOnceOffset;
      const int32_t cpu_count =
          *reinterpret_cast<const int32_t*>(image_base + 0x180c728);
      const auto* records = *reinterpret_cast<const uint8_t* const*>(
          image_base + 0x1728140);
      std::fprintf(stderr,
                   "DARWIN pthread Unity CPU once base=%p count=%d "
                   "records=%p active=%u,%u,%u,%u,%u,%u,%u,%u\n",
                   reinterpret_cast<void*>(image_base), cpu_count,
                   static_cast<const void*>(records),
                   records == nullptr ? 0u : records[0x08],
                   records == nullptr ? 0u : records[0x28],
                   records == nullptr ? 0u : records[0x48],
                   records == nullptr ? 0u : records[0x68],
                   records == nullptr ? 0u : records[0x88],
                   records == nullptr ? 0u : records[0xa8],
                   records == nullptr ? 0u : records[0xc8],
                   records == nullptr ? 0u : records[0xe8]);
    }
    constexpr uintptr_t kUnityClassifierOnceOffset = 0x8978ec;
    if (routine_address >= kUnityClassifierOnceOffset &&
        ((routine_address - kUnityClassifierOnceOffset) & 0x3fff) == 0) {
      const uintptr_t image_base = routine_address - kUnityClassifierOnceOffset;
      const auto* records = *reinterpret_cast<const uint8_t* const*>(
          image_base + 0x1728140);
      const uint64_t first =
          *reinterpret_cast<const uint64_t*>(image_base + 0x172c5c8);
      const uint64_t second =
          *reinterpret_cast<const uint64_t*>(image_base + 0x172c5d0);
      std::fprintf(stderr,
                   "DARWIN pthread Unity classifier once base=%p "
                   "records=%p count=%d active=%u,%u,%u,%u,%u,%u,%u,%u "
                   "results=%#llx,%#llx\n",
                   reinterpret_cast<void*>(image_base),
                   static_cast<const void*>(records),
                   records == nullptr ? -1
                                      : *reinterpret_cast<const int32_t*>(records),
                   records == nullptr ? 0u : records[0x08],
                   records == nullptr ? 0u : records[0x28],
                   records == nullptr ? 0u : records[0x48],
                   records == nullptr ? 0u : records[0x68],
                   records == nullptr ? 0u : records[0x88],
                   records == nullptr ? 0u : records[0xa8],
                   records == nullptr ? 0u : records[0xc8],
                   records == nullptr ? 0u : records[0xe8],
                   static_cast<unsigned long long>(first),
                   static_cast<unsigned long long>(second));
      if (records != nullptr) {
        const auto* raw = records + 0x568;
        const auto* raw_text =
            *reinterpret_cast<const char* const*>(raw);
        const uint64_t raw_length =
            *reinterpret_cast<const uint64_t*>(raw + 0x10);
        std::fprintf(stderr,
                     "DARWIN pthread Unity cpuinfo raw=%p len=%llu text='%.*s'\n",
                     static_cast<const void*>(raw_text),
                     static_cast<unsigned long long>(raw_length),
                     static_cast<int>(std::min<uint64_t>(raw_length, 160)),
                     raw_text == nullptr ? "" : raw_text);
        const auto* lines = records + 0x588;
        const auto* entries =
            *reinterpret_cast<const uint8_t* const*>(lines);
        const uint64_t line_count =
            *reinterpret_cast<const uint64_t*>(lines + 0x10);
        std::fprintf(stderr,
                     "DARWIN pthread Unity cpuinfo lines entries=%p count=%llu\n",
                     static_cast<const void*>(entries),
                     static_cast<unsigned long long>(line_count));
        for (uint64_t index = 0;
             entries != nullptr && index < line_count && index < 16; ++index) {
          const auto* text = *reinterpret_cast<const char* const*>(
              entries + index * 0x10);
          const uint64_t length = *reinterpret_cast<const uint64_t*>(
              entries + index * 0x10 + 8);
          std::fprintf(stderr,
                       "DARWIN pthread Unity cpuinfo line[%llu] len=%llu text='%.*s'\n",
                       static_cast<unsigned long long>(index),
                       static_cast<unsigned long long>(length),
                       static_cast<int>(std::min<uint64_t>(length, 120)),
                       text == nullptr ? "" : text);
        }
      }
    }
  }
  lock.lock();
  entry->state = kOnceComplete;
  __atomic_store_n(once, kOnceComplete, __ATOMIC_RELEASE);
  lock.unlock();
  entry->condition.notify_all();
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutexattr_init(
    DarwinArtAndroidPthreadMutexAttr* attributes) {
  if (attributes == nullptr) return kAndroidEinval;
  const int64_t value = 0;
  std::memcpy(attributes, &value, sizeof(value));
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutexattr_destroy(
    DarwinArtAndroidPthreadMutexAttr* attributes) {
  if (attributes == nullptr) return kAndroidEinval;
  const int64_t destroyed = -1;
  std::memcpy(attributes, &destroyed, sizeof(destroyed));
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutexattr_settype(
    DarwinArtAndroidPthreadMutexAttr* attributes,
    int type) {
  if (attributes == nullptr || type < 0 || type > 2) return kAndroidEinval;
  int64_t value = 0;
  std::memcpy(&value, attributes, sizeof(value));
  // POSIX makes use after destroy undefined. Preserve Bionic's -1 guest
  // sentinel but define a safe failure rather than resurrecting its low bits.
  if (value == -1) return kAndroidEinval;
  value = (value & ~kAndroidMutexAttrTypeMask) | type;
  std::memcpy(attributes, &value, sizeof(value));
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutex_init(
    DarwinArtAndroidPthreadMutex* mutex,
    const DarwinArtAndroidPthreadMutexAttr* android_attributes) {
  if (mutex == nullptr) return kAndroidEinval;
  MutexEntry::Kind kind = MutexEntry::Kind::kNormal;
  const int parsed = ParseAndroidMutexAttributes(android_attributes, &kind);
  if (parsed != 0) return parsed;
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.mutexes_mutex);
  // Bionic pthread_mutex_init overwrites the object and never reports EBUSY.
  // Memory freed without pthread_mutex_destroy (BoringSSL per-thread state,
  // for one) is reused by the allocator, so a live side-table entry at this
  // address belongs to a dead lifetime: retire it, so every thread's lookup
  // cache falls through to the new generation, and install a replacement.
  const auto found = state.mutexes.find(mutex);
  if (found != state.mutexes.end()) {
    {
      std::lock_guard<std::mutex> lifecycle_lock(
          found->second->lifecycle_mutex);
      found->second->lifecycle = MutexEntry::Lifecycle::kDestroyed;
    }
    found->second->lifecycle_condition.notify_all();
  }
  auto entry = std::make_shared<MutexEntry>();
  const int result = InitializeHostMutex(entry, kind);
  if (result != 0) return result;
  std::memset(mutex, 0, sizeof(*mutex));
  SetAndroidMutexState(
      mutex,
      static_cast<uint16_t>(static_cast<int>(kind) << kAndroidMutexTypeShift));
  state.mutexes[mutex] = entry;
  return 0;
}

extern "C" int darwin_art_bionic_pthread_mutex_lock(
    DarwinArtAndroidPthreadMutex* mutex) {
  int error = 0;
  std::shared_ptr<MutexEntry> entry = FindOrCreateMutex(mutex, &error);
  const int result =
      entry == nullptr ? error : WithLiveMutex(entry, pthread_mutex_lock);
  if (result != 0 && std::getenv("DARWIN_ART_PTHREAD_TRACE") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN pthread mutex_lock failure mutex=%p visible=0x%04x "
                 "result=%d entry=%p\n",
                 static_cast<void*>(mutex),
                 mutex == nullptr ? 0 : AndroidMutexState(mutex),
                 result,
                 static_cast<void*>(entry.get()));
  }
  return result;
}

extern "C" int darwin_art_bionic_pthread_atfork(
    DarwinArtAndroidForkRoutine prepare, DarwinArtAndroidForkRoutine parent,
    DarwinArtAndroidForkRoutine child) {
  // Android's pthread_atfork ABI uses the same three void(void) callbacks as
  // Darwin. Keep registration in the host pthread subsystem so callbacks run
  // at the real process fork boundary without a guest pthread object.
  return pthread_atfork(prepare, parent, child);
}

extern "C" int darwin_art_bionic_pthread_mutex_trylock(
    DarwinArtAndroidPthreadMutex* mutex) {
  int error = 0;
  std::shared_ptr<MutexEntry> entry = FindOrCreateMutex(mutex, &error);
  return entry == nullptr ? error : WithLiveMutex(entry, pthread_mutex_trylock);
}

extern "C" int darwin_art_bionic_pthread_mutex_unlock(
    DarwinArtAndroidPthreadMutex* mutex) {
  int error = 0;
  std::shared_ptr<MutexEntry> entry = FindOrCreateMutex(mutex, &error);
  // Unlock is the drain path for a lock operation that completed before a
  // concurrent destroy entered kDestroying. Blocking it would deadlock the
  // destroy waiter behind a still-held host mutex.
  return entry == nullptr ? error
                          : WithLiveMutex(entry, pthread_mutex_unlock, true);
}

extern "C" int darwin_art_bionic_pthread_mutex_destroy(
    DarwinArtAndroidPthreadMutex* mutex) {
  int error = 0;
  std::shared_ptr<MutexEntry> entry = FindOrCreateMutex(mutex, &error);
  if (entry == nullptr) return error;
  std::unique_lock<std::mutex> lock(entry->lifecycle_mutex);
  while (entry->lifecycle == MutexEntry::Lifecycle::kDestroying) {
    entry->lifecycle_condition.wait(lock);
  }
  if (entry->lifecycle == MutexEntry::Lifecycle::kDestroyed) {
    return kAndroidEbusy;
  }
  entry->lifecycle = MutexEntry::Lifecycle::kDestroying;
  while (entry->active_operations != 0) {
    entry->lifecycle_condition.wait(lock);
  }
  lock.unlock();
  const int result = pthread_mutex_destroy(&entry->host);
  lock.lock();
  if (result == 0) {
    entry->host_initialized = false;
    entry->lifecycle = MutexEntry::Lifecycle::kDestroyed;
    SetAndroidMutexState(mutex, kAndroidMutexDestroyed);
  } else {
    entry->lifecycle = MutexEntry::Lifecycle::kAlive;
  }
  lock.unlock();
  entry->lifecycle_condition.notify_all();
  return AndroidError(result);
}

extern "C" int darwin_art_bionic_pthread_cond_wait(
    DarwinArtAndroidPthreadCond* cond,
    DarwinArtAndroidPthreadMutex* mutex) {
  const bool debug_waits =
      std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr;
  char thread_name[64] = {};
  if (debug_waits) {
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    const uintptr_t caller =
        reinterpret_cast<uintptr_t>(__builtin_return_address(0));
    std::fprintf(stderr,
                 "DARWIN pthread cond-wait enter thread=%s cond=%p mutex=%p "
                 "caller=%p\n",
                 thread_name, static_cast<void*>(cond),
                 static_cast<void*>(mutex), reinterpret_cast<void*>(caller));
  }
  const int result = CondWait(cond, mutex, nullptr);
  if (debug_waits) {
    std::fprintf(stderr,
                 "DARWIN pthread cond-wait leave thread=%s cond=%p result=%d\n",
                 thread_name, static_cast<void*>(cond), result);
  }
  return result;
}

extern "C" int darwin_art_bionic_pthread_cond_timedwait(
    DarwinArtAndroidPthreadCond* cond,
    DarwinArtAndroidPthreadMutex* mutex,
    const timespec* absolute_timeout) {
  return CondWait(cond, mutex, absolute_timeout);
}

extern "C" int darwin_art_bionic_pthread_cond_signal(
    DarwinArtAndroidPthreadCond* cond) {
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    char thread_name[64] = {};
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    std::fprintf(stderr,
                 "DARWIN pthread cond-signal thread=%s cond=%p caller=%p\n",
                 thread_name, static_cast<void*>(cond),
                 __builtin_return_address(0));
  }
  return CondPulse(cond, false);
}

extern "C" int darwin_art_bionic_pthread_cond_broadcast(
    DarwinArtAndroidPthreadCond* cond) {
  if (std::getenv("DARWIN_ART_DEBUG_PTHREAD_WAITS") != nullptr) {
    char thread_name[64] = {};
    (void)pthread_getname_np(pthread_self(), thread_name, sizeof(thread_name));
    std::fprintf(stderr,
                 "DARWIN pthread cond-broadcast thread=%s cond=%p caller=%p\n",
                 thread_name, static_cast<void*>(cond),
                 __builtin_return_address(0));
  }
  return CondPulse(cond, true);
}

extern "C" int darwin_art_bionic_pthread_cond_destroy(
    DarwinArtAndroidPthreadCond* visible_cond) {
  int error = 0;
  std::shared_ptr<CondEntry> cond = FindOrCreateCond(visible_cond, &error);
  if (cond == nullptr) return error;
  std::unique_lock<std::mutex> lock(cond->lifecycle_mutex);
  if (cond->lifecycle != CondEntry::Lifecycle::kAlive) return kAndroidEbusy;
  // POSIX makes destruction with waiters undefined and Bionic does not detect
  // it. The facade defines a safe Android-errno failure instead of destroying
  // host storage beneath a blocked Android thread.
  if (cond->active_operations != 0 || cond->waiters != 0) return kAndroidEbusy;
  cond->lifecycle = CondEntry::Lifecycle::kDestroying;
  lock.unlock();
  const int result = pthread_cond_destroy(&cond->host);
  lock.lock();
  if (result == 0) {
    cond->host_initialized = false;
    cond->lifecycle = CondEntry::Lifecycle::kDestroyed;
    SetAndroidCondState(visible_cond, kAndroidCondDestroyed);
    SetAndroidCondWaiters(visible_cond, 0);
  } else {
    cond->lifecycle = CondEntry::Lifecycle::kAlive;
  }
  lock.unlock();
  cond->lifecycle_condition.notify_all();
  return AndroidError(result);
}

extern "C" int darwin_art_bionic_pthread_rwlock_rdlock(
    DarwinArtAndroidPthreadRwlock* visible) {
  int error = 0;
  std::shared_ptr<RwlockEntry> rwlock = FindOrCreateRwlock(visible, &error);
  if (rwlock == nullptr) return error;
  const DarwinArtAndroidPthread self = CurrentThreadToken();
  std::unique_lock<std::mutex> lock(rwlock->mutex);
  if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
    return kAndroidEbusy;
  }
  if (rwlock->writer_owner == self) return kAndroidEdeadlk;
  if (rwlock->writer_owner != 0) {
    ++rwlock->pending_readers;
    SetAndroidRwlockVisible(visible, *rwlock);
    rwlock->readers_condition.wait(lock, [&] {
      return rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive ||
             rwlock->writer_owner == 0;
    });
    --rwlock->pending_readers;
    if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
      SetAndroidRwlockVisible(visible, *rwlock);
      return kAndroidEbusy;
    }
  }
  if (rwlock->active_readers == kAndroidRwlockMaxReaders) {
    SetAndroidRwlockVisible(visible, *rwlock);
    return kAndroidEagain;
  }
  ++rwlock->active_readers;
  SetAndroidRwlockVisible(visible, *rwlock);
  return 0;
}

extern "C" int darwin_art_bionic_pthread_rwlock_wrlock(
    DarwinArtAndroidPthreadRwlock* visible) {
  int error = 0;
  std::shared_ptr<RwlockEntry> rwlock = FindOrCreateRwlock(visible, &error);
  if (rwlock == nullptr) return error;
  const DarwinArtAndroidPthread self = CurrentThreadToken();
  std::unique_lock<std::mutex> lock(rwlock->mutex);
  if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
    return kAndroidEbusy;
  }
  if (rwlock->writer_owner == self) return kAndroidEdeadlk;
  if (rwlock->writer_owner != 0 || rwlock->active_readers != 0) {
    ++rwlock->pending_writers;
    SetAndroidRwlockVisible(visible, *rwlock);
    rwlock->writers_condition.wait(lock, [&] {
      return rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive ||
             (rwlock->writer_owner == 0 && rwlock->active_readers == 0);
    });
    --rwlock->pending_writers;
    if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
      SetAndroidRwlockVisible(visible, *rwlock);
      return kAndroidEbusy;
    }
  }
  rwlock->writer_owner = self;
  SetAndroidRwlockVisible(visible, *rwlock);
  return 0;
}

extern "C" int darwin_art_bionic_pthread_rwlock_unlock(
    DarwinArtAndroidPthreadRwlock* visible) {
  int error = 0;
  std::shared_ptr<RwlockEntry> rwlock = FindOrCreateRwlock(visible, &error);
  if (rwlock == nullptr) return error;
  const DarwinArtAndroidPthread self = CurrentThreadToken();
  std::unique_lock<std::mutex> lock(rwlock->mutex);
  if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
    return kAndroidEbusy;
  }
  if (rwlock->writer_owner != 0) {
    if (rwlock->writer_owner != self) return 1;
    rwlock->writer_owner = 0;
  } else if (rwlock->active_readers != 0) {
    // Pinned Bionic tracks only a global reader count, not reader thread ids.
    --rwlock->active_readers;
  } else {
    return 1;
  }
  SetAndroidRwlockVisible(visible, *rwlock);
  const bool wake_writer =
      rwlock->writer_owner == 0 && rwlock->active_readers == 0 &&
      rwlock->pending_writers != 0;
  const bool wake_readers = rwlock->writer_owner == 0 &&
                            rwlock->pending_writers == 0 &&
                            rwlock->pending_readers != 0;
  lock.unlock();
  if (wake_writer) {
    rwlock->writers_condition.notify_one();
  } else if (wake_readers) {
    rwlock->readers_condition.notify_all();
  }
  return 0;
}

extern "C" int darwin_art_bionic_pthread_rwlock_init(
    DarwinArtAndroidPthreadRwlock* visible,
    const DarwinArtAndroidPthreadRwlockAttr* attributes) {
  if (visible == nullptr) return kAndroidEinval;
  if (attributes != nullptr && *attributes != 0) return kAndroidEnotsup;
  std::shared_ptr<RwlockEntry> replacement;
  try {
    replacement = std::make_shared<RwlockEntry>();
  } catch (const std::bad_alloc&) {
    return kAndroidEnomem;
  } catch (...) {
    return kAndroidEagain;
  }
  ProviderState& state = State();
  try {
    std::lock_guard<std::mutex> state_lock(state.rwlocks_mutex);
    const auto found = state.rwlocks.find(visible);
    if (found != state.rwlocks.end()) {
      // Bionic pthread_rwlock_init overwrites the object and never reports
      // EBUSY; a live entry here is a lifetime whose memory was freed without
      // pthread_rwlock_destroy (BoringSSL's CRYPTO_MUTEX_init aborts on any
      // failure). Retire it, waking its waiters, and install a new generation.
      const std::shared_ptr<RwlockEntry> existing = found->second;
      {
        std::lock_guard<std::mutex> entry_lock(existing->mutex);
        existing->lifecycle = RwlockEntry::Lifecycle::kDestroyed;
      }
      existing->readers_condition.notify_all();
      existing->writers_condition.notify_all();
      found->second = replacement;
    } else {
      state.rwlocks.emplace(visible, replacement);
    }
  } catch (const std::bad_alloc&) {
    return kAndroidEnomem;
  } catch (...) {
    return kAndroidEagain;
  }
  visible->opaque[0] = 0;
  for (size_t index = 1; index < sizeof(visible->opaque) / sizeof(visible->opaque[0]);
       ++index) {
    visible->opaque[index] = 0;
  }
  return 0;
}

extern "C" int darwin_art_bionic_pthread_rwlock_destroy(
    DarwinArtAndroidPthreadRwlock* visible) {
  int error = 0;
  std::shared_ptr<RwlockEntry> rwlock = FindOrCreateRwlock(visible, &error);
  if (rwlock == nullptr) return error;
  std::lock_guard<std::mutex> lock(rwlock->mutex);
  if (rwlock->lifecycle != RwlockEntry::Lifecycle::kAlive) {
    return kAndroidEbusy;
  }
  if (rwlock->writer_owner != 0 || rwlock->active_readers != 0 ||
      rwlock->pending_readers != 0 || rwlock->pending_writers != 0) {
    return kAndroidEbusy;
  }
  rwlock->lifecycle = RwlockEntry::Lifecycle::kDestroyed;
  // Bionic pthread_rwlock_destroy leaves an idle object's zero bytes intact.
  SetAndroidRwlockVisible(visible, *rwlock);
  return 0;
}

struct SemEntry {
  std::mutex mutex;
  std::condition_variable condition;
  unsigned value = 0;
  size_t waiters = 0;
  bool destroyed = false;
};
std::mutex g_sem_mutex;
std::unordered_map<void*, std::shared_ptr<SemEntry>> g_semaphores;

int SemFail(int error) {
  darwin_art_bionic_errno_store(error);
  return -1;
}

extern "C" int darwin_art_bionic_sem_init(void* address, int pshared,
                                           unsigned value) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  if (pshared != 0) return SemFail(kAndroidEnotsup);
  if (value > kAndroidSemValueMax) return SemFail(kAndroidEinval);
  auto entry = std::make_shared<SemEntry>();
  entry->value = value;
  std::lock_guard<std::mutex> lock(g_sem_mutex);
  if (g_semaphores.count(address) != 0) return SemFail(kAndroidEinval);
  g_semaphores.emplace(address, std::move(entry));
  return 0;
}

extern "C" int darwin_art_bionic_sem_destroy(void* address) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
    std::lock_guard<std::mutex> sem_lock(entry->mutex);
    if (entry->waiters != 0) return SemFail(kAndroidEbusy);
    entry->destroyed = true;
    g_semaphores.erase(found);
  }
  return 0;
}

extern "C" int darwin_art_bionic_sem_post(void* address) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
  }
  {
    std::lock_guard<std::mutex> lock(entry->mutex);
    if (entry->destroyed) return SemFail(kAndroidEinval);
    if (entry->value == kAndroidSemValueMax)
      return SemFail(kAndroidEoverflow);
    ++entry->value;
  }
  entry->condition.notify_one();
  return 0;
}

extern "C" int darwin_art_bionic_sem_wait(void* address) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
  }
  std::unique_lock<std::mutex> lock(entry->mutex);
  ++entry->waiters;
  entry->condition.wait(lock, [&] { return entry->destroyed || entry->value != 0; });
  --entry->waiters;
  if (entry->destroyed) return SemFail(kAndroidEinval);
  --entry->value;
  return 0;
}

extern "C" int darwin_art_bionic_sem_trywait(void* address) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
  }
  std::lock_guard<std::mutex> lock(entry->mutex);
  if (entry->destroyed) return SemFail(kAndroidEinval);
  if (entry->value == 0) return SemFail(kAndroidEagain);
  --entry->value;
  return 0;
}

extern "C" int darwin_art_bionic_sem_getvalue(void* address, int* value) {
  if (address == nullptr || value == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
  }
  std::lock_guard<std::mutex> lock(entry->mutex);
  if (entry->destroyed) return SemFail(kAndroidEinval);
  *value = static_cast<int>(entry->value);
  return 0;
}

extern "C" int darwin_art_bionic_sem_timedwait(
    void* address, const timespec* absolute_timeout) {
  if (address == nullptr) return SemFail(kAndroidEinval);
  std::shared_ptr<SemEntry> entry;
  {
    std::lock_guard<std::mutex> lock(g_sem_mutex);
    auto found = g_semaphores.find(address);
    if (found == g_semaphores.end()) return SemFail(kAndroidEinval);
    entry = found->second;
  }
  std::unique_lock<std::mutex> lock(entry->mutex);
  if (entry->destroyed) return SemFail(kAndroidEinval);
  // POSIX permits an already-available semaphore to be consumed without
  // inspecting the timeout object. This matters for callers that pass a
  // stale/invalid timespec while racing a posted permit.
  if (entry->value != 0) {
    --entry->value;
    return 0;
  }
  if (absolute_timeout == nullptr || absolute_timeout->tv_sec < 0 ||
      absolute_timeout->tv_nsec < 0 ||
      absolute_timeout->tv_nsec >= 1000000000L)
    return SemFail(kAndroidEinval);
  const auto duration = std::chrono::seconds(absolute_timeout->tv_sec) +
                        std::chrono::nanoseconds(absolute_timeout->tv_nsec);
  const auto deadline = std::chrono::system_clock::time_point(
      std::chrono::duration_cast<std::chrono::system_clock::duration>(duration));
  ++entry->waiters;
  if (!entry->condition.wait_until(
          lock, deadline, [&] { return entry->destroyed || entry->value != 0; }))
  {
    --entry->waiters;
    return SemFail(kAndroidEtimedout);
  }
  --entry->waiters;
  if (entry->destroyed) return SemFail(kAndroidEinval);
  --entry->value;
  return 0;
}

extern "C" void* darwin_art_bionic_pthread_resolve(const char* soname,
                                                    const char* symbol,
                                                    const char* version) {
  const bool protocol_version_alias =
      symbol != nullptr && version != nullptr &&
      std::string_view(symbol) == "pthread_mutexattr_setprotocol" &&
      std::string_view(version) == "LIBC_P";
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::string_view(soname) != "libc.so" ||
      (std::string_view(version) != "LIBC" && !protocol_version_alias)) {
    return nullptr;
  }
  if (std::string_view(symbol) == "sem_init")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_init);
  if (std::string_view(symbol) == "sem_destroy")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_destroy);
  if (std::string_view(symbol) == "sem_post")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_post);
  if (std::string_view(symbol) == "sem_wait")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_wait);
  if (std::string_view(symbol) == "sem_trywait")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_trywait);
  if (std::string_view(symbol) == "sem_getvalue")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_getvalue);
  if (std::string_view(symbol) == "sem_timedwait")
    return reinterpret_cast<void*>(&darwin_art_bionic_sem_timedwait);
  if (std::string_view(symbol) == "__pthread_cleanup_push")
    return reinterpret_cast<void*>(&darwin_art_bionic___pthread_cleanup_push);
  if (std::string_view(symbol) == "__pthread_cleanup_pop")
    return reinterpret_cast<void*>(&darwin_art_bionic___pthread_cleanup_pop);
#define RESOLVE(name)                                                        \
  if (std::string_view(symbol) == "pthread_" #name)                         \
    return reinterpret_cast<void*>(&darwin_art_bionic_pthread_##name)
  RESOLVE(self);
  RESOLVE(create);
  RESOLVE(join);
  RESOLVE(detach);
  RESOLVE(attr_destroy);
  RESOLVE(attr_getguardsize);
  RESOLVE(attr_getstack);
  RESOLVE(attr_getstacksize);
  RESOLVE(attr_getschedparam);
  RESOLVE(attr_init);
  RESOLVE(attr_setdetachstate);
  RESOLVE(attr_setguardsize);
  RESOLVE(attr_setschedparam);
  RESOLVE(attr_setschedpolicy);
  RESOLVE(attr_setscope);
  RESOLVE(attr_setstacksize);
  RESOLVE(equal);
  RESOLVE(getattr_np);
  RESOLVE(gettid_np);
  RESOLVE(getschedparam);
  RESOLVE(setschedparam);
  RESOLVE(exit);
  RESOLVE(kill);
  RESOLVE(setname_np);
  RESOLVE(sigmask);
  if (std::string_view(symbol) == "sigprocmask")
    return reinterpret_cast<void*>(&darwin_art_bionic_pthread_sigmask);
  RESOLVE(key_create);
  RESOLVE(key_delete);
  RESOLVE(getspecific);
  RESOLVE(setspecific);
  RESOLVE(once);
  RESOLVE(atfork);
  RESOLVE(mutexattr_init);
  RESOLVE(mutexattr_destroy);
  RESOLVE(mutexattr_settype);
  RESOLVE(mutexattr_setprotocol);
  RESOLVE(mutex_init);
  RESOLVE(mutex_lock);
  RESOLVE(mutex_trylock);
  RESOLVE(mutex_unlock);
  RESOLVE(mutex_destroy);
  RESOLVE(cond_wait);
  RESOLVE(condattr_destroy);
  RESOLVE(condattr_init);
  RESOLVE(condattr_setclock);
  RESOLVE(cond_init);
  RESOLVE(cond_timedwait);
  RESOLVE(cond_signal);
  RESOLVE(cond_broadcast);
  RESOLVE(cond_destroy);
  RESOLVE(rwlock_rdlock);
  RESOLVE(rwlock_wrlock);
  RESOLVE(rwlock_unlock);
  RESOLVE(rwlock_init);
  RESOLVE(rwlock_destroy);
#undef RESOLVE
  return nullptr;
}

extern "C" int darwin_art_bionic_pthread_provider_reset(void) {
  ProviderState& state = State();
  std::scoped_lock lock(state.keys_mutex,
                        state.mutexes_mutex,
                        state.once_mutex,
                        state.rwlocks_mutex,
                        state.threads_mutex);
  ThreadTlsState* current = tls_destructor_state;
  if (current == nullptr) {
    pthread_once(&HostThreadTlsKeyOnce(), &InitializeHostThreadTlsKey);
    current = static_cast<ThreadTlsState*>(
        pthread_getspecific(HostThreadTlsKey()));
  }
  std::shared_ptr<ThreadEntry> current_foreign_entry;
  if (!state.threads.empty()) {
    // A host/ART-created current thread has a non-joinable provider entry so
    // its pthread_self token can be used by other threads.  It is safe to
    // retire only that one entry at reset; all other owned or foreign threads
    // remain a real busy state.
    if (current == nullptr || current->foreign_thread_token == 0 ||
        state.threads.size() != 1) {
      return kAndroidEbusy;
    }
    const auto found =
        state.threads.find(current->foreign_thread_token);
    if (found == state.threads.end()) return kAndroidEbusy;
    current_foreign_entry = found->second;
    {
      std::lock_guard<std::mutex> entry_lock(current_foreign_entry->mutex);
      if (current_foreign_entry->provider_owned ||
          !current_foreign_entry->published ||
          current_foreign_entry->host_exited ||
          !pthread_equal(current_foreign_entry->host, pthread_self())) {
        return kAndroidEbusy;
      }
    }
  }
  for (const KeySlot& slot : state.key_slots) {
    if (slot.active != nullptr) return kAndroidEbusy;
  }
  for (const auto& [address, entry] : state.mutexes) {
    (void)address;
    std::lock_guard<std::mutex> lifecycle_lock(entry->lifecycle_mutex);
    if (entry->lifecycle != MutexEntry::Lifecycle::kDestroyed) {
      return kAndroidEbusy;
    }
  }
  for (ConditionShard& shard : state.conditions) {
    std::lock_guard<std::mutex> shard_lock(shard.mutex);
    for (const auto& [address, entry] : shard.entries) {
      (void)address;
      std::lock_guard<std::mutex> lifecycle_lock(entry->lifecycle_mutex);
      if (entry->lifecycle != CondEntry::Lifecycle::kDestroyed) {
        return kAndroidEbusy;
      }
    }
  }
  for (const auto& [address, entry] : state.rwlocks) {
    (void)address;
    std::lock_guard<std::mutex> lifecycle_lock(entry->mutex);
    if (entry->writer_owner != 0 || entry->active_readers != 0 ||
        entry->pending_readers != 0 || entry->pending_writers != 0) {
      return kAndroidEbusy;
    }
  }
  for (ThreadTlsState* thread_state : state.thread_tls_states) {
    if (thread_state != current) return kAndroidEbusy;
  }
  if (current != nullptr) {
    if (tls_destructor_state != nullptr) return kAndroidEbusy;
    if (pthread_setspecific(HostThreadTlsKey(), nullptr) != 0) {
      return kAndroidEbusy;
    }
    if (current_foreign_entry != nullptr) {
      std::lock_guard<std::mutex> entry_lock(current_foreign_entry->mutex);
      current_foreign_entry->host_exited = true;
      current_foreign_entry->published = false;
      state.threads.erase(current_foreign_entry->token);
      current->foreign_thread_token = 0;
    }
    state.thread_tls_states.erase(current);
    delete current;
    current_thread_token = 0;
  }
  state.mutexes.clear();
  for (ConditionShard& shard : state.conditions) shard.entries.clear();
  state.rwlocks.clear();
  state.once_controls.clear();
  return 0;
}

extern "C" size_t darwin_art_bionic_pthread_provider_retired_cell_count(void) {
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.keys_mutex);
  size_t count = 0;
  for (const ThreadTlsState* thread_state : state.thread_tls_states) {
    for (const ThreadTlsValue& value : thread_state->values) {
      if (value.value != nullptr) ++count;
    }
  }
  return count;
}

extern "C" int darwin_art_bionic_pthread_capability(const char* capability) {
  if (capability == nullptr) return 0;
  const std::string_view name(capability);
  return name == "thread-identity-token" || name == "tls-key-destructor" ||
         name == "once-private" || name == "mutex-normal-private" ||
         name == "mutex-recursive-private" ||
         name == "mutex-errorcheck-private" ||
         name == "cond-private" || name == "cond-monotonic-clock" ||
         name == "rwlock-private-reader-preferred" ||
         name == "thread-create-join-detach-owner";
}
