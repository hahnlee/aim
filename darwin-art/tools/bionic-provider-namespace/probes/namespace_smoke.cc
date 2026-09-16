#include "darwin_art_bionic_provider_namespace.h"

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
  DarwinArtBionicProviderId owner;
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
  std::vector<DarwinArtBionicProviderId> releases;
  bool block = false;
  bool entered = false;
};

struct Context {
  DarwinArtBionicProviderId owner;
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

DarwinArtBionicNamespace *Build(Context *contexts, Shared *shared) {
  DarwinArtBionicNamespace *instance = darwin_art_bionic_namespace_create();
  Check(instance != nullptr, "create");
  for (int index = 0; index < DARWIN_ART_BIONIC_PROVIDER_COUNT; ++index) {
    contexts[index].owner = static_cast<DarwinArtBionicProviderId>(index);
    contexts[index].shared = shared;
    DarwinArtBionicProviderBinding binding{
        static_cast<DarwinArtBionicProviderId>(index), &contexts[index],
        Resolve, Release};
    Check(darwin_art_bionic_namespace_bind(instance, &binding) ==
              DARWIN_ART_BIONIC_NAMESPACE_OK,
          "bind");
    Check(darwin_art_bionic_namespace_bind(instance, &binding) ==
              DARWIN_ART_BIONIC_NAMESPACE_DUPLICATE_BINDING,
          "duplicate binding");
  }
  Check(darwin_art_bionic_namespace_seal(instance) ==
            DARWIN_ART_BIONIC_NAMESPACE_OK,
        "seal");
  return instance;
}

} // namespace

