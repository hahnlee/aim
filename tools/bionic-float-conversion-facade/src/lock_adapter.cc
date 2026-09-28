#include <cstdlib>
#include <memory>
#include <mutex>
#include <unordered_map>

extern "C" {
void* __dtoa_locks[2] = {};
}

namespace {

struct LockRegistry {
  std::mutex mutex;
  std::unordered_map<void*, std::unique_ptr<std::mutex>> locks;
};

LockRegistry& Registry() {
  static LockRegistry registry;
  return registry;
}

std::mutex* Lookup(void* identity) {
  LockRegistry& registry = Registry();
  std::lock_guard<std::mutex> guard(registry.mutex);
  auto found = registry.locks.find(identity);
  if (found != registry.locks.end()) return found->second.get();
  std::unique_ptr<std::mutex> created(new (std::nothrow) std::mutex);
  if (created == nullptr) std::abort();
  std::mutex* result = created.get();
  try {
    registry.locks.emplace(identity, std::move(created));
  } catch (...) {
    std::abort();
  }
  return result;
}

}  // namespace

extern "C" void aim_gdtoa_lock(void* identity) {
  Lookup(identity)->lock();
}

extern "C" void aim_gdtoa_unlock(void* identity) {
  Lookup(identity)->unlock();
}
