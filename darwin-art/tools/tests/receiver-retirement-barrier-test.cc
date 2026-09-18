#include "runtime/framework/input/receiver_retirement_barrier.h"
#include "runtime/framework/input/pending_receiver_retirement.h"
#include "runtime/framework/input/receiver_retirement_driver.h"
#include "tools/tests/input-owner-task-fixture.h"

#include <algorithm>
#include <atomic>
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <new>
#include <stdexcept>
#include <thread>

static bool fail_allocations = false;
void* operator new(std::size_t size) {
  if (fail_allocations) throw std::bad_alloc();
  if (void* allocation = std::malloc(size == 0 ? 1 : size)) return allocation;
  throw std::bad_alloc();
}
void operator delete(void* ptr) noexcept { std::free(ptr); }

extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int*) { std::abort(); }
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*, size_t, int) { std::abort(); }
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t, int) { std::abort(); }
extern "C" int darwin_art_bionic_socket_broker_close(int) { std::abort(); }
extern "C" int darwin_art_bionic_errno_load() { std::abort(); }

namespace {
size_t send_budget = 0;
bool send_terminal = false;
int closes = 0;
darwin_art::input::PendingReceiverRetirementHandle close_observer;
bool destructor_reentered = false;
bool fail_remove = false;
struct Registration {
  int fd = -1;
  darwin_art::looper::FdCallback callback = nullptr;
  void* context = nullptr;
  void* owner = nullptr;
  darwin_art::looper::OwnerRelease release = nullptr;
  int events = 0;
};
Registration registrations[8];
DarwinArtPointerEventV2 PointerDown(uint64_t sequence) {
  DarwinArtPointerEventV2 event{};
  event.version = 2;
  event.size = sizeof(event);
  event.action = DARWIN_ART_POINTER_DOWN;
  event.sequence = sequence;
  event.pointer_id = 1;
  event.pointer_count = 1;
  event.x = 10.0f;
  event.y = 20.0f;
  event.raw_x = event.x;
  event.raw_y = event.y;
  event.pressure = 1.0f;
  event.size_value = 1.0f;
  return event;
}
intptr_t Send(int, const void*, size_t count, int) {
  if (send_terminal) { errno = EPIPE; return -1; }
  if (send_budget == 0) { errno = EAGAIN; return -1; }
  const auto sent = std::min(count, send_budget);
  send_budget -= sent;
  return static_cast<intptr_t>(sent);
}
intptr_t Receive(int, void*, size_t, int) { errno = EAGAIN; return -1; }
int Close(int) {
  ++closes;
  if (close_observer != nullptr) {
    assert(darwin_art::input::IsPendingReceiverRetirementRetained(close_observer));
    destructor_reentered = true;
  }
  return 0;
}
int Error() { return errno == EAGAIN ? 11 : errno; }
}
namespace darwin_art::looper {
int AddFdOwned(void*, int fd, int, int events, FdCallback callback, void* context,
               void* owner, OwnerRelease release) {
  for (auto& slot : registrations) {
    if (slot.fd != -1) continue;
    slot = {fd, callback, context, owner, release, events};
    return 1;
  }
  std::abort();
}
int RemoveFdIfOwned(void*, int fd, FdCallback callback, void* context) {
  if (fail_remove) return -1;
  for (auto& slot : registrations) {
    if (slot.fd != fd || slot.callback != callback || slot.context != context) continue;
    const auto previous = slot;
    slot = {};
    previous.release(previous.owner);
    return 1;
  }
  return 0;
}
}

