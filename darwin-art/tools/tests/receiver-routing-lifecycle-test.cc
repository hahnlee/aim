#include "runtime/framework/input/receiver_routing_lifecycle.h"
#include "runtime/framework/input/input_transport.h"

#include <atomic>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <new>
#include <thread>

namespace {
std::atomic<bool> fail_next{false};
std::atomic<bool> allocation_entered{false};
std::atomic<bool> allocation_release{true};
}
void* operator new(std::size_t size) {
  if (fail_next.exchange(false)) {
    allocation_entered.store(true, std::memory_order_release);
    while (!allocation_release.load(std::memory_order_acquire)) std::this_thread::yield();
    throw std::bad_alloc();
  }
  if (void* ptr = std::malloc(size == 0 ? 1 : size)) return ptr;
  throw std::bad_alloc();
}
void operator delete(void* ptr) noexcept { std::free(ptr); }
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete[](void* ptr) noexcept { ::operator delete(ptr); }

namespace darwin_art::input {
int InputTransport::RemoteEndpointFd() const { std::abort(); }
InputTransportStatus SendInputTransportPacket(InputTransport*, const DarwinArtInputPacket&) {
  std::abort();
}
}

using namespace darwin_art::input;

struct Reentry {
  ReceiverRoutingLifecycle* lifecycle;
  std::unique_ptr<ReceiverRoutingLifecycle>* destroy = nullptr;
  ReceiverRoutingCloseStatus close_status = ReceiverRoutingCloseStatus::kClosed;
  std::atomic<bool> entered{false};
  std::atomic<bool> release{true};
  void Run() {
    // Snapshot and close both reenter while the ledger notification is active.
    assert(lifecycle->Snapshot().operation_pending);
    close_status = lifecycle->Close();
    assert(close_status == ReceiverRoutingCloseStatus::kDeferred);
    entered.store(true, std::memory_order_release);
    while (!release.load(std::memory_order_acquire)) std::this_thread::yield();
    if (destroy != nullptr) destroy->reset();
  }
};

auto Subscribe(const InputRoutingHandle& routing, const std::shared_ptr<Reentry>& context) {
  return SubscribeInputRoutingNotifications(routing, {
      .on_notification = [](void* opaque, const InputRoutingNotification&) {
        auto* reentry = static_cast<Reentry*>(opaque);
        // Publication and then exact self-retirement can each notify.
        if (!reentry->entered.load(std::memory_order_acquire)) reentry->Run();
      }, .context = context.get(), .context_token = context});
}

