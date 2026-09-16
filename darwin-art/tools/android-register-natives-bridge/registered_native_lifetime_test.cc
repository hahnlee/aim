#include "darwin_art_registered_native_bridge.h"

#include <cassert>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <mutex>
#include <thread>

namespace {

constexpr uintptr_t kFunction = 0x10001000;
constexpr uintptr_t kExecutableBegin = 0x10000000;
constexpr uintptr_t kExecutableEnd = 0x10010000;
constexpr uint64_t kImageId = 0x415254454c46ull;

struct TestContext {
  std::mutex mutex;
  std::condition_variable condition;
  DarwinArtAndroidFunctionOwnerV1 owner{kImageId, 1, kExecutableBegin,
                                        kExecutableEnd};
  DarwinArtRegisteredNativeCache *cache = nullptr;
  int lookup_status = 1;
  bool malformed_owner = false;
  bool active = true;
  bool block_build = false;
  bool build_entered = false;
  bool unblock_build = false;
  size_t owner_leases = 0;
  size_t owner_releases = 0;
  size_t lookup_calls = 0;
  size_t builds = 0;
  size_t destroys = 0;
  size_t destroys_with_lease = 0;
  size_t destroys_without_lease = 0;
  size_t reentrant_cache_size_calls = 0;
  size_t reentrant_cache_size_observed = 0;
};

int LookupOwner(void *opaque, const void *function,
                DarwinArtAndroidFunctionOwnerV1 *owner_out) {
  auto *context = static_cast<TestContext *>(opaque);
  const uintptr_t address = reinterpret_cast<uintptr_t>(function);
  int status = 0;
  bool malformed = false;
  DarwinArtAndroidFunctionOwnerV1 owner{};
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    ++context->lookup_calls;
    if (!context->active || address != kFunction)
      return 0;
    status = context->lookup_status;
    malformed = context->malformed_owner;
    owner = context->owner;
    if (status > 0) {
      ++context->owner_leases;
    }
  }
  if (status <= 0) {
    if (owner_out != nullptr)
      *owner_out = {};
    return status;
  }
  *owner_out = malformed ? DarwinArtAndroidFunctionOwnerV1{0, owner.generation,
                                                           kExecutableBegin,
                                                           kExecutableEnd}
                         : owner;
  // A positive result is an acquired temporary image lease, even when the
  // returned record is malformed and the cache must reject it.
  return status;
}

void ReleaseOwner(void *opaque, const DarwinArtAndroidFunctionOwnerV1 *) {
  auto *context = static_cast<TestContext *>(opaque);
  std::lock_guard<std::mutex> lock(context->mutex);
  assert(context->owner_leases != 0);
  --context->owner_leases;
  ++context->owner_releases;
  context->condition.notify_all();
}

void *BuildThunk(void *opaque, const void *,
                 const DarwinArtAndroidFunctionOwnerV1 *owner, const char *,
                 uint32_t, DarwinArtJniCallType) {
  auto *context = static_cast<TestContext *>(opaque);
  {
    std::unique_lock<std::mutex> lock(context->mutex);
    ++context->builds;
    const uintptr_t value = 0x20000000u + context->builds * 0x100u;
    if (context->block_build) {
      context->build_entered = true;
      context->condition.notify_all();
      context->condition.wait(lock,
                              [context] { return context->unblock_build; });
    }
    lock.unlock();
    // The cache mutex must not be held while invoking this callback. A
    // reentrant cache-size query would deadlock if the build ran under it.
    const size_t observed =
        darwin_art_registered_native_cache_size(context->cache);
    lock.lock();
    ++context->reentrant_cache_size_calls;
    context->reentrant_cache_size_observed = observed;
    return reinterpret_cast<void *>(value + owner->generation * 0x10u);
  }
}

void DestroyThunk(void *opaque, void *) {
  auto *context = static_cast<TestContext *>(opaque);
  std::lock_guard<std::mutex> lock(context->mutex);
  ++context->destroys;
  if (context->owner_leases == 0)
    ++context->destroys_without_lease;
  else
    ++context->destroys_with_lease;
  context->condition.notify_all();
}

