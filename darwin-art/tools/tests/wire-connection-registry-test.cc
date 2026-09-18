#include "compat/binder/wire_connection_registry.h"

#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <thread>

namespace {

struct Payload final {
  std::function<void()> on_destroy;
  ~Payload() {
    if (on_destroy) on_destroy();
  }
};

using Registry =
    darwin_art::binder::WireConnectionRegistry<Payload>;
using Handle = Registry::Handle;

void TestBasicIdentityAndExactRetirement() {
  Registry registry;
  auto tx = registry.Lock();

  assert(!tx.FindEstablished(7));
  assert(!tx.FindExact(7, 1));
  assert(!tx.CreateOwned(-1, std::make_shared<Payload>()));
  assert(!tx.CreateOwned(7, std::shared_ptr<Payload>()));

  auto first_payload = std::make_shared<Payload>();
  auto first = tx.CreateOwned(7, first_payload);
  assert(first);
  assert(first->fd == 7 && first->payload == first_payload);
  const uint64_t first_generation = first->Generation();
  assert(first_generation != 0);
  assert(tx.FindEstablished(7) == first);
  assert(tx.FindExact(7, first_generation) == first);
  assert(!tx.FindExact(7, first_generation + 1));

  // Existing fds are never replaced, even when a different payload is
  // supplied.  This also checks that creation has no fallback fd behavior.
  auto replacement_payload = std::make_shared<Payload>();
  assert(!tx.CreateOwned(7, replacement_payload));
  assert(tx.FindEstablished(7) == first);

  auto stale = first;
  assert(tx.RetireExact(stale));
  assert(!first->lifetime->Live());
  assert(!tx.FindEstablished(7));
  assert(!tx.RetireExact(stale));

  auto successor_payload = std::make_shared<Payload>();
  auto successor = tx.CreateOwned(7, successor_payload);
  assert(successor);
  assert(successor->Generation() != first_generation);
  assert(!tx.RetireExact(stale));  // stale identity cannot retire successor
  assert(tx.FindExact(7, successor->Generation()) == successor);
  assert(tx.RetireExact(successor));
}

void TestStrongPinAcrossWaitAndPeerRecreate() {
  Registry registry;
  auto old_payload = std::make_shared<Payload>();
  std::atomic<bool> old_destroyed{false};
  old_payload->on_destroy = [&] { old_destroyed.store(true); };

  auto waiter = registry.Lock();
  auto old = waiter.CreateOwned(19, old_payload);
  assert(old);
  const uint64_t old_generation = old->Generation();
  old_payload.reset();
  Handle retire_token = old;
  old.reset();  // the transaction's pin is now the only old identity lease

  std::atomic<bool> recreated{false};
  std::atomic<bool> peer_pin_destroyed{false};
  std::thread peer([&] {
    {
      auto peer_tx = registry.Lock();
      assert(peer_tx.RetireExact(retire_token));
      retire_token.reset();
      auto successor = peer_tx.CreateOwned(19, std::make_shared<Payload>());
      assert(successor && successor->Generation() != old_generation);

      // This identity is retired before peer_tx exits.  Its payload must be
      // released when that peer transaction drops its pins, rather than being
      // appended to the waiter that temporarily released the mutex.
      auto transient_payload = std::make_shared<Payload>();
      transient_payload->on_destroy = [&] { peer_pin_destroyed.store(true); };
      auto transient = peer_tx.CreateOwned(29, transient_payload);
      assert(transient && peer_tx.RetireExact(transient));
      transient.reset();
      transient_payload.reset();
    }
    assert(peer_pin_destroyed.load());
    recreated.store(true);
    auto signal = registry.Lock();
    signal.NotifyAll();
  });

  waiter.Wait([&] { return recreated.load(); });
  peer.join();
  assert(!old_destroyed.load());
  assert(!waiter.FindExact(19, old_generation));
  auto current = waiter.FindEstablished(19);
  assert(current && current->Generation() != old_generation);
  assert(waiter.RetireExact(retire_token) == false);
  assert(waiter.RetireExact(current));
}

void TestNestedWaitAndOutOfLockPayloadDestruction() {
  Registry registry;
  std::atomic<bool> destructor_reentered{false};
  auto payload = std::make_shared<Payload>();
  payload->on_destroy = [&] {
    // If any recursive lock level is still held, this join would deadlock.
    // The registry contract requires all pin releases to happen after unlock.
    std::thread peer([&] {
      auto tx = registry.Lock();
      destructor_reentered.store(true);
    });
    peer.join();
  };

  {
    auto outer = registry.Lock();
    auto handle = outer.CreateOwned(23, payload);
    assert(handle);
    {
      auto nested = registry.Lock();
      assert(nested.FindEstablished(23) == handle);
      bool threw = false;
      try {
        nested.Wait([] { return true; });
      } catch (const std::logic_error&) {
        threw = true;
      }
      assert(threw);
      assert(nested.RetireExact(handle));
    }
    payload.reset();
    handle.reset();
    // The outer transaction still owns the deferred nested and outer pins.
  }
  assert(destructor_reentered.load());
}

}  // namespace

int main() {
  TestBasicIdentityAndExactRetirement();
  TestStrongPinAcrossWaitAndPeerRecreate();
  TestNestedWaitAndOutOfLockPayloadDestruction();
  return 0;
}