int main() {
  {
    auto routing = CreateInputRoutingState();
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 90));
    assert(owner.Publish(999).publication.status == InputRoutingPublicationStatus::kCurrentMismatch);
    assert(owner.Publish().status == ReceiverRoutingPublishStatus::kAlreadyAttempted);
    assert(owner.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(owner.Snapshot().retirement.status == InputRoutingRecipientRetirementStatus::kNotPublished);
  }
  {
    auto routing = CreateInputRoutingState();
    assert(PublishInputRoutingRecipient(PrepareInputRoutingRecipient(routing, 91)).Published());
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 92));
    fail_next.store(true);
    bool threw = false;
    try { (void)owner.Publish(); } catch (const std::bad_alloc&) { threw = true; }
    assert(threw && !owner.Snapshot().operation_pending);
    assert(InputRoutingConsumerMatches(routing, 91));
    assert(owner.Publish().publication.Published());
    assert(owner.Close() == ReceiverRoutingCloseStatus::kClosed);
  }
  {
    auto routing = CreateInputRoutingState();
    assert(PublishInputRoutingRecipient(PrepareInputRoutingRecipient(routing, 93)).Published());
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 94));
    allocation_entered.store(false);
    allocation_release.store(false);
    // Construct thread before arming allocator to target actual ledger reserve.
    std::atomic<bool> begin{false};
    std::thread publishing([&] {
      while (!begin.load(std::memory_order_acquire)) std::this_thread::yield();
      bool threw = false;
      try { (void)owner.Publish(); } catch (const std::bad_alloc&) { threw = true; }
      assert(threw);
    });
    fail_next.store(true);
    begin.store(true, std::memory_order_release);
    while (!allocation_entered.load(std::memory_order_acquire)) std::this_thread::yield();
    assert(owner.Close() == ReceiverRoutingCloseStatus::kDeferred);
    allocation_release.store(true, std::memory_order_release);
    publishing.join();
    assert(owner.Snapshot().logical_closed);
    assert(!owner.Snapshot().operation_pending);
    assert(InputRoutingConsumerMatches(routing, 93));
    assert(owner.Publish().status == ReceiverRoutingPublishStatus::kClosed);
  }
  {
    auto routing = CreateInputRoutingState();
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 1));
    assert(owner.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(owner.Publish().status == ReceiverRoutingPublishStatus::kClosed);
    assert(owner.Snapshot().logical_closed);
    assert(!InputRoutingConsumerMatches(routing, 1));
  }
  {
    auto routing = CreateInputRoutingState();
    ReceiverRoutingLifecycle first(PrepareInputRoutingRecipient(routing, 2));
    assert(first.Publish().publication.Published());
    assert(first.Publish().status == ReceiverRoutingPublishStatus::kAlreadyAttempted);
    ReceiverRoutingLifecycle second(PrepareInputRoutingRecipient(routing, 3));
    auto publication = second.Publish();
    assert(publication.publication.predecessor.ConsumerId() == 2);
    assert(first.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(first.Snapshot().retirement.status == InputRoutingRecipientRetirementStatus::kHistorical);
    assert(InputRoutingConsumerMatches(routing, 3));
    assert(second.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(second.Snapshot().retirement.ticket.ConsumerId() == 3);
    assert(second.Snapshot().publication.predecessor.ConsumerId() == 2);
  }
  {
    auto routing = CreateInputRoutingState();
    assert(PublishInputRoutingRecipient(PrepareInputRoutingRecipient(routing, 4)).Published());
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 5));
    auto context = std::make_shared<Reentry>();
    context->lifecycle = &owner;
    context->release.store(false);
    auto subscription = Subscribe(routing, context);
    std::thread publishing([&] { assert(owner.Publish().publication.Published()); });
    while (!context->entered.load(std::memory_order_acquire)) std::this_thread::yield();
    assert(owner.Close() == ReceiverRoutingCloseStatus::kDeferred);
    assert(owner.Publish().status == ReceiverRoutingPublishStatus::kClosed);
    context->release.store(true, std::memory_order_release);
    publishing.join();
    assert(owner.Snapshot().logical_closed);
    assert(owner.Snapshot().retirement.ticket.ConsumerId() == 5);
    assert(!InputRoutingConsumerMatches(routing, 5));
  }
  {
    auto routing = CreateInputRoutingState();
    assert(PublishInputRoutingRecipient(PrepareInputRoutingRecipient(routing, 6)).Published());
    auto owner = std::make_unique<ReceiverRoutingLifecycle>(PrepareInputRoutingRecipient(routing, 7));
    auto retained = owner->Retain();
    auto context = std::make_shared<Reentry>();
    context->lifecycle = owner.get();
    context->destroy = &owner;
    auto subscription = Subscribe(routing, context);
    const auto result = owner->Publish();
    assert(owner == nullptr);
    assert(result.publication.Published());
    assert(result.publication.predecessor.ConsumerId() == 6);
    assert(!InputRoutingConsumerMatches(routing, 7));
    assert(retained.Snapshot().logical_closed);
    assert(retained.Snapshot().retirement.ticket.ConsumerId() == 7);
  }
  {
    auto routing = CreateInputRoutingState();
    ReceiverRoutingLifecycle owner(PrepareInputRoutingRecipient(routing, 8));
    std::weak_ptr<InputRoutingState> weak = routing;
    routing.reset();
    assert(!weak.expired());
    assert(owner.Publish().publication.Published());
    assert(owner.Close() == ReceiverRoutingCloseStatus::kClosed);
  }
  {
    auto routing = CreateInputRoutingState();
    auto owner = std::make_unique<ReceiverRoutingLifecycle>(PrepareInputRoutingRecipient(routing, 9));
    auto retained = owner->Retain();
    assert(owner->Publish().publication.Published());
    auto context = std::make_shared<Reentry>();
    context->lifecycle = owner.get();
    context->destroy = &owner;
    auto subscription = Subscribe(routing, context);
    assert(owner->Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(owner == nullptr);
    assert(retained.Snapshot().logical_closed);
    assert(QueryInputRoutingRetirement(retained.Snapshot().retirement.ticket).status ==
           InputRoutingRetirementStatus::kSettled);
  }
  std::puts("receiver routing lifecycle: exact token/deferred close/reentry/destruction PASS");
}
