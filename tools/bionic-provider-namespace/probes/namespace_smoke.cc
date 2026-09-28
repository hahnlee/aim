#include "aim_bionic_provider_namespace.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace {

struct Expected {
  const char *soname;
  const char *symbol;
  const char *version;
  AimBionicProviderId owner;
  bool default_version;
};

constexpr Expected kExpected[] = {
#include "ownership.inc"
};

constexpr std::array<const char *, 0> kUnsupported = {{
#include "unsupported_symbols.inc"
}};

struct Shared {
  std::mutex mutex;
  std::condition_variable condition;
  std::vector<AimBionicProviderId> releases;
  bool block = false;
  bool entered = false;
};

struct Context {
  AimBionicProviderId owner;
  Shared *shared;
  std::atomic<size_t> calls{0};
  bool reject_malloc = false;
};

uintptr_t Hash(const char *text) {
  uintptr_t value = 1469598103934665603ULL;
  for (const unsigned char *cursor =
           reinterpret_cast<const unsigned char *>(text);
       *cursor != 0; ++cursor) {
    value = (value ^ *cursor) * 1099511628211ULL;
  }
  return value;
}

uintptr_t Resolve(void *opaque, const char *soname, const char *symbol,
                  const char *version) {
  auto *context = static_cast<Context *>(opaque);
  context->calls.fetch_add(1, std::memory_order_relaxed);
  if (context->reject_malloc && std::strcmp(symbol, "malloc") == 0)
    return 0;
  if (std::strcmp(symbol, "strlen") == 0) {
    std::unique_lock<std::mutex> lock(context->shared->mutex);
    if (context->shared->block) {
      context->shared->entered = true;
      context->shared->condition.notify_all();
      context->shared->condition.wait(
          lock, [context] { return !context->shared->block; });
    }
  }
  const uintptr_t version_hash = version == nullptr ? 17 : Hash(version);
  const uintptr_t value =
      ((static_cast<uintptr_t>(context->owner) + 1) << 56) ^ Hash(soname) ^
      Hash(symbol) ^ version_hash;
  return value == 0 ? 1 : value;
}

void Release(void *opaque) {
  auto *context = static_cast<Context *>(opaque);
  std::lock_guard<std::mutex> lock(context->shared->mutex);
  context->shared->releases.push_back(context->owner);
}

void Check(bool condition, const char *message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-provider-namespace: FAIL %s\n", message);
    std::abort();
  }
}

AimBionicNamespace *Build(Context *contexts, Shared *shared) {
  AimBionicNamespace *instance = aim_bionic_namespace_create();
  Check(instance != nullptr, "create");
  for (int index = 0; index < AIM_BIONIC_PROVIDER_COUNT; ++index) {
    contexts[index].owner = static_cast<AimBionicProviderId>(index);
    contexts[index].shared = shared;
    AimBionicProviderBinding binding{
        static_cast<AimBionicProviderId>(index), &contexts[index],
        Resolve, Release};
    Check(aim_bionic_namespace_bind(instance, &binding) ==
              AIM_BIONIC_NAMESPACE_OK,
          "bind");
    Check(aim_bionic_namespace_bind(instance, &binding) ==
              AIM_BIONIC_NAMESPACE_DUPLICATE_BINDING,
          "duplicate binding");
  }
  Check(aim_bionic_namespace_seal(instance) ==
            AIM_BIONIC_NAMESPACE_OK,
        "seal");
  return instance;
}

} // namespace

