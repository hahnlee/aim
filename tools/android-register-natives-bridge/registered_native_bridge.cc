#include "aim_registered_native_bridge.h"

#include <cstring>
#include <exception>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace {

bool ValidOwner(const void* function,
                const AimAndroidFunctionOwnerV1& owner) {
  const uintptr_t address = reinterpret_cast<uintptr_t>(function);
  return owner.image_id != 0 && owner.generation != 0 &&
         owner.executable_begin < owner.executable_end &&
         address >= owner.executable_begin && address < owner.executable_end;
}

bool ValidShorty(const char* shorty,
                 uint32_t shorty_length,
                 AimJniCallType call_type) {
  if (shorty == nullptr || shorty_length == 0 ||
      std::strlen(shorty) != shorty_length ||
      (call_type != AIM_JNI_CALL_REGULAR &&
       call_type != AIM_JNI_CALL_CRITICAL_NATIVE)) {
    return false;
  }
  const auto valid_type = [](char type) {
    return std::strchr("VZBCSIJFDL", type) != nullptr;
  };
  if (!valid_type(shorty[0]) ||
      (call_type == AIM_JNI_CALL_CRITICAL_NATIVE &&
       shorty[0] == 'L')) {
    return false;
  }
  for (uint32_t index = 1; index < shorty_length; ++index) {
    if (!valid_type(shorty[index]) || shorty[index] == 'V') {
      return false;
    }
    // ART forbids references in @CriticalNative declarations. Rejecting them
    // here prevents a regular-JNI implicit-argument thunk from being cached
    // under a critical call key.
    if (call_type == AIM_JNI_CALL_CRITICAL_NATIVE &&
        shorty[index] == 'L') {
      return false;
    }
  }
  return true;
}

struct Key {
  uint64_t image_id;
  uint64_t generation;
  uintptr_t function;
  std::string shorty;
  AimJniCallType call_type;

  bool operator==(const Key& other) const {
    return image_id == other.image_id && generation == other.generation &&
           function == other.function && shorty == other.shorty &&
           call_type == other.call_type;
  }
};

struct KeyHash {
  size_t operator()(const Key& key) const {
    size_t hash = std::hash<uint64_t>{}(key.image_id);
    const auto combine = [&hash](size_t value) {
      hash ^= value + 0x9e3779b97f4a7c15ULL + (hash << 6) + (hash >> 2);
    };
    combine(std::hash<uint64_t>{}(key.generation));
    combine(std::hash<uintptr_t>{}(key.function));
    combine(std::hash<std::string>{}(key.shorty));
    combine(std::hash<int>{}(static_cast<int>(key.call_type)));
    return hash;
  }
};

struct Entry {
  void* thunk;
  AimAndroidFunctionOwnerV1 owner;
};

}  // namespace

struct AimRegisteredNativeCache {
  explicit AimRegisteredNativeCache(
      const AimRegisteredNativeThunkFactoryV1& value)
      : factory(value) {}

  AimRegisteredNativeThunkFactoryV1 factory;
  std::mutex mutex;
  std::unordered_map<Key, Entry, KeyHash> entries;
  std::unordered_set<Key, KeyHash> retired;
};

namespace {

int LookupOwner(AimRegisteredNativeCache* cache,
                const void* function,
                AimAndroidFunctionOwnerV1* owner_out) {
  if (cache == nullptr || function == nullptr || owner_out == nullptr) {
    return -1;
  }
  AimAndroidFunctionOwnerV1 owner{};
  const int status =
      cache->factory.lookup_owner(cache->factory.context, function, &owner);
  if (status <= 0) return status == 0 ? 0 : -1;
  if (!ValidOwner(function, owner)) {
    cache->factory.release_owner(cache->factory.context, &owner);
    return -1;
  }
  *owner_out = owner;
  return 1;
}

}  // namespace

extern "C" AimRegisteredNativeCache*
aim_registered_native_cache_create(
    const AimRegisteredNativeThunkFactoryV1* factory) try {
  if (factory == nullptr ||
      factory->abi_version != AIM_REGISTERED_NATIVE_BRIDGE_ABI_VERSION ||
      factory->struct_size != sizeof(*factory) ||
      factory->lookup_owner == nullptr || factory->release_owner == nullptr ||
      factory->build_thunk == nullptr || factory->destroy_thunk == nullptr) {
    return nullptr;
  }
  return new AimRegisteredNativeCache(*factory);
} catch (...) {
  return nullptr;
}

extern "C" void aim_registered_native_cache_destroy(
    AimRegisteredNativeCache* cache) try {
  if (cache == nullptr) {
    return;
  }
  decltype(cache->entries) doomed;
  {
    std::lock_guard<std::mutex> lock(cache->mutex);
    doomed.swap(cache->entries);
  }
  for (const auto& [key, entry] : doomed) {
    cache->factory.destroy_thunk(cache->factory.context, entry.thunk);
    cache->factory.release_owner(cache->factory.context, &entry.owner);
  }
  delete cache;
} catch (...) {
  std::terminate();
}

extern "C" bool aim_is_android_function_pointer(
    AimRegisteredNativeCache* cache,
    const void* function) {
  AimAndroidFunctionOwnerV1 owner{};
  const bool owned = LookupOwner(cache, function, &owner) == 1;
  if (owned) {
    cache->factory.release_owner(cache->factory.context, &owner);
  }
  return owned;
}