using namespace darwin_art::input;
using darwin_art::DarwinArtInputEnqueueResult;
using Status = ReceiverRetirementBarrierStatus;
struct Fixture {
  std::shared_ptr<InputTransport> transport = std::make_shared<InputTransport>(
      InputTransportIo{Send, Receive, Close, Error}, false);
  InputRoutingHandle routing = CreateInputRoutingState();
  InputRoutingEndpointHandle endpoint;
  ReceiverRoutingLifecycle lifecycle;
  ReceiverAdmission admission;
  ReceiverEndpointBinding binding;
  ReceiverRetirementBarrier barrier;
  explicit Fixture(ReceiverId id, bool owns_descriptors = false)
      : transport(std::make_shared<InputTransport>(InputTransportIo{Send, Receive, Close, Error}, owns_descriptors)),
        endpoint(std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{transport, id})),
        lifecycle(PrepareInputRoutingRecipient(routing, id, endpoint)),
        barrier(lifecycle, admission.RetainRetirement(), binding.RetainRetirement(), transport) {
    AdoptRemoteInputTransport(transport.get(), 81);
  }
  void Quiesce() {
    assert(admission.Retire());
    assert(admission.CompleteCleanup());
    assert(binding.Retire());
  }
};

int main() {
  {
    // Real driver task + claim/pump + resource completion: no TX/routing
    // event arrives after the final admitted JNI cleanup completes.
    Fixture fixture(9901);
    assert(fixture.admission.Admit());
    auto record = PreparePendingReceiverRetirement(fixture.lifecycle,
        fixture.admission.RetainRetirement(), fixture.binding.RetainRetirement(), fixture.transport);
    auto driver = ReceiverRetirementDriver::Prepare(record,
        reinterpret_cast<void*>(1), fixture.transport, 81, 9901, fixture.routing);
    assert(driver && AttachPendingReceiverRetirementDriver(record, driver));
    assert(EnlistPendingReceiverRetirement(record));
    assert(PublishPendingReceiverRetirement(record).publication.Published());
    assert(driver->Close());
    // Logical revocation cannot wait for the owner task.
    assert(fixture.lifecycle.Snapshot().logical_closed);
    assert(!fixture.admission.Retire());
    assert(fixture.binding.Retire());
    for (int i = 0; i < 4; ++i) (void)darwin_art::test::DispatchInputOwnerTasks();
    assert(IsPendingReceiverRetirementRetained(record) && !driver->IsQuiescent());
    for (const auto& slot : registrations)
      if (slot.fd == 81) assert((slot.events & 0x0002) == 0);
    assert(fixture.admission.Release());
    assert(IsPendingReceiverRetirementRetained(record));
    assert(fixture.admission.CompleteCleanup());
    for (int i = 0; i < 16; ++i) (void)darwin_art::test::DispatchInputOwnerTasks();
    assert(driver->IsQuiescent() && !IsPendingReceiverRetirementRetained(record));
  }
  {
    // Final binding completion alone must wake the independent driver; a
    // failed provider removal cannot prematurely release its authority.
    Fixture fixture(9902);
    assert(fixture.binding.Register(reinterpret_cast<void*>(1), fixture.transport,
                                    81, 81, {}, 9902, 9902));
    auto record = PreparePendingReceiverRetirement(fixture.lifecycle,
        fixture.admission.RetainRetirement(), fixture.binding.RetainRetirement(), fixture.transport);
    auto driver = ReceiverRetirementDriver::Prepare(record,
        reinterpret_cast<void*>(1), fixture.transport, 81, 9902, fixture.routing);
    assert(driver && AttachPendingReceiverRetirementDriver(record, driver));
    assert(EnlistPendingReceiverRetirement(record));
    assert(PublishPendingReceiverRetirement(record).publication.Published());
    assert(driver->Close());
    assert(fixture.admission.Retire() && fixture.admission.CompleteCleanup());
    fail_remove = true;
    (void)fixture.binding.Retire();
    assert(!fixture.binding.RetainRetirement().IsQuiescent());
    for (int i = 0; i < 2; ++i) (void)darwin_art::test::DispatchInputOwnerTasks();
    assert(IsPendingReceiverRetirementRetained(record) && !driver->IsQuiescent());
    fail_remove = false;
    for (int i = 0; i < 16; ++i) (void)darwin_art::test::DispatchInputOwnerTasks();
    assert(fixture.binding.RetainRetirement().IsQuiescent());
    assert(driver->IsQuiescent() && !IsPendingReceiverRetirementRetained(record));
  }
  {
    // An OOM while materializing a stream CANCEL leaves only ledger
    // cancel_pending (no ActionRecord). The barrier's nested kRunnable status
    // must grant one bounded retry, then stop internally until an explicit
    // external request opens the next epoch.
    Fixture fixture(9903);
    auto record = PreparePendingReceiverRetirement(
        fixture.lifecycle, fixture.admission.RetainRetirement(),
        fixture.binding.RetainRetirement(), fixture.transport);
    auto driver = ReceiverRetirementDriver::Prepare(
        record, reinterpret_cast<void*>(1), fixture.transport, 81, 9903,
        fixture.routing);
    assert(driver && AttachPendingReceiverRetirementDriver(record, driver));
    assert(EnlistPendingReceiverRetirement(record));
    assert(PublishPendingReceiverRetirement(record).publication.Published());
    (void)PublishInputRoutingWmsFrame(fixture.routing, 0, 0, 100, 100, true);
    assert(SetInputRoutingFocus(fixture.routing, 9903));
    InputRoutingAdmission down;
    assert(RouteFrameworkPointerPacket(PointerDown(1), &down) ==
           DarwinArtInputEnqueueResult::kQueued);
    assert(CommitInputRoutingPacket(std::move(down), true, true));

    // Keep a second DOWN admitted in the real routing FIFO.  Closing while
    // this send is admitted records the stream's deferred CANCEL obligation
    // without allocating an ActionRecord.  The orphan is therefore allowed
    // to become active while allocations still work; the fault below can only
    // exercise the later cancellation materialization path.
    InputRoutingAdmission admitted_down;
    assert(RouteFrameworkPointerPacket(PointerDown(2), &admitted_down) ==
           DarwinArtInputEnqueueResult::kQueued);
    InputRoutingInflightLease inflight;
    assert(ReserveInputRoutingPacket(std::move(admitted_down), false,
                                     &inflight));
    assert(AcquireInputRoutingHead(fixture.routing, &inflight));
    assert(AdmitInputRoutingTransportSend(inflight));
    assert(fixture.admission.Retire() && fixture.admission.CompleteCleanup());
    assert(fixture.binding.Retire());

    // Activation is intentionally outside the fault window.  The admitted
    // send keeps the barrier in kRouting and prevents Close from attempting a
    // CANCEL allocation before the provider orphan is active.
    assert(driver->Close());
    assert(darwin_art::test::DispatchInputOwnerTasks() == 1);
    bool orphan_registered = false;
    for (const auto& slot : registrations)
      orphan_registered |= slot.fd == 81;
    assert(orphan_registered);
    fail_allocations = true;
    // Completion after exact logical close releases the admitted send and
    // retries the deferred CANCEL.  QueueLedgerCancellationLocked catches
    // this injected allocation failure, leaving only the stream ledger.
    assert(!CompleteInputRoutingPacket(std::move(inflight),
                                      InputRoutingDeliveryResult::kAccepted));
    assert(fixture.barrier.Poll().status == Status::kRouting);
    assert(fixture.barrier.Poll().routing.status ==
           InputRoutingRetirementStatus::kRunnable);
    assert(!InputRoutingHasPendingCancellation(fixture.routing));
    // The completion notification gets one owner turn, and the routing
    // predicate grants exactly one bounded retry. Persistent OOM then stops
    // without an internal dispatch spin.
    assert(darwin_art::test::DispatchInputOwnerTasks() == 1);
    assert(darwin_art::test::DispatchInputOwnerTasks() == 1);
    assert(darwin_art::test::DispatchInputOwnerTasks() == 0);
    fail_allocations = false;
    assert(driver->Request());
    for (int i = 0; i < 16; ++i)
      (void)darwin_art::test::DispatchInputOwnerTasks();
    assert(driver->IsQuiescent() && !IsPendingReceiverRetirementRetained(record));
  }
  std::puts("retirement driver: synchronous close/empty OUTPUT/admission and binding final wakes PASS");
  {
    Fixture fixture(1);
    assert(fixture.barrier.Poll().status == Status::kLifecycle);
    assert(fixture.lifecycle.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(fixture.admission.Admit());
    assert(!fixture.admission.Retire());
    assert(fixture.barrier.Poll().status == Status::kAdmission);
    assert(fixture.admission.Release());
    // Cleanup election alone cannot authorize handoff.
    assert(fixture.barrier.Poll().status == Status::kAdmission);
    assert(fixture.admission.CompleteCleanup());
    assert(fixture.barrier.Poll().status == Status::kBinding);
    assert(fixture.binding.Retire());
    assert(SendInputTransportAck(fixture.transport.get(), 1, true) == InputTransportStatus::kAccepted);
    const auto pending = fixture.barrier.Poll();
    assert(pending.status == Status::kTx && pending.fence_captured);
    assert(SendInputTransportAck(fixture.transport.get(), 2, true) == InputTransportStatus::kAccepted);
    send_budget = 16;  // One actual versioned ACK frame, not successor ACK.
    assert(FlushInputTransport(fixture.transport.get()) == InputTransportStatus::kBackpressured);
    assert(fixture.transport->HasPendingTx());
    assert(fixture.barrier.Poll().status == Status::kReady);
    // Post-preparation proof/retain/fence queries cannot allocate.
    fail_allocations = true;
    auto retained = fixture.barrier.Retain();
    assert(retained.Poll().status == Status::kReady);
    fail_allocations = false;
    send_budget = std::numeric_limits<size_t>::max();
    assert(FlushInputTransport(fixture.transport.get()) == InputTransportStatus::kAccepted);
    send_budget = 0;
  }
  {
    Fixture fixture(2);
    assert(fixture.lifecycle.Publish().publication.Published());
    PublishInputRoutingWmsFrame(fixture.routing, 0, 0, 100, 100, true);
    SetInputRoutingTransportReady(fixture.routing, 2, true);
    assert(SetInputRoutingFocus(fixture.routing, 2));
    DarwinArtPointerEventV2 pointer{};
    pointer.version = 2;
    pointer.size = sizeof(pointer);
    pointer.pointer_count = 1;
    pointer.action = DARWIN_ART_POINTER_DOWN;
    pointer.x = 20;
    pointer.y = 20;
    InputRoutingAdmission admission;
    assert(RouteFrameworkPointerPacket(pointer, &admission) == DarwinArtInputEnqueueResult::kQueued);
    InputRoutingInflightLease sending;
    assert(ReserveInputRoutingPacket(std::move(admission), false, &sending));
    assert(AcquireInputRoutingHead(fixture.routing, &sending));
    assert(AdmitInputRoutingTransportSend(sending));
    const auto packet = *sending.Packet();
    assert(fixture.lifecycle.Close() == ReceiverRoutingCloseStatus::kClosed);
    fixture.Quiesce();
    TerminateInputRoutingTransport(fixture.routing, fixture.endpoint);
    const auto ticket = fixture.lifecycle.Snapshot().retirement.ticket;
    const auto query = QueryInputRoutingRetirement(ticket);
    assert(query.status == InputRoutingRetirementStatus::kTerminal && query.admitted_send);
    const auto held = fixture.barrier.Poll();
    assert(held.status == Status::kRouting && !held.fence_captured);
    // Real late transport acceptance after terminal routing belongs in fence.
    assert(SendInputTransportPacket(fixture.transport.get(), packet) == InputTransportStatus::kAccepted);
    (void)CompleteInputRoutingPacket(std::move(sending), InputRoutingDeliveryResult::kAccepted);
    assert(!QueryInputRoutingRetirement(ticket).admitted_send);
    fail_allocations = true;
    assert(fixture.barrier.Poll().status == Status::kTx);
    fail_allocations = false;
    send_budget = std::numeric_limits<size_t>::max();
    assert(FlushInputTransport(fixture.transport.get()) == InputTransportStatus::kAccepted);
    assert(fixture.barrier.Poll().status == Status::kReady);
    send_budget = 0;
  }
  {
    Fixture fixture(3);
    auto other = std::make_shared<InputTransport>(InputTransportIo{Send, Receive, Close, Error}, false);
    bool rejected = false;
    try {
      ReceiverRetirementBarrier invalid(fixture.lifecycle, fixture.admission.RetainRetirement(),
                                        fixture.binding.RetainRetirement(), other);
    } catch (const std::invalid_argument&) { rejected = true; }
    assert(rejected);  // Even before first publication.
    assert(fixture.binding.Register(reinterpret_cast<void*>(1), fixture.transport, 81, 81,
        {.on_event = [](void*, ReceiverEndpointBindingSlot, int, int) {
          return ReceiverEndpointBindingEventResult::kKeep;
        }, .context_owner = std::make_shared<int>(1)}, 3, 1));
    assert(fixture.lifecycle.Close() == ReceiverRoutingCloseStatus::kClosed);
    assert(fixture.admission.Retire());
    assert(fixture.admission.CompleteCleanup());
    fail_remove = true;
    (void)fixture.binding.Retire();
    assert(fixture.barrier.Poll().status == Status::kBinding);
    fail_remove = false;
    assert(fixture.binding.Retire());
    // Both callers contend on first capture, not just a finished fence query.
    auto retained = fixture.barrier.Retain();
    std::thread a([&] {
      for (int i = 0; i < 1000; ++i) {
        const auto result = retained.Poll().status;
        assert(result == Status::kReady || result == Status::kBusy);
      }
    });
    for (int i = 0; i < 1000; ++i) {
      const auto result = fixture.barrier.Poll().status;
      assert(result == Status::kReady || result == Status::kBusy);
    }
    a.join();
  }
  {
    auto fixture = std::make_unique<Fixture>(4);
    assert(fixture->lifecycle.Close() == ReceiverRoutingCloseStatus::kClosed);
    fixture->Quiesce();
    auto retained = fixture->barrier.Retain();
    std::weak_ptr<InputTransport> weak = fixture->transport;
    fixture.reset();
    assert(!weak.expired());
    // Independent record survives all containing wrappers. First capture,
    // not only a previously captured fence query, is allocation-independent.
    fail_allocations = true;
    const auto query = retained.Poll();
    assert(query.status == Status::kReady && query.fence_captured);
    fail_allocations = false;
  }
  {
    Fixture fixture(5);
    assert(fixture.lifecycle.Close() == ReceiverRoutingCloseStatus::kClosed);
    fixture.Quiesce();
    assert(SendInputTransportAck(fixture.transport.get(), 5, false) == InputTransportStatus::kAccepted);
    assert(fixture.barrier.Poll().status == Status::kTx);
    send_terminal = true;
    assert(FlushInputTransport(fixture.transport.get()) == InputTransportStatus::kTerminal);
    const auto terminal = fixture.barrier.Poll();
    assert(terminal.status == Status::kReady && terminal.tx == InputTransportTxFenceStatus::kTerminal);
    send_terminal = false;
  }
  assert(closes == 0);  // Proof-only barrier cannot close or take FD authority.
  {
    auto fixture = std::make_unique<Fixture>(6);
    auto record = PreparePendingReceiverRetirement(fixture->lifecycle,
        fixture->admission.RetainRetirement(), fixture->binding.RetainRetirement(), fixture->transport);
    std::weak_ptr<PendingReceiverRetirement> weak = record;
    fail_allocations = true;
    assert(EnlistPendingReceiverRetirement(record));
    assert(!EnlistPendingReceiverRetirement(record));
    assert(!ReleaseSettledReceiverRetirement(record));
    assert(ClosePendingReceiverRetirement(record) == ReceiverRoutingCloseStatus::kClosed);
    assert(!ReleaseSettledReceiverRetirement(record));
    fail_allocations = false;
    fixture->Quiesce();
    assert(SendInputTransportAck(fixture->transport.get(), 6, false) == InputTransportStatus::kAccepted);
    auto transport = fixture->transport;
    fixture.reset();
    record.reset();  // Registry, not wrapper/caller, is the only record owner.
    assert(!weak.expired());
    record = weak.lock();
    assert(IsPendingReceiverRetirementRetained(record));
    fail_allocations = true;
    assert(PollPendingReceiverRetirement(record).status == Status::kTx);
    assert(!ReleaseSettledReceiverRetirement(record));
    fail_allocations = false;
    send_budget = std::numeric_limits<size_t>::max();
    assert(FlushInputTransport(transport.get()) == InputTransportStatus::kAccepted);
    send_budget = 0;
    fail_allocations = true;
    assert(ReleaseSettledReceiverRetirement(record));
    assert(!IsPendingReceiverRetirementRetained(record));
    assert(!EnlistPendingReceiverRetirement(record));
    assert(!ReleaseSettledReceiverRetirement(record));
    fail_allocations = false;
    record.reset();
    assert(weak.expired());
  }
  {
    Fixture a(7), b(7), c(7);  // Same numeric IDs cannot identify list nodes.
    const auto prepare = [](Fixture& fixture) {
      return PreparePendingReceiverRetirement(fixture.lifecycle,
          fixture.admission.RetainRetirement(), fixture.binding.RetainRetirement(), fixture.transport);
    };
    bool failed = false;
    fail_allocations = true;
    try { (void)prepare(a); } catch (const std::bad_alloc&) { failed = true; }
    fail_allocations = false;
    assert(failed && a.barrier.Poll().status == Status::kLifecycle);
    auto ra = prepare(a), rb = prepare(b), rc = prepare(c);
    assert(EnlistPendingReceiverRetirement(ra));
    assert(EnlistPendingReceiverRetirement(rb));
    assert(EnlistPendingReceiverRetirement(rc));
    assert(ClosePendingReceiverRetirement(ra) == ReceiverRoutingCloseStatus::kClosed);
    assert(ClosePendingReceiverRetirement(rb) == ReceiverRoutingCloseStatus::kClosed);
    assert(ClosePendingReceiverRetirement(rc) == ReceiverRoutingCloseStatus::kClosed);
    a.Quiesce(); b.Quiesce(); c.Quiesce();
    fail_allocations = true;
    assert(ReleaseSettledReceiverRetirement(rb));  // Middle.
    assert(IsPendingReceiverRetirementRetained(ra) && IsPendingReceiverRetirementRetained(rc));
    assert(ReleaseSettledReceiverRetirement(ra));  // Tail, never same-ID head.
    assert(IsPendingReceiverRetirementRetained(rc));
    assert(!ReleaseSettledReceiverRetirement(ra));
    fail_allocations = false;
    std::atomic<int> winners{0};
    std::thread first([&] { if (ReleaseSettledReceiverRetirement(rc)) ++winners; });
    std::thread second([&] { if (ReleaseSettledReceiverRetirement(rc)) ++winners; });
    first.join(); second.join();
    assert(winners == 1 && !IsPendingReceiverRetirementRetained(rc));
  }
  {
    Fixture observer(8);
    close_observer = PreparePendingReceiverRetirement(observer.lifecycle,
        observer.admission.RetainRetirement(), observer.binding.RetainRetirement(), observer.transport);
    assert(EnlistPendingReceiverRetirement(close_observer));
    auto fixture = std::make_unique<Fixture>(9, true);
    auto record = PreparePendingReceiverRetirement(fixture->lifecycle,
        fixture->admission.RetainRetirement(), fixture->binding.RetainRetirement(), fixture->transport);
    assert(EnlistPendingReceiverRetirement(record));
    assert(ClosePendingReceiverRetirement(record) == ReceiverRoutingCloseStatus::kClosed);
    fixture->Quiesce();
    fixture.reset();
    assert(ReleaseSettledReceiverRetirement(record));
    record.reset();  // Last transport provider destructor reenters list query.
    assert(destructor_reentered && closes == 1);
    assert(ClosePendingReceiverRetirement(close_observer) == ReceiverRoutingCloseStatus::kClosed);
    observer.Quiesce();
    assert(ReleaseSettledReceiverRetirement(close_observer));
    close_observer.reset();
  }
  {
    auto fixture = std::make_unique<Fixture>(12);
    auto record = PreparePendingReceiverRetirement(fixture->lifecycle,
        fixture->admission.RetainRetirement(), fixture->binding.RetainRetirement(), fixture->transport);
    bool rejected = false;
    try { (void)PublishPendingReceiverRetirement(record); }
    catch (const std::logic_error&) { rejected = true; }
    assert(rejected);  // Visibility cannot precede independent retention.
    auto previous_endpoint = std::make_shared<const InputRoutingEndpoint>(
        InputRoutingEndpoint{fixture->transport, 11});
    assert(PublishInputRoutingRecipient(PrepareInputRoutingRecipient(
        fixture->routing, 11, previous_endpoint)).Published());
    assert(EnlistPendingReceiverRetirement(record));
    fixture->Quiesce();
    struct Reentry {
      PendingReceiverRetirementHandle* record;
      std::unique_ptr<Fixture>* fixture;
      bool entered = false;
    };
    auto context = std::make_shared<Reentry>(Reentry{&record, &fixture});
    auto subscription = SubscribeInputRoutingNotifications(fixture->routing,
        {.on_notification = [](void* opaque, const InputRoutingNotification&) {
          auto* context = static_cast<Reentry*>(opaque);
          if (context->entered) return;
          context->entered = true;
          assert(ClosePendingReceiverRetirement(*context->record) == ReceiverRoutingCloseStatus::kDeferred);
          context->record->reset();
          context->fixture->reset();
        }, .context = context.get(), .context_token = context});
    std::weak_ptr<PendingReceiverRetirement> weak = record;
    assert(PublishPendingReceiverRetirement(record).publication.Published());
    assert(record == nullptr && fixture == nullptr && context->entered);
    record = weak.lock();
    assert(record != nullptr && IsPendingReceiverRetirementRetained(record));
    assert(PollPendingReceiverRetirement(record).status == Status::kReady);
    assert(ReleaseSettledReceiverRetirement(record));
    record.reset();
    assert(weak.expired());
  }
  std::puts("receiver retirement barrier: admitted terminal/JNI/pump/TX-prefix/OOM/concurrency PASS");
  {
    Fixture a(13), b(14);
    auto ra = PreparePendingReceiverRetirement(a.lifecycle, a.admission.RetainRetirement(),
        a.binding.RetainRetirement(), a.transport);
    auto rb = PreparePendingReceiverRetirement(b.lifecycle, b.admission.RetainRetirement(),
        b.binding.RetainRetirement(), b.transport);
    assert(EnlistPendingReceiverRetirement(ra) && EnlistPendingReceiverRetirement(rb));
    assert(ClosePendingReceiverRetirement(ra) == ReceiverRoutingCloseStatus::kClosed);
    assert(ClosePendingReceiverRetirement(rb) == ReceiverRoutingCloseStatus::kClosed);
    a.Quiesce(); b.Quiesce();
    std::weak_ptr<PendingReceiverRetirement> wa = ra, wb = rb;
    ra.reset(); rb.reset();
    size_t calls = 0;
    const auto count = [](void* opaque, const PendingReceiverRetirementHandle& record) {
      assert(IsPendingReceiverRetirementRetained(record));
      ++*static_cast<size_t*>(opaque);
    };
    fail_allocations = true;
    auto bounded = VisitPendingReceiverRetirements(1, count, &calls);
    assert(bounded.visited == 1 && bounded.budget_exhausted && !bounded.membership_changed);
    auto resumed = VisitPendingReceiverRetirements(1, count, &calls, &bounded);
    assert(resumed.visited == 1 && !resumed.budget_exhausted && !resumed.membership_changed);
    auto zero = VisitPendingReceiverRetirements(0, count, &calls);
    assert(zero.visited == 0 && zero.budget_exhausted);
    const auto removed = VisitPendingReceiverRetirements(8,
        [](void* opaque, const PendingReceiverRetirementHandle& record) {
          ++*static_cast<size_t*>(opaque);
          assert(ReleaseSettledReceiverRetirement(record));
        }, &calls);
    assert(removed.visited == 2 && removed.membership_changed && !removed.budget_exhausted);
    const auto empty = VisitPendingReceiverRetirements(8, count, &calls);
    assert(empty.visited == 0 && !empty.membership_changed && !empty.budget_exhausted);
    fail_allocations = false;
    bounded.continuation.reset(); zero.continuation.reset();
    assert(calls == 4 && wa.expired() && wb.expired());
  }
  std::puts("pending retirement discovery: lost handles/bounded OOM scan/reentrant unlink PASS");
  {
    Fixture a(15), b(16), c(17);
    const auto prepare = [](Fixture& fixture) {
      auto record = PreparePendingReceiverRetirement(fixture.lifecycle,
          fixture.admission.RetainRetirement(), fixture.binding.RetainRetirement(), fixture.transport);
      assert(EnlistPendingReceiverRetirement(record));
      assert(ClosePendingReceiverRetirement(record) == ReceiverRoutingCloseStatus::kClosed);
      fixture.Quiesce();
      return record;
    };
    auto ra = prepare(a), rb = prepare(b), rc = prepare(c);
    fail_allocations = true;
    bool threw = false;
    try {
      (void)VisitPendingReceiverRetirements(1,
          [](void*, const PendingReceiverRetirementHandle&) { throw 77; }, nullptr);
    } catch (int value) { threw = value == 77; }
    assert(threw && IsPendingReceiverRetirementRetained(rc));
    auto changed = VisitPendingReceiverRetirements(1,
        [](void* opaque, const PendingReceiverRetirementHandle&) {
          assert(ReleaseSettledReceiverRetirement(*static_cast<PendingReceiverRetirementHandle*>(opaque)));
        }, &rb);
    assert(changed.membership_changed && changed.continuation == rb);
    auto restarted = VisitPendingReceiverRetirements(1,
        [](void* opaque, const PendingReceiverRetirementHandle& record) {
          assert(record == *static_cast<PendingReceiverRetirementHandle*>(opaque));
        }, &rc, &changed);
    assert(!restarted.membership_changed && restarted.continuation == ra);
    const auto tail = VisitPendingReceiverRetirements(1,
        [](void* opaque, const PendingReceiverRetirementHandle& record) {
          assert(record == *static_cast<PendingReceiverRetirementHandle*>(opaque));
        }, &ra, &restarted);
    assert(!tail.budget_exhausted && !tail.membership_changed);
    assert(ReleaseSettledReceiverRetirement(ra) && ReleaseSettledReceiverRetirement(rc));
    fail_allocations = false;
  }
  std::puts("retirement discovery mutation: removed next/revision restart/visitor exception PASS");
}
