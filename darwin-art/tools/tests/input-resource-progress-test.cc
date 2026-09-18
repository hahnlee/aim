#include "runtime/framework/input/input_resource_progress.h"

#include <atomic>
#include <cassert>
#include <cstdint>
#include <cstdlib>
#include <memory>
#include <new>
#include <thread>

using Source = darwin_art::input::InputResourceProgressSource;
using Event = darwin_art::input::InputResourceProgress;
using Kind = darwin_art::input::InputResourceProgressKind;

namespace {

std::atomic<int> g_fail_at{-1};
std::atomic<int> g_allocations{0};

void* Allocate(std::size_t size) {
  const int allocation = g_allocations.fetch_add(1) + 1;
  if (allocation == g_fail_at.load()) throw std::bad_alloc();
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}

struct Observer {
  std::atomic<int> calls{0};
  std::atomic<std::uint64_t> last_revision{0};
  std::atomic<int> last_kind{-1};
};

void Observe(void* opaque, Event event) noexcept {
  auto* observer = static_cast<Observer*>(opaque);
  observer->last_revision.store(event.revision, std::memory_order_relaxed);
  observer->last_kind.store(static_cast<int>(event.kind),
                            std::memory_order_relaxed);
  observer->calls.fetch_add(1, std::memory_order_relaxed);
}

struct Reentry {
  Source* source = nullptr;
  Source::SubscriptionHandle* lease = nullptr;
  std::shared_ptr<Observer> nested_context;
  std::atomic<int> calls{0};
  bool entered = false;
};

void Reenter(void* opaque, Event) noexcept {
  auto* reentry = static_cast<Reentry*>(opaque);
  reentry->calls.fetch_add(1, std::memory_order_relaxed);
  if (reentry->entered) return;
  reentry->entered = true;
  auto nested = reentry->source->Subscribe(
      Observe, std::weak_ptr<void>(reentry->nested_context));
  assert(nested);
  reentry->source->Notify(Kind::kLocalCapacity);
  *reentry->lease = Source::SubscriptionHandle{};
  // The nested handle is dropped after reentrant publication and notification.
}

void Noop(void*, Event) noexcept {}

}  // namespace

void* operator new(std::size_t size) { return Allocate(size); }
void* operator new[](std::size_t size) { return Allocate(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept {
  std::free(pointer);
}
void operator delete[](void* pointer, std::size_t) noexcept {
  std::free(pointer);
}

int main() {
  Source source;
  assert(source.Revision() == 0);

  auto observer = std::make_shared<Observer>();
  auto lease = source.Subscribe(
      Observe, std::weak_ptr<void>(observer));
  assert(lease);
  source.Notify(Kind::kTxAccepted);
  assert(source.Revision() == 1);
  assert(observer->calls == 1 && observer->last_revision == 1 &&
         observer->last_kind == static_cast<int>(Kind::kTxAccepted));

  // Dropping a lease unsubscribes discovery without requiring source-lock
  // mutation. An in-flight callback may still finish, but later notifications
  // do not invoke it.
  lease = Source::SubscriptionHandle{};
  source.Notify(Kind::kTxAdvanced);
  assert(observer->calls == 1 && source.Revision() == 2);

  // Expired contexts and expired leases are pruned by the next allocating
  // Subscribe operation, while the old immutable chain stays transactional.
  auto expired_context = std::make_shared<Observer>();
  auto expired_lease = source.Subscribe(
      Observe, std::weak_ptr<void>(expired_context));
  assert(expired_lease);
  expired_context.reset();
  auto live_context = std::make_shared<Observer>();
  auto live_lease = source.Subscribe(Observe, std::weak_ptr<void>(live_context));
  assert(live_lease);
  source.Notify(Kind::kTxAdvanced);
  assert(live_context->calls == 1);

  // Callback reentry can subscribe, notify and drop the outer lease without
  // observing a source mutex or corrupting the immutable snapshot.
  Source reentrant_source;
  Source::SubscriptionHandle reentrant_lease;
  Reentry reentry;
  reentry.source = &reentrant_source;
  reentry.lease = &reentrant_lease;
  reentry.nested_context = std::make_shared<Observer>();
  auto callback_context = std::shared_ptr<Reentry>(&reentry, [](Reentry*) {});
  reentrant_lease = reentrant_source.Subscribe(
      Reenter, std::weak_ptr<void>(callback_context));
  assert(reentrant_lease);
  reentrant_source.Notify(Kind::kTxAccepted);
  assert(reentry.calls >= 1 && !reentrant_lease);

  // Concurrent notifications may overlap, but every event gets a distinct
  // revision and callbacks remain outside the source mutex.
  Source overlapping_source;
  auto overlapping_context = std::make_shared<Observer>();
  auto overlapping_lease = overlapping_source.Subscribe(
      Observe, std::weak_ptr<void>(overlapping_context));
  assert(overlapping_lease);
  std::thread first([&] { overlapping_source.Notify(Kind::kTxAccepted); });
  std::thread second([&] { overlapping_source.Notify(Kind::kTxAdvanced); });
  first.join();
  second.join();
  assert(overlapping_source.Revision() == 2 &&
         overlapping_context->calls == 2);

  // Notify itself does not allocate: a forced next allocation cannot prevent
  // delivery to an already-published subscriber.
  g_allocations.store(0);
  g_fail_at.store(1);
  const auto before_notify_calls = overlapping_context->calls.load();
  overlapping_source.Notify(Kind::kTerminal);
  g_fail_at.store(-1);
  assert(overlapping_context->calls == before_notify_calls + 1);

  // Allocation failure while pruning/building a new chain cannot publish a
  // partial subscription; the preexisting subscriber still receives events.
  Source failure_source;
  auto existing_context = std::make_shared<Observer>();
  auto existing_lease = failure_source.Subscribe(
      Observe, std::weak_ptr<void>(existing_context));
  assert(existing_lease);
  g_allocations.store(0);
  g_fail_at.store(2);
  auto failed_lease = failure_source.Subscribe(
      Noop, std::weak_ptr<void>(existing_context));
  g_fail_at.store(-1);
  assert(!failed_lease && failure_source.Revision() == 0);
  failure_source.Notify(Kind::kLocalCapacity);
  assert(existing_context->calls == 1);

  return 0;
}
