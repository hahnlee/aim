#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_routing_domain.h"
#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/routing_transport_dispatch.h"

#include <atomic>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <new>

// Allocation injection is test-only. All routing objects below are production.
static std::atomic<bool> fail_allocations{false};
static std::atomic<int> failed_allocations{0};
static int allocations_before_failure = 0;
void* operator new(std::size_t size) {
  if (fail_allocations.load() && allocations_before_failure-- <= 0) {
    ++failed_allocations;
    throw std::bad_alloc();
  }
  if (auto* allocation = std::malloc(size == 0 ? 1 : size)) return allocation;
  throw std::bad_alloc();
}
void operator delete(void* allocation) noexcept { std::free(allocation); }

// Default provider is not used; only the explicit test I/O below is allowed.
extern "C" int darwin_art_bionic_socket_broker_socketpair(int, int, int, int*) { std::abort(); }
extern "C" intptr_t darwin_art_bionic_socket_broker_send(int, const void*, size_t, int) { std::abort(); }
extern "C" intptr_t darwin_art_bionic_socket_broker_recv(int, void*, size_t, int) { std::abort(); }
extern "C" int darwin_art_bionic_socket_broker_close(int) { std::abort(); }
extern "C" int darwin_art_bionic_errno_load() { std::abort(); }
using namespace darwin_art::input;

static int sent_fds[1024];
static int sends = 0;
static intptr_t Send(int fd, const void*, size_t count, int) {
  assert(sends < 1024);
  sent_fds[sends++] = fd;
  return static_cast<intptr_t>(count);
}
static intptr_t Receive(int, void*, size_t, int) { return -1; }
static int Close(int) { return 0; }
static int Error() { return 11; }

template <typename Action>
static bool AllocationFails(Action action, int allowed = 0) {
  allocations_before_failure = allowed;
  fail_allocations = true;
  bool threw = false;
  try { action(); } catch (const std::bad_alloc&) { threw = true; }
  fail_allocations = false;
  allocations_before_failure = 0;
  return threw;
}
static void SamePolicy(const InputRoutingHandle& owner,
                       const InputRoutingSelectionSnapshot& before) {
  const auto after = SnapshotInputRoutingSelection(owner);
  assert(after.generation == before.generation && after.recipient == before.recipient);
  assert(after.eligible == before.eligible && after.transport_ready == before.transport_ready);
  assert(after.focus_order == before.focus_order);
  assert(after.frame.left == before.frame.left && after.frame.top == before.frame.top &&
         after.frame.right == before.frame.right && after.frame.bottom == before.frame.bottom);
}

static DarwinArtPointerEventV2 Pointer(uint32_t action) {
  DarwinArtPointerEventV2 event{};
  event.version = 2;
  event.size = sizeof(event);
  event.action = action;
  event.pointer_count = 1;
  event.x = 20;
  event.y = 20;
  return event;
}