DarwinArtRegisteredNativeCache *MakeCache(TestContext *context) {
  const DarwinArtRegisteredNativeThunkFactoryV1 factory{
      DARWIN_ART_REGISTERED_NATIVE_BRIDGE_ABI_VERSION,
      sizeof(DarwinArtRegisteredNativeThunkFactoryV1),
      context,
      &LookupOwner,
      &ReleaseOwner,
      &BuildThunk,
      &DestroyThunk};
  auto *cache = darwin_art_registered_native_cache_create(&factory);
  assert(cache != nullptr);
  context->cache = cache;
  return cache;
}

void TestOwnerStatusesAndMalformedLease(DarwinArtRegisteredNativeCache *cache,
                                        TestContext *context) {
  const void *callable = reinterpret_cast<const void *>(uintptr_t{1});
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->lookup_status = 0;
    context->malformed_owner = false;
  }
  assert(!darwin_art_is_android_function_pointer(
      cache, reinterpret_cast<void *>(kFunction)));
  assert(darwin_art_resolve_registered_native(
             cache, reinterpret_cast<void *>(kFunction), false, "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR,
             &callable) == DARWIN_ART_REGISTERED_NATIVE_DIRECT);
  assert(callable == reinterpret_cast<const void *>(kFunction));

  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->lookup_status = -1;
  }
  callable = reinterpret_cast<const void *>(uintptr_t{1});
  assert(!darwin_art_is_android_function_pointer(
      cache, reinterpret_cast<void *>(kFunction)));
  assert(darwin_art_resolve_registered_native(
             cache, reinterpret_cast<void *>(kFunction), false, "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR,
             &callable) == DARWIN_ART_REGISTERED_NATIVE_ERROR);
  assert(callable == nullptr);

  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->lookup_status = 1;
    context->malformed_owner = true;
  }
  assert(darwin_art_get_registered_native_trampoline(
             cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR) == nullptr);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->owner_leases == 0);
    assert(context->owner_releases == 1);
  }
  callable = reinterpret_cast<const void *>(uintptr_t{1});
  assert(darwin_art_resolve_registered_native(
             cache, reinterpret_cast<void *>(kFunction), false, "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR,
             &callable) == DARWIN_ART_REGISTERED_NATIVE_ERROR);
  assert(callable == nullptr);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->owner_leases == 0);
    assert(context->owner_releases == 2);
  }
}

void TestRetirementRaceAndGenerationReuse(DarwinArtRegisteredNativeCache *cache,
                                          TestContext *context) {
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->lookup_status = 1;
    context->active = true;
    context->malformed_owner = false;
    context->block_build = true;
    context->build_entered = false;
    context->unblock_build = false;
  }

  void *produced = reinterpret_cast<void *>(uintptr_t{1});
  std::thread builder([&] {
    produced = darwin_art_get_registered_native_trampoline(
        cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
        DARWIN_ART_JNI_CALL_REGULAR);
  });
  {
    std::unique_lock<std::mutex> lock(context->mutex);
    context->condition.wait(lock, [&] { return context->build_entered; });
    assert(context->reentrant_cache_size_calls == 0);
  }

  // Retirement races an in-flight build. The synthetic thunk returned by the
  // callback must be destroyed rather than published under a retired key.
  assert(darwin_art_registered_native_cache_retire_image(cache, kImageId, 1) ==
         0);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->active = false;
    context->unblock_build = true;
  }
  context->condition.notify_all();
  builder.join();
  assert(produced == nullptr);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->builds == 1);
    assert(context->destroys == 1);
    assert(context->owner_leases == 0);
    assert(context->reentrant_cache_size_calls == 1);
    assert(context->reentrant_cache_size_observed == 0);
  }
  assert(darwin_art_registered_native_cache_size(cache) == 0);

  // A retired generation remains rejected even if the loader reports the
  // address as live again; no stale generation may be rebuilt.
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->active = true;
  }
  assert(darwin_art_get_registered_native_trampoline(
             cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR) == nullptr);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->builds == 1);
    assert(context->destroys == 1);
    assert(context->owner_leases == 0);
  }

  // A new image generation gets a fresh cache key and a fresh synthetic thunk.
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->owner.generation = 2;
    context->block_build = false;
  }
  void *fresh = darwin_art_get_registered_native_trampoline(
      cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
      DARWIN_ART_JNI_CALL_REGULAR);
  assert(fresh != nullptr);
  assert(darwin_art_get_registered_native_trampoline(
             cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR) == fresh);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->builds == 2);
    assert(context->destroys == 1);
    assert(context->owner_leases == 1);
    assert(context->reentrant_cache_size_calls == 2);
  }
  assert(darwin_art_registered_native_cache_size(cache) == 1);
  assert(darwin_art_registered_native_cache_retire_image(cache, kImageId, 2) ==
         1);
  assert(darwin_art_registered_native_cache_size(cache) == 0);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->destroys == 2);
  }
}

