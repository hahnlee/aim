#include "../../compat/binder/wire_channel_lifetime.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <limits>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_set>
#include <vector>

namespace darwin_art::binder {

// This peer is defined only in the test executable.  It can position the
// private allocator at the terminal boundary without adding a production
// reset/exported test hook.
class WireChannelLifetimeTestPeer final {
 public:
  static void SetNextGeneration(uint64_t generation) noexcept {
    WireChannelLifetime::next_generation_.store(generation,
                                                  std::memory_order_relaxed);
  }
  static uint64_t NextGeneration() noexcept {
    return WireChannelLifetime::next_generation_.load(
        std::memory_order_relaxed);
  }
};

}  // namespace darwin_art::binder

namespace {

using darwin_art::binder::WireChannelLifetime;
using darwin_art::binder::WireChannelLifetimeTestPeer;

void BasicAdmissionAndSealing() {
  const auto owner = WireChannelLifetime::Create();
  assert(owner);
  const uint64_t generation = owner->Generation();
  assert(generation != 0);
  assert(owner->Live());
  assert(owner->Matches(generation));
  assert(!owner->Matches(0));
  assert(!owner->Matches(generation - 1));
  assert(!owner->Matches(generation + 1));
  assert(owner->Seal());
  assert(!owner->Seal());
  assert(!owner->Live());
  assert(!owner->Matches(generation));
}

void ConcurrentCreationHasUniqueGenerations() {
  constexpr size_t kThreads = 8;
  constexpr size_t kPerThread = 256;
  std::mutex mutex;
  std::unordered_set<uint64_t> generations;
  std::vector<std::thread> threads;
  threads.reserve(kThreads);
  for (size_t thread = 0; thread < kThreads; ++thread) {
    threads.emplace_back([&] {
      std::vector<std::shared_ptr<WireChannelLifetime>> owners;
      owners.reserve(kPerThread);
      for (size_t i = 0; i < kPerThread; ++i) {
        auto owner = WireChannelLifetime::Create();
        assert(owner);
        std::lock_guard lock(mutex);
        assert(generations.insert(owner->Generation()).second);
        owners.push_back(std::move(owner));
      }
    });
  }
  for (auto& thread : threads) {
    thread.join();
  }
  assert(generations.size() == kThreads * kPerThread);
}

void ConcurrentLiveAndSealUsesAtomicState() {
  auto owner = WireChannelLifetime::Create();
  assert(owner);
  const uint64_t generation = owner->Generation();
  std::atomic<bool> start{false};
  std::atomic<uint64_t> live_observations{0};
  std::vector<std::thread> readers;
  for (size_t i = 0; i < 8; ++i) {
    readers.emplace_back([&] {
      while (!start.load(std::memory_order_acquire)) {
      }
      for (;;) {
        if (owner->Live()) {
          live_observations.fetch_add(1, std::memory_order_relaxed);
        }
        if (!owner->Matches(generation)) {
          break;
        }
      }
    });
  }
  std::thread sealer([&] {
    while (!start.load(std::memory_order_acquire)) {
    }
    // Establish an actual live observation before racing the seal; scheduling
    // the sealer first must not make this correctness test spuriously fail.
    while (live_observations.load(std::memory_order_acquire) == 0) {
      std::this_thread::yield();
    }
    assert(owner->Seal());
    assert(!owner->Seal());
  });
  start.store(true, std::memory_order_release);
  sealer.join();
  for (auto& reader : readers) {
    reader.join();
  }
  assert(live_observations.load(std::memory_order_relaxed) > 0);
  assert(!owner->Live());
  assert(!owner->Matches(generation));
}

void RetainedOwnerRemainsSealed() {
  auto owner = WireChannelLifetime::Create();
  assert(owner);
  const uint64_t generation = owner->Generation();
  auto retained = owner;
  std::weak_ptr<WireChannelLifetime> weak = owner;
  assert(owner->Seal());
  owner.reset();
  // A retained shared owner is the lifetime authority after the caller's
  // admission reference is dropped; a sealed owner never becomes live again.
  assert(weak.lock() == retained);
  assert(!retained->Live());
  assert(!retained->Matches(generation));
  assert(!retained->Seal());
  retained.reset();
  assert(weak.expired());
}

void ExhaustionIsDeterministicAndNeverReuses() {
  constexpr uint64_t kMax = std::numeric_limits<uint64_t>::max();
  WireChannelLifetimeTestPeer::SetNextGeneration(kMax - 1);
  auto last = WireChannelLifetime::Create();
  assert(last);
  assert(last->Generation() == kMax - 1);
  assert(WireChannelLifetimeTestPeer::NextGeneration() == kMax);
  assert(!WireChannelLifetime::Create());
  assert(!WireChannelLifetime::Create());
  assert(WireChannelLifetimeTestPeer::NextGeneration() == kMax);
  assert(last->Seal());
  // Sealing the final owner does not reopen the globally exhausted identity
  // space and cannot make its generation available to a new owner.
  assert(!WireChannelLifetime::Create());
}

}  // namespace

int main() {
  BasicAdmissionAndSealing();
  ConcurrentCreationHasUniqueGenerations();
  ConcurrentLiveAndSealUsesAtomicState();
  RetainedOwnerRemainsSealed();
  ExhaustionIsDeterministicAndNeverReuses();
  return 0;
}