int main() {
  for (int epochs = 0; epochs <= 32; ++epochs) {
    auto routing = CreateInputRoutingState();
    auto recipient = PrepareInputRoutingRecipient(routing, 1000 + epochs);
    assert(PublishInputRoutingRecipient(recipient).Published());
    for (int i = 0; i < epochs; ++i)
      SetInputRoutingTransportReady(routing, recipient->id, (i % 2) == 0);
    darwin_art::DarwinArtInputPacket packet{};
    packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
    packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE);
    assert(EnqueueInputRoutingPacket(routing, packet, recipient->id));
    failed_allocations = 0;
    fail_allocations = true;
    bool threw = false;
    InputRoutingRecipientRetirementResult result;
    try {
      result = RetireInputRoutingRecipient(recipient);
    } catch (const std::bad_alloc&) {
      threw = true;
    }
    fail_allocations = false;
    assert(!threw && failed_allocations == 0);
    assert(result.status == InputRoutingRecipientRetirementStatus::kDetached);
    assert(result.changed && !InputRoutingHasPending(routing));
    assert(QueryInputRoutingRetirement(result.ticket).status ==
           InputRoutingRetirementStatus::kSettled);
  }
  {
    auto previous = CreateInputRoutingState();
    auto next = CreateInputRoutingState();
    auto identity = std::make_shared<int>(1);
    auto transport = std::shared_ptr<InputTransport>(identity,
        reinterpret_cast<InputTransport*>(identity.get()));
    auto endpoint = std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{transport, 1500});
    auto previous_token = PrepareInputRoutingRecipient(previous, 1500, endpoint);
    auto next_token = PrepareInputRoutingRecipient(next, 1501);
    assert(PublishInputRoutingRecipient(previous_token).Published());
    assert(PublishInputRoutingRecipient(next_token).Published());
    PublishInputRoutingWmsFrame(previous, 0, 0, 100, 100, true);
    PublishInputRoutingWmsFrame(next, 0, 0, 100, 100, true);
    assert(SetInputRoutingFocus(previous, 1500));
    const auto previous_policy = SnapshotInputRoutingSelection(previous);
    const auto next_policy = SnapshotInputRoutingSelection(next);
    auto replacement = PrepareInputRoutingRecipient(previous, 1502);
    assert(AllocationFails([&] { (void)PublishInputRoutingRecipient(replacement); }));
    assert(AllocationFails([&] { SetInputRoutingTransportReady(previous, 1500, true); }));
    assert(AllocationFails([&] { (void)ClearInputRoutingFocus(previous, 1500); }));
    assert(AllocationFails([&] { (void)PublishInputRoutingWmsFrame(previous, 0, 0, 100, 100, false); }));
    assert(AllocationFails([&] { TerminateInputRoutingTransport(previous, endpoint); }));
    SamePolicy(previous, previous_policy);
    for (int allowed = 0; allowed <= 1; ++allowed) {
      assert(AllocationFails([&] { (void)SetInputRoutingFocus(next, 1501); }, allowed));
      SamePolicy(previous, previous_policy);
      SamePolicy(next, next_policy);
      auto domain = LockInputRoutingDomain();
      assert(domain.Focused() == previous);
    }
    // This time history has capacity; fail the terminal-vector reservation.
    SetInputRoutingTransportReady(previous, 1500, true);
    const auto ready_policy = SnapshotInputRoutingSelection(previous);
    assert(AllocationFails([&] { TerminateInputRoutingTransport(previous, endpoint); }));
    SamePolicy(previous, ready_policy);
  }
  {
    auto owner = CreateInputRoutingState();
    auto old = PrepareInputRoutingRecipient(owner, 1600);
    auto next = PrepareInputRoutingRecipient(owner, 1600);
    assert(PublishInputRoutingRecipient(old).Published());
    darwin_art::DarwinArtInputPacket packet{};
    packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
    packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE);
    assert(EnqueueInputRoutingPacket(owner, packet, 1600));
    assert(PublishInputRoutingRecipient(next).Published());
    assert(EnqueueInputRoutingPacket(owner, packet, 1600));
    fail_allocations = true;
    auto historical = RetireInputRoutingRecipient(old);
    auto repeated = RetireInputRoutingRecipient(old);
    fail_allocations = false;
    assert(historical.changed && !repeated.changed);
    assert(InputRoutingConsumerMatches(owner, 1600) && InputRoutingHasPending(owner));
    fail_allocations = true;
    auto retired_next = RetireInputRoutingRecipient(next);
    fail_allocations = false;
    assert(retired_next.changed && !InputRoutingHasPending(owner));
  }
  // CANCEL may fail to allocate, but recipient revocation must complete and
  // preserve its obligation for the real FIFO retry after memory recovers.
  auto routing = CreateInputRoutingState();
  auto recipient = PrepareInputRoutingRecipient(routing, 2000);
  assert(PublishInputRoutingRecipient(recipient).Published());
  PublishInputRoutingWmsFrame(routing, 0, 0, 100, 100, true);
  SetInputRoutingTransportReady(routing, 2000, true);
  assert(SetInputRoutingFocus(routing, 2000));
  InputRoutingAdmission admission;
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN), &admission) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  failed_allocations = 0;
  fail_allocations = true;
  auto result = RetireInputRoutingRecipient(recipient);
  fail_allocations = false;
  assert(result.status == InputRoutingRecipientRetirementStatus::kDetached);
  assert(failed_allocations > 0 && !InputRoutingConsumerMatches(routing, 2000));
  assert(QueryInputRoutingRetirement(result.ticket).status ==
         InputRoutingRetirementStatus::kRunnable);
  fail_allocations = true;
  const auto stalled = DrainInputRoutingTransport(routing, 1);
  fail_allocations = false;
  assert(stalled.settled == 0 && !stalled.continuation_needed);
  (void)RetryInputRoutingCancellations(routing);
  InputRoutingCancellationLease cancel;
  assert(AcquireInputRoutingCancellation(routing, &cancel));
  assert(cancel.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(CompleteInputRoutingCancellation(std::move(cancel), true));
  assert(!AcquireInputRoutingCancellation(routing, &cancel));
  assert(QueryInputRoutingRetirement(result.ticket).status ==
         InputRoutingRetirementStatus::kSettled);
  // Remote CANCEL was left pending by OOM, then a different recipient filled
  // the FIFO. Its packet completions cannot retry the old stream's generation.
  // The real bounded dispatcher must retry after freeing capacity and request
  // continuation, not rely on an unrelated future input/writable event.
  {
    const InputTransportIo io{Send, Receive, Close, Error};
    auto old_transport = std::make_shared<InputTransport>(io, false);
    auto next_transport = std::make_shared<InputTransport>(io, false);
    AdoptRemoteInputTransport(old_transport.get(), 81);
    AdoptRemoteInputTransport(next_transport.get(), 82);
    auto owner = CreateInputRoutingState();
    auto old_endpoint = std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{old_transport, 2100});
    auto next_endpoint = std::make_shared<const InputRoutingEndpoint>(InputRoutingEndpoint{next_transport, 2101});
    auto old = PrepareInputRoutingRecipient(owner, 2100, old_endpoint);
    auto next = PrepareInputRoutingRecipient(owner, 2101, next_endpoint);
    assert(PublishInputRoutingRecipient(old).Published());
    PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true);
    assert(SetInputRoutingFocus(owner, 2100));
    // Populate history to a geometric-capacity point with enough publication
    // spare storage. Failure below must hit CANCEL, not publication preparation.
    SetInputRoutingTransportReady(owner, 2100, true);
    SetInputRoutingTransportReady(owner, 2100, false);
    assert(ClearInputRoutingFocus(owner, 2100));
    assert(SetInputRoutingFocus(owner, 2100));
    InputRoutingAdmission down;
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN), &down) ==
           darwin_art::DarwinArtInputEnqueueResult::kQueued);
    assert(SubmitInputRoutingAdmission(std::move(down)).refresh_writable);
    failed_allocations = 0;
    fail_allocations = true;
    auto publication = PublishInputRoutingRecipient(next);
    fail_allocations = false;
    assert(publication.Published() && failed_allocations > 0);
    assert(SetInputRoutingFocus(owner, 2101));
    const auto focus_result = ApplyInputRoutingFocusControl(
        next, FocusControl{1, true});
    assert(focus_result.Accepted() && focus_result.ShouldNotify());
    assert(CommitInputRoutingFocusNotification(focus_result));
    assert(CommitInputRoutingFocusReadiness(focus_result, true));
    DarwinArtKeyEventV1 key{};
    key.version = 1;
    key.size = sizeof(key);
    key.action = 0;
    key.key_code = 29;
    InputRoutingInflightLease reserved[256];
    for (auto& lease : reserved) {
      InputRoutingAdmission move;
      assert(RouteFrameworkKeyPacket(key, &move) ==
             darwin_art::DarwinArtInputEnqueueResult::kQueued);
      assert(ReserveInputRoutingPacket(std::move(move), false, &lease));
    }
    const auto before = sends;
    auto drained = DrainInputRoutingTransport(owner, 256);
    assert(drained.settled == 256 && drained.continuation_needed);
    assert(sends == before + 256);
    for (int i = before; i < sends; ++i) assert(sent_fds[i] == 82);
    drained = DrainInputRoutingTransport(owner, 1);
    assert(drained.settled == 1 && !drained.continuation_needed);
    assert(sends == before + 257 && sent_fds[sends - 1] == 81);
    assert(QueryInputRoutingRetirement(publication.predecessor).status ==
           InputRoutingRetirementStatus::kSettled);
  }
  // Fail FIFO growth after both the action and its opaque lease are allocated.
  // A rejected reservation must leave its output empty; otherwise an unpublished
  // record prevents a caller from retrying the empty-output API safely.
  bool fifo_growth_failure_seen = false;
  for (int count = 0; count <= 64; ++count) {
    const auto owner = CreateInputRoutingState();
    SetInputRoutingConsumer(owner, 2200);
    PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true);
    const auto proposal = [&] {
      InputRoutingAdmission value;
      assert(ValidateInputRoutingSelection(
          owner, SnapshotInputRoutingSelection(owner), &value));
      value.packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
      value.packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE);
      return value;
    };
    InputRoutingInflightLease pending[64];
    for (int index = 0; index < count; ++index)
      assert(ReserveInputRoutingPacket(proposal(), true, &pending[index]));
    InputRoutingInflightLease output;
    auto value = proposal();
    failed_allocations = 0;
    allocations_before_failure = 2;
    fail_allocations = true;
    const bool accepted = ReserveInputRoutingPacket(std::move(value), true, &output);
    fail_allocations = false;
    allocations_before_failure = 0;
    if (failed_allocations != 0) {
      fifo_growth_failure_seen = true;
      assert(!accepted && !output);
      assert(ReserveInputRoutingPacket(proposal(), true, &output));
    } else {
      assert(accepted && output);
    }
  }
  assert(fifo_growth_failure_seen);
  std::puts("routing retirement OOM: prepared epochs/revocation/CANCEL retry/reservation rollback PASS");
}