void TestCachedLeaseAndResolve(DarwinArtRegisteredNativeCache *cache,
                               TestContext *context) {
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    context->owner.generation = 3;
    context->active = true;
    context->block_build = false;
    context->lookup_calls = 0;
  }
  void *fresh = darwin_art_get_registered_native_trampoline(
      cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
      DARWIN_ART_JNI_CALL_REGULAR);
  assert(fresh != nullptr);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->lookup_calls == 1);
    // A cached entry transfers the target-image lease acquired by its initial
    // lookup; cache hits use separate temporary lookup leases.
    assert(context->owner_leases == 1);
  }

  size_t releases_before = 0;
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    releases_before = context->owner_releases;
    context->lookup_calls = 0;
  }
  assert(darwin_art_get_registered_native_trampoline(
             cache, reinterpret_cast<void *>(kFunction), "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR) == fresh);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    // Cache hits acquire/release only their extra lookup lease; the cached
    // entry's lease remains held until retirement or cache destruction.
    assert(context->lookup_calls == 1);
    assert(context->owner_releases == releases_before + 1);
    assert(context->owner_leases == 1);
  }

  // Resolve must retain one owner lease through its single lookup and avoid a
  // lookup/release gap before returning the already-cached callable.
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    releases_before = context->owner_releases;
    context->lookup_calls = 0;
  }
  const void *callable = nullptr;
  assert(darwin_art_resolve_registered_native(
             cache, reinterpret_cast<void *>(kFunction), true, "JIJI", 4,
             DARWIN_ART_JNI_CALL_REGULAR,
             &callable) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE);
  assert(callable == fresh);
  {
    std::lock_guard<std::mutex> lock(context->mutex);
    assert(context->lookup_calls == 1);
    assert(context->owner_releases == releases_before + 1);
    assert(context->owner_leases == 1);
  }
}

} // namespace

int main() {
  TestContext context;
  DarwinArtRegisteredNativeCache *cache = MakeCache(&context);
  TestOwnerStatusesAndMalformedLease(cache, &context);
  TestRetirementRaceAndGenerationReuse(cache, &context);
  TestCachedLeaseAndResolve(cache, &context);
  {
    std::lock_guard<std::mutex> lock(context.mutex);
    assert(context.owner_leases == 1);
    assert(context.destroys_without_lease == 0);
    assert(context.destroys_with_lease == 2);
  }
  darwin_art_registered_native_cache_destroy(cache);
  {
    std::lock_guard<std::mutex> lock(context.mutex);
    assert(context.owner_leases == 0);
    assert(context.destroys == 3);
    assert(context.destroys_with_lease == 3);
    assert(context.destroys_without_lease == 0);
  }
  std::puts("registered-native-lifetime: PASS owner-status lease-cleanup "
            "reentrant-build retirement-race generation-reuse");
  return 0;
}