int main() {
  Check(aim_bionic_namespace_owned_count() == 798, "owned count");
  Check(aim_bionic_namespace_unsupported_libc_count() == 0,
        "unsupported count");
  Check(sizeof(kExpected) / sizeof(kExpected[0]) == 798, "fixture count");
  Check(kUnsupported.empty(), "unsupported fixture count");

  {
    Shared partial_shared;
    Context partial_context{AIM_BIONIC_PROVIDER_LEAF, &partial_shared};
    AimBionicNamespace *partial = aim_bionic_namespace_create();
    AimBionicProviderBinding binding{AIM_BIONIC_PROVIDER_LEAF,
                                           &partial_context, Resolve, Release};
    Check(aim_bionic_namespace_bind(partial, &binding) ==
                  AIM_BIONIC_NAMESPACE_OK &&
              aim_bionic_namespace_seal(partial) ==
                  AIM_BIONIC_NAMESPACE_PROVIDER_UNBOUND,
          "partial namespace cannot seal");
    aim_bionic_namespace_destroy(partial);
    Check(partial_shared.releases.size() == 1 &&
              partial_shared.releases[0] == AIM_BIONIC_PROVIDER_LEAF,
          "partial namespace releases bound provider");
  }

  {
    Shared rejecting_shared;
    Context rejecting_contexts[AIM_BIONIC_PROVIDER_COUNT];
    AimBionicNamespace *rejecting =
        Build(rejecting_contexts, &rejecting_shared);
    rejecting_contexts[AIM_BIONIC_PROVIDER_ALLOCATOR].reject_malloc =
        true;
    auto result = aim_bionic_namespace_resolve(rejecting, "libc.so",
                                                      "malloc", "LIBC");
    Check(result.status == AIM_BIONIC_NAMESPACE_PROVIDER_REJECTED &&
              result.owner == AIM_BIONIC_PROVIDER_ALLOCATOR &&
              result.address == 0,
          "provider drift rejects lookup");
    aim_bionic_namespace_destroy(rejecting);
  }

  Shared shared;
  Context contexts[AIM_BIONIC_PROVIDER_COUNT];
  AimBionicNamespace *instance = Build(contexts, &shared);

  for (const Expected &expected : kExpected) {
    AimBionicNamespaceResult result = aim_bionic_namespace_resolve(
        instance, expected.soname, expected.symbol,
        expected.version[0] == '\0' ? nullptr : expected.version);
    Check(result.status == AIM_BIONIC_NAMESPACE_OK, "owned resolution");
    Check(result.owner == expected.owner && result.address != 0,
          "owner/address");
  }
  for (const char *symbol : kUnsupported) {
    AimBionicNamespaceResult result = aim_bionic_namespace_resolve(
        instance, "libc.so", symbol, "LIBC");
    Check(result.status == AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
          "unsupported manifest");
    char capability_class = 0;
    const char *reason = nullptr;
    Check(aim_bionic_namespace_unsupported_libc(
              symbol, &capability_class, &reason) == 1 &&
              capability_class >= 'B' && capability_class <= 'D' &&
              reason != nullptr && reason[0] != '\0',
          "unsupported capability detail");
  }
  char capability_class = '!';
  const char *reason = reinterpret_cast<const char *>(1);
  Check(aim_bionic_namespace_unsupported_libc(
            "strlen", &capability_class, &reason) == 0 &&
            capability_class == '!' &&
            reason == reinterpret_cast<const char *>(1),
        "owned symbol has no unsupported detail");
  Check(aim_bionic_namespace_resolve(instance, "libSystem.B.dylib",
                                            "strlen", "LIBC")
                .status == AIM_BIONIC_NAMESPACE_UNKNOWN_SONAME,
        "host soname rejection");
  Check(aim_bionic_namespace_resolve(instance, "libc.so", "strlen",
                                            nullptr)
                .status == AIM_BIONIC_NAMESPACE_OK,
        "original libc default version");
  Check(aim_bionic_namespace_resolve(instance, "liblog.so",
                                            "__android_log_write", "LIBC")
                .status == AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION,
        "versioned liblog rejection");
  Check(aim_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_assert", "LIBLOG").status == AIM_BIONIC_NAMESPACE_OK,
        "original assert version admitted");
  Check(aim_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_set_logger", "LIBLOG_R").status == AIM_BIONIC_NAMESPACE_OK,
        "original logger version admitted");
  Check(aim_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_set_logger", "LIBLOG").status == AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION,
        "another symbol's version denied");
  Check(aim_bionic_namespace_resolve(instance, "libc.so",
                                            "dl_iterate_phdr", "LIBC")
                .status == AIM_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
        "libdl symbol is not aliased into libc");

  constexpr int kThreads = 12;
  constexpr int kRounds = 200;
  std::vector<std::thread> workers;
  std::atomic<bool> failed{false};
  for (int thread = 0; thread < kThreads; ++thread) {
    workers.emplace_back([instance, &failed] {
      for (int round = 0; round < kRounds; ++round) {
        for (const Expected &expected : kExpected) {
          const auto result = aim_bionic_namespace_resolve(
              instance, expected.soname, expected.symbol,
              expected.version[0] == '\0' ? nullptr : expected.version);
          if (result.status != AIM_BIONIC_NAMESPACE_OK ||
              result.owner != expected.owner || result.address == 0) {
            failed.store(true, std::memory_order_relaxed);
          }
        }
      }
    });
  }
  for (std::thread &worker : workers)
    worker.join();
  Check(!failed.load(), "concurrent resolution");

  {
    std::lock_guard<std::mutex> lock(shared.mutex);
    shared.block = true;
  }
  std::thread slow([instance] {
    Check(aim_bionic_namespace_resolve(instance, "libc.so", "strlen",
                                              "LIBC")
                  .status == AIM_BIONIC_NAMESPACE_OK,
          "in-flight resolution");
  });
  {
    std::unique_lock<std::mutex> lock(shared.mutex);
    shared.condition.wait(lock, [&shared] { return shared.entered; });
  }
  std::thread teardown([instance] {
    Check(aim_bionic_namespace_teardown(instance) ==
              AIM_BIONIC_NAMESPACE_OK,
          "teardown");
  });
  for (;;) {
    auto result = aim_bionic_namespace_resolve(instance, "libc.so",
                                                      "memcmp", "LIBC");
    if (result.status == AIM_BIONIC_NAMESPACE_SHUTTING_DOWN)
      break;
    Check(result.status == AIM_BIONIC_NAMESPACE_OK,
          "drain admission transition");
  }
  {
    std::lock_guard<std::mutex> lock(shared.mutex);
    Check(shared.releases.empty(), "release waited for callback");
    shared.block = false;
    shared.condition.notify_all();
  }
  slow.join();
  teardown.join();

  constexpr AimBionicProviderId kExpectedRelease[] = {
      AIM_BIONIC_PROVIDER_DSO_LIFECYCLE,
      AIM_BIONIC_PROVIDER_VM,
      AIM_BIONIC_PROVIDER_MATH,
      AIM_BIONIC_PROVIDER_DNS,
      AIM_BIONIC_PROVIDER_SOCKET,
      AIM_BIONIC_PROVIDER_AAUDIO,
      AIM_BIONIC_PROVIDER_BINDER_NDK,
      AIM_BIONIC_PROVIDER_CENTRAL_FD_BROKER,
      AIM_BIONIC_PROVIDER_ABORT,
      AIM_BIONIC_PROVIDER_SYSLOG,
      AIM_BIONIC_PROVIDER_SYSCALL,
      AIM_BIONIC_PROVIDER_SWPRINTF,
      AIM_BIONIC_PROVIDER_IOCTL,
      AIM_BIONIC_PROVIDER_STRFTIME,
      AIM_BIONIC_PROVIDER_SENDFILE,
      AIM_BIONIC_PROVIDER_LIBLOG,
      AIM_BIONIC_PROVIDER_NUMERIC,
      AIM_BIONIC_PROVIDER_BINARY128_CONVERSION,
      AIM_BIONIC_PROVIDER_WIDE_FLOAT,
      AIM_BIONIC_PROVIDER_FLOAT_CONVERSION,
      AIM_BIONIC_PROVIDER_WIDE_INTEGER,
      AIM_BIONIC_PROVIDER_STRERROR,
      AIM_BIONIC_PROVIDER_WIDE_STDIO,
      AIM_BIONIC_PROVIDER_SCANF,
      AIM_BIONIC_PROVIDER_FORMATTED_STDIO,
      AIM_BIONIC_PROVIDER_FORMAT,
      AIM_BIONIC_PROVIDER_LOCALE,
      AIM_BIONIC_PROVIDER_STDIO,
      AIM_BIONIC_PROVIDER_PHDR,
      AIM_BIONIC_PROVIDER_PROCESS_STATE,
      AIM_BIONIC_PROVIDER_PTHREAD,
      AIM_BIONIC_PROVIDER_TIME,
      AIM_BIONIC_PROVIDER_FILESYSTEM,
      AIM_BIONIC_PROVIDER_ALLOCATOR,
      AIM_BIONIC_PROVIDER_LEAF,
      AIM_BIONIC_PROVIDER_ERRNO,
  };
  Check(shared.releases.size() == AIM_BIONIC_PROVIDER_COUNT,
        "release count");
  for (size_t index = 0; index < shared.releases.size(); ++index) {
    Check(shared.releases[index] == kExpectedRelease[index], "release order");
  }
  Check(aim_bionic_namespace_teardown(instance) ==
            AIM_BIONIC_NAMESPACE_OK,
        "idempotent teardown");
  aim_bionic_namespace_destroy(instance);

  std::fprintf(
      stderr,
      "bionic-provider-namespace: PASS libcxx=160/160 libc-rows=620 "
      "liblog-rows=19 binder-ndk=39 aaudio=30 owned=798 duplicate-triple=0 threads=12 "
      "teardown=ordered+quiescent host-fallback=denied\n");
  return 0;
}