namespace {

// Consumes one acquired lookup lease, transferring it only on publication.
void* GetWithOwner(
    AimRegisteredNativeCache* cache,
    const void* android_function,
    const char* shorty,
    uint32_t shorty_length,
    AimJniCallType call_type,
    AimAndroidFunctionOwnerV1 owner) {
  const auto release = [cache](AimAndroidFunctionOwnerV1* held) {
    cache->factory.release_owner(cache->factory.context, held);
  };
  std::unique_ptr<AimAndroidFunctionOwnerV1, decltype(release)> lease(
      &owner, release);
  if (!ValidShorty(shorty, shorty_length, call_type)) return nullptr;
  Key key{owner.image_id,
          owner.generation,
          reinterpret_cast<uintptr_t>(android_function),
          std::string(shorty, shorty_length),
          call_type};

  const Key generation{
      owner.image_id, owner.generation, 0, "", AIM_JNI_CALL_REGULAR};
  {
    std::lock_guard<std::mutex> lock(cache->mutex);
    if (cache->retired.contains(generation)) return nullptr;
    const auto found = cache->entries.find(key);
    if (found != cache->entries.end()) return found->second.thunk;
  }
  void* thunk = cache->factory.build_thunk(cache->factory.context,
                                          android_function,
                                          &owner,
                                          shorty,
                                          shorty_length,
                                          call_type);
  if (thunk == nullptr) {
    return nullptr;
  }
  const auto destroy = [cache](void* value) {
    cache->factory.destroy_thunk(cache->factory.context, value);
  };
  std::unique_ptr<void, decltype(destroy)> pending(thunk, destroy);
  std::lock_guard<std::mutex> lock(cache->mutex);
  if (cache->retired.contains(generation)) return nullptr;
  const auto winner = cache->entries.find(key);
  if (winner != cache->entries.end()) return winner->second.thunk;
  cache->entries.emplace(std::move(key), Entry{thunk, owner});
  pending.release();
  lease.release();
  return thunk;
}

}  // namespace

extern "C" void* aim_get_registered_native_trampoline(
    AimRegisteredNativeCache* cache,
    const void* android_function,
    const char* shorty,
    uint32_t shorty_length,
    AimJniCallType call_type) try {
  AimAndroidFunctionOwnerV1 owner{};
  if (LookupOwner(cache, android_function, &owner) != 1) return nullptr;
  return GetWithOwner(
      cache, android_function, shorty, shorty_length, call_type, owner);
} catch (...) {
  return nullptr;
}

extern "C" AimRegisteredNativeResolution
aim_resolve_registered_native(
    AimRegisteredNativeCache* cache,
    const void* function,
    bool class_loader_namespace_is_bridged,
    const char* shorty,
    uint32_t shorty_length,
    AimJniCallType call_type,
    const void** callable_out) try {
  if (callable_out != nullptr) *callable_out = nullptr;
  if (cache == nullptr || function == nullptr || callable_out == nullptr) {
    return AIM_REGISTERED_NATIVE_ERROR;
  }
  *callable_out = nullptr;
  AimAndroidFunctionOwnerV1 owner{};
  const int status = LookupOwner(cache, function, &owner);
  if (status < 0) return AIM_REGISTERED_NATIVE_ERROR;
  const bool owned = status == 1;
  if (!class_loader_namespace_is_bridged && !owned) {
    *callable_out = function;
    return AIM_REGISTERED_NATIVE_DIRECT;
  }
  if (!owned) {
    return AIM_REGISTERED_NATIVE_ERROR;
  }
  void* thunk = GetWithOwner(
      cache, function, shorty, shorty_length, call_type, owner);
  if (thunk == nullptr) {
    return AIM_REGISTERED_NATIVE_ERROR;
  }
  *callable_out = thunk;
  return AIM_REGISTERED_NATIVE_TRAMPOLINE;
} catch (...) {
  if (callable_out != nullptr) *callable_out = nullptr;
  return AIM_REGISTERED_NATIVE_ERROR;
}

extern "C" size_t aim_registered_native_cache_retire_image(
    AimRegisteredNativeCache* cache,
    uint64_t image_id,
    uint64_t generation) try {
  if (cache == nullptr || image_id == 0 || generation == 0) {
    return 0;
  }
  std::vector<Entry> doomed;
  {
    std::lock_guard<std::mutex> lock(cache->mutex);
    // Allocate before changing entries, so allocation failure cannot orphan
    // previously removed thunks or their retained image leases.
    doomed.reserve(cache->entries.size());
    cache->retired.insert(
        Key{image_id, generation, 0, "", AIM_JNI_CALL_REGULAR});
    for (auto iterator = cache->entries.begin();
         iterator != cache->entries.end();) {
      if (iterator->first.image_id == image_id &&
          iterator->first.generation == generation) {
        doomed.push_back(iterator->second);
        iterator = cache->entries.erase(iterator);
      } else {
        ++iterator;
      }
    }
  }
  for (const Entry& entry : doomed) {
    cache->factory.destroy_thunk(cache->factory.context, entry.thunk);
    cache->factory.release_owner(cache->factory.context, &entry.owner);
  }
  return doomed.size();
} catch (...) {
  // This ABI has no retirement-failure result. Returning zero could authorize
  // unmapping a still-published target, so fail-stop instead of claiming success.
  std::terminate();
}

extern "C" size_t aim_registered_native_cache_size(
    AimRegisteredNativeCache* cache) try {
  if (cache == nullptr) {
    return 0;
  }
  std::lock_guard<std::mutex> lock(cache->mutex);
  return cache->entries.size();
} catch (...) {
  std::terminate();
}