int main() {
  Check(darwin_art_bionic_namespace_owned_count() == 800, "owned count");
  Check(darwin_art_bionic_namespace_unsupported_libc_count() == 0,
        "unsupported count");
  Check(sizeof(kExpected) / sizeof(kExpected[0]) == 800, "fixture count");
  Check(kUnsupported.empty(), "unsupported fixture count");

  {
    Shared partial_shared;
    Context partial_context{DARWIN_ART_BIONIC_PROVIDER_LEAF, &partial_shared};
    DarwinArtBionicNamespace *partial = darwin_art_bionic_namespace_create();
    DarwinArtBionicProviderBinding binding{DARWIN_ART_BIONIC_PROVIDER_LEAF,
                                           &partial_context, Resolve, Release};
    Check(darwin_art_bionic_namespace_bind(partial, &binding) ==
                  DARWIN_ART_BIONIC_NAMESPACE_OK &&
              darwin_art_bionic_namespace_seal(partial) ==
                  DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_UNBOUND,
          "partial namespace cannot seal");
    darwin_art_bionic_namespace_destroy(partial);
    Check(partial_shared.releases.size() == 1 &&
              partial_shared.releases[0] == DARWIN_ART_BIONIC_PROVIDER_LEAF,
          "partial namespace releases bound provider");
  }

  {
    Shared rejecting_shared;
    Context rejecting_contexts[DARWIN_ART_BIONIC_PROVIDER_COUNT];
    DarwinArtBionicNamespace *rejecting =
        Build(rejecting_contexts, &rejecting_shared);
    rejecting_contexts[DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR].reject_malloc =
        true;
    auto result = darwin_art_bionic_namespace_resolve(rejecting, "libc.so",
                                                      "malloc", "LIBC");
    Check(result.status == DARWIN_ART_BIONIC_NAMESPACE_PROVIDER_REJECTED &&
              result.owner == DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR &&
              result.address == 0,
          "provider drift rejects lookup");
    darwin_art_bionic_namespace_destroy(rejecting);
  }

  Shared shared;
  Context contexts[DARWIN_ART_BIONIC_PROVIDER_COUNT];
  DarwinArtBionicNamespace *instance = Build(contexts, &shared);

  for (const Expected &expected : kExpected) {
    DarwinArtBionicNamespaceResult result = darwin_art_bionic_namespace_resolve(
        instance, expected.soname, expected.symbol,
        expected.version[0] == '\0' ? nullptr : expected.version);
    Check(result.status == DARWIN_ART_BIONIC_NAMESPACE_OK, "owned resolution");
    Check(result.owner == expected.owner && result.address != 0,
          "owner/address");
  }
  for (const char *symbol : kUnsupported) {
    DarwinArtBionicNamespaceResult result = darwin_art_bionic_namespace_resolve(
        instance, "libc.so", symbol, "LIBC");
    Check(result.status == DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
          "unsupported manifest");
    char capability_class = 0;
    const char *reason = nullptr;
    Check(darwin_art_bionic_namespace_unsupported_libc(
              symbol, &capability_class, &reason) == 1 &&
              capability_class >= 'B' && capability_class <= 'D' &&
              reason != nullptr && reason[0] != '\0',
          "unsupported capability detail");
  }
  char capability_class = '!';
  const char *reason = reinterpret_cast<const char *>(1);
  Check(darwin_art_bionic_namespace_unsupported_libc(
            "strlen", &capability_class, &reason) == 0 &&
            capability_class == '!' &&
            reason == reinterpret_cast<const char *>(1),
        "owned symbol has no unsupported detail");
  Check(darwin_art_bionic_namespace_resolve(instance, "libSystem.B.dylib",
                                            "strlen", "LIBC")
                .status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_SONAME,
        "host soname rejection");
  Check(darwin_art_bionic_namespace_resolve(instance, "libc.so", "strlen",
                                            nullptr)
                .status == DARWIN_ART_BIONIC_NAMESPACE_OK,
        "original libc default version");
  Check(darwin_art_bionic_namespace_resolve(instance, "liblog.so",
                                            "__android_log_write", "LIBC")
                .status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION,
        "versioned liblog rejection");
  Check(darwin_art_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_assert", "LIBLOG").status == DARWIN_ART_BIONIC_NAMESPACE_OK,
        "original assert version admitted");
  Check(darwin_art_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_set_logger", "LIBLOG_R").status == DARWIN_ART_BIONIC_NAMESPACE_OK,
        "original logger version admitted");
  Check(darwin_art_bionic_namespace_resolve(instance, "liblog.so",
            "__android_log_set_logger", "LIBLOG").status == DARWIN_ART_BIONIC_NAMESPACE_UNKNOWN_VERSION,
        "another symbol's version denied");
  Check(darwin_art_bionic_namespace_resolve(instance, "libc.so",
                                            "dl_iterate_phdr", "LIBC")
                .status == DARWIN_ART_BIONIC_NAMESPACE_UNSUPPORTED_SYMBOL,
        "libdl symbol is not aliased into libc");

  constexpr int kThreads = 12;
  constexpr int kRounds = 200;
  std::vector<std::thread> workers;
  std::atomic<bool> failed{false};
  for (int thread = 0; thread < kThreads; ++thread) {
    workers.emplace_back([instance, &failed] {
      for (int round = 0; round < kRounds; ++round) {
        for (const Expected &expected : kExpected) {
          const auto result = darwin_art_bionic_namespace_resolve(
              instance, expected.soname, expected.symbol,
              expected.version[0] == '\0' ? nullptr : expected.version);
          if (result.status != DARWIN_ART_BIONIC_NAMESPACE_OK ||
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
    Check(darwin_art_bionic_namespace_resolve(instance, "libc.so", "strlen",
                                              "LIBC")
                  .status == DARWIN_ART_BIONIC_NAMESPACE_OK,
          "in-flight resolution");
  });
  {
    std::unique_lock<std::mutex> lock(shared.mutex);
    shared.condition.wait(lock, [&shared] { return shared.entered; });
  }
  std::thread teardown([instance] {
    Check(darwin_art_bionic_namespace_teardown(instance) ==
              DARWIN_ART_BIONIC_NAMESPACE_OK,
          "teardown");
  });
  for (;;) {
    auto result = darwin_art_bionic_namespace_resolve(instance, "libc.so",
                                                      "memcmp", "LIBC");
    if (result.status == DARWIN_ART_BIONIC_NAMESPACE_SHUTTING_DOWN)
      break;
    Check(result.status == DARWIN_ART_BIONIC_NAMESPACE_OK,
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

  constexpr DarwinArtBionicProviderId kExpectedRelease[] = {
      DARWIN_ART_BIONIC_PROVIDER_DSO_LIFECYCLE,
      DARWIN_ART_BIONIC_PROVIDER_VM,
      DARWIN_ART_BIONIC_PROVIDER_MATH,
      DARWIN_ART_BIONIC_PROVIDER_DNS,
      DARWIN_ART_BIONIC_PROVIDER_SOCKET,
      DARWIN_ART_BIONIC_PROVIDER_AAUDIO,
      DARWIN_ART_BIONIC_PROVIDER_BINDER_NDK,
      DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER,
      DARWIN_ART_BIONIC_PROVIDER_ABORT,
      DARWIN_ART_BIONIC_PROVIDER_SYSLOG,
      DARWIN_ART_BIONIC_PROVIDER_SYSCALL,
      DARWIN_ART_BIONIC_PROVIDER_SWPRINTF,
      DARWIN_ART_BIONIC_PROVIDER_IOCTL,
      DARWIN_ART_BIONIC_PROVIDER_STRFTIME,
      DARWIN_ART_BIONIC_PROVIDER_SENDFILE,
      DARWIN_ART_BIONIC_PROVIDER_LIBLOG,
      DARWIN_ART_BIONIC_PROVIDER_NUMERIC,
      DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION,
      DARWIN_ART_BIONIC_PROVIDER_WIDE_FLOAT,
      DARWIN_ART_BIONIC_PROVIDER_FLOAT_CONVERSION,
      DARWIN_ART_BIONIC_PROVIDER_WIDE_INTEGER,
      DARWIN_ART_BIONIC_PROVIDER_STRERROR,
      DARWIN_ART_BIONIC_PROVIDER_WIDE_STDIO,
      DARWIN_ART_BIONIC_PROVIDER_SCANF,
      DARWIN_ART_BIONIC_PROVIDER_FORMATTED_STDIO,
      DARWIN_ART_BIONIC_PROVIDER_FORMAT,
      DARWIN_ART_BIONIC_PROVIDER_LOCALE,
      DARWIN_ART_BIONIC_PROVIDER_STDIO,
      DARWIN_ART_BIONIC_PROVIDER_PHDR,
      DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE,
      DARWIN_ART_BIONIC_PROVIDER_PTHREAD,
      DARWIN_ART_BIONIC_PROVIDER_TIME,
      DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM,
      DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR,
      DARWIN_ART_BIONIC_PROVIDER_LEAF,
      DARWIN_ART_BIONIC_PROVIDER_ERRNO,
  };
  Check(shared.releases.size() == DARWIN_ART_BIONIC_PROVIDER_COUNT,
        "release count");
  for (size_t index = 0; index < shared.releases.size(); ++index) {
    Check(shared.releases[index] == kExpectedRelease[index], "release order");
  }
  Check(darwin_art_bionic_namespace_teardown(instance) ==
            DARWIN_ART_BIONIC_NAMESPACE_OK,
        "idempotent teardown");
  darwin_art_bionic_namespace_destroy(instance);

  std::fprintf(
      stderr,
      "bionic-provider-namespace: PASS libcxx=160/160 libc-rows=620 "
      "liblog-rows=19 binder-ndk=39 aaudio=30 owned=800 duplicate-triple=0 threads=12 "
      "teardown=ordered+quiescent host-fallback=denied\n");
  return 0;
}
