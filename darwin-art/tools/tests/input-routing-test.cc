#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_routing_domain.h"
#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/routing_transport_dispatch.h"
#include "runtime/framework/input/input_transport.h"

#include <cassert>
#include <atomic>
#include <cstdio>
#include <thread>
#include <type_traits>
#include <utility>

namespace darwin_art::input {
template <typename T>
concept WritableTicketIdentity = requires(T& ticket) {
  ticket.id_ = ReceiverId{1};
};
template <typename T>
concept WritableTicketCutoff = requires(T& ticket) {
  ticket.cutoffgeneration_ = uint64_t{1};
};
static_assert(!std::is_aggregate_v<InputRoutingRetirementTicket>);
static_assert(!WritableTicketIdentity<InputRoutingRetirementTicket>);
static_assert(!WritableTicketCutoff<InputRoutingRetirementTicket>);
static_assert(std::is_copy_constructible_v<InputRoutingRetirementTicket>);
// This routing suite validates local dispatch only, not remote byte encoding.
int InputTransport::RemoteEndpointFd() const {
  assert(false && "local dispatch must not inspect a remote transport");
  return -1;
}
InputTransportStatus SendInputTransportPacket(InputTransport*,
                                             const darwin_art::DarwinArtInputPacket&) {
  assert(false && "remote dispatch requires the transport integration suite");
  return InputTransportStatus::kTerminal;
}
}

using darwin_art::DarwinArtInputPacket;
using darwin_art::DarwinArtInputPacketKind;
using darwin_art::DarwinArtInputEnqueueResult;
using darwin_art::input::CreateInputRoutingState;
using darwin_art::input::CommitInputRoutingPacket;
using darwin_art::input::DequeueInputRoutingPacket;
using darwin_art::input::EnqueueInputRoutingPacket;
using darwin_art::input::InputRoutingHandle;
using darwin_art::input::InputRoutingInflightLease;
using darwin_art::input::InputRoutingCancellationLease;
using darwin_art::input::InputRoutingPacketEnvelope;
using darwin_art::input::AcquireInputRoutingCancellation;
using darwin_art::input::AcquireInputRoutingRetry;
using darwin_art::input::AcquireInputRoutingHead;
using darwin_art::input::AdmitInputRoutingTransportSend;
using darwin_art::input::CompleteInputRoutingPacket;
using darwin_art::input::CompleteInputRoutingCancellation;
using darwin_art::input::InputRoutingDeliveryResult;
using darwin_art::input::ReserveInputRoutingPacket;
using darwin_art::input::DequeueInputRoutingPacketEnvelope;
using darwin_art::input::RequeueInputRoutingPacketEnvelopeFront;
using darwin_art::input::RouteFrameworkPointerPacket;
using darwin_art::input::SetInputRoutingConsumer;
using darwin_art::input::SetInputRoutingFocus;
using darwin_art::input::SetInputRoutingTransportReady;
using darwin_art::input::PublishInputRoutingWmsFrame;
using darwin_art::input::SubmitInputRoutingAdmission;

static DarwinArtPointerEventV2 Pointer(uint32_t action, float x, float y,
                                       uint64_t sequence) {
  DarwinArtPointerEventV2 event{};
  event.version = 2;
  event.size = sizeof(event);
  event.action = action;
  event.pointer_count = 1;
  event.x = x;
  event.y = y;
  event.raw_x = x;
  event.raw_y = y;
  event.sequence = sequence;
  return event;
}

static void DeliverFocusForKeys(const InputRoutingHandle& state,
                                uint64_t epoch) {
  const auto recipient = darwin_art::input::SnapshotInputRoutingSelection(state).recipient;
  const auto result = darwin_art::input::ApplyInputRoutingFocusControl(
      recipient, darwin_art::input::FocusControl{epoch, true});
  assert(result.Accepted() && result.ShouldNotify());
  assert(darwin_art::input::CommitInputRoutingFocusNotification(result));
  assert(darwin_art::input::CommitInputRoutingFocusReadiness(result, true));
}

static void DeliverLocalCancellation(const InputRoutingHandle& state) {
  InputRoutingInflightLease lease;
  assert(AcquireInputRoutingHead(state, &lease));
  assert(lease.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(darwin_art::input::BeginInputRoutingTransportSend(lease));
  assert(CompleteInputRoutingPacket(
      std::move(lease),
      InputRoutingDeliveryResult::kAccepted));
}

int main() {
  {
    // Only a real full -> available transition publishes capacity. The
    // observer reenters routing, proving publication is outside its mutex.
    using namespace darwin_art::input;
    const auto owner = CreateInputRoutingState();
    SetInputRoutingConsumer(owner, 919);
    struct CapacityObserver {
      InputRoutingHandle owner;
      unsigned notifications = 0;
    };
    auto observer = std::make_shared<CapacityObserver>();
    observer->owner = owner;
    const auto subscription = SubscribeInputRoutingNotifications(owner, {
        .on_notification = [](void* raw, const InputRoutingNotification& event) {
          if (event.kind != InputRoutingNotificationKind::kLocalCapacity) return;
          auto& observer = *static_cast<CapacityObserver*>(raw);
          ++observer.notifications;
          assert(event.id == 919);
          DarwinArtInputPacket refill;
          refill.kind = DarwinArtInputPacketKind::kKey;
          assert(EnqueueInputRoutingPacket(observer.owner, refill, 919));
        },
        .context = observer.get(),
        .context_token = observer,
    });
    assert(subscription);
    DarwinArtInputPacket key;
    key.kind = DarwinArtInputPacketKind::kKey;
    for (unsigned i = 0; i < 256; ++i)
      assert(EnqueueInputRoutingPacket(owner, key, 919));
    DarwinArtInputPacket packet;
    assert(!DequeueInputRoutingPacket(owner, &packet, 920));
    assert(observer->notifications == 0);
    assert(DequeueInputRoutingPacket(owner, &packet, 919));
    assert(observer->notifications == 1);
    // The reentrant observer refilled the queue; capacity is advisory only.
    assert(!EnqueueInputRoutingPacket(owner, key, 919));
    observer.reset();
    assert(DequeueInputRoutingPacket(owner, &packet, 919));
    assert(DequeueInputRoutingPacket(owner, &packet, 919));
  }
  {
    // Domain focus handoff while an old DOWN is already admitted. Late
    // acceptance belongs only to the old lane and cannot replace new capture.
    const auto old = CreateInputRoutingState();
    const auto next = CreateInputRoutingState();
    const auto old_endpoint = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
        darwin_art::input::InputRoutingEndpoint{nullptr, 9901});
    SetInputRoutingConsumer(old, 9901, old_endpoint);
    SetInputRoutingConsumer(next, 9902);
    PublishInputRoutingWmsFrame(old, 0, 0, 100, 100, true);
    PublishInputRoutingWmsFrame(next, 0, 0, 100, 100, true);
    assert(SetInputRoutingFocus(old, 9901));
    darwin_art::input::InputRoutingAdmission admission;
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 10, 10, 1), &admission) == DarwinArtInputEnqueueResult::kQueued);
    InputRoutingInflightLease old_send;
    assert(ReserveInputRoutingPacket(std::move(admission), false, &old_send));
    assert(AcquireInputRoutingHead(old, &old_send));
    assert(AdmitInputRoutingTransportSend(old_send));
    assert(SetInputRoutingFocus(next, 9902));
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 20, 20, 2), &admission) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == next && CommitInputRoutingPacket(std::move(admission), true, true));
    assert(!CompleteInputRoutingPacket(std::move(old_send), InputRoutingDeliveryResult::kAccepted));
    InputRoutingCancellationLease cancel;
    assert(AcquireInputRoutingCancellation(old, &cancel));
    assert(cancel.Endpoint() == old_endpoint && cancel.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
    assert(!CompleteInputRoutingCancellation(std::move(cancel), darwin_art::input::InputRoutingCancellationResult::kBackpressured));
    assert(AcquireInputRoutingCancellation(old, &cancel) && cancel.Endpoint() == old_endpoint);
    assert(CompleteInputRoutingCancellation(std::move(cancel), true));
    assert(!AcquireInputRoutingCancellation(old, &cancel));
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 500, 500, 3), &admission) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == next);
  }
  {
    const auto channel = CreateInputRoutingState();
    SetInputRoutingConsumer(channel, 9910);
    PublishInputRoutingWmsFrame(channel, 0, 0, 100, 100, true);
    const auto selection = darwin_art::input::SnapshotInputRoutingSelection(channel);
    darwin_art::input::InputRoutingAdmission admission;
    assert(darwin_art::input::ValidateInputRoutingSelection(channel, selection, &admission));
    SetInputRoutingConsumer(channel, 9911);
    assert(!darwin_art::input::ValidateInputRoutingSelection(channel, selection, &admission));
    // A new publication with the same receiver ID is still a different epoch.
    SetInputRoutingConsumer(channel, 9910);
    assert(!darwin_art::input::ValidateInputRoutingSelection(channel, selection, &admission));
  }
  {
    auto window = CreateInputRoutingState();
    assert(SetInputRoutingConsumer(window, 990) == 0);
    assert(darwin_art::input::UpdateInputRoutingReceiverGeometry(
        window, 0, 0, 100, 100, 990));
    assert(!SetInputRoutingFocus(window, 990));
    assert(!darwin_art::input::UpdateInputRoutingReceiverGeometry(
        window, 0, 0, 100, 100, 989));
    assert(!PublishInputRoutingWmsFrame(window, 0, 0, 100, 100, false));
    assert(!darwin_art::input::UpdateInputRoutingReceiverGeometry(
        window, 0, 0, 100, 100, 990));
    assert(!SetInputRoutingFocus(window, 990));
    assert(SetInputRoutingConsumer(window, 991) == 990);
    assert(!darwin_art::input::UpdateInputRoutingReceiverGeometry(
        window, 0, 0, 100, 100, 991));
    assert(!SetInputRoutingFocus(window, 991));
    assert(!PublishInputRoutingWmsFrame(window, 0, 0, 100, 100, true));
    SetInputRoutingTransportReady(window, 991, true);
    assert(SetInputRoutingFocus(window, 991));
    darwin_art::input::InputRoutingAdmission active;
    assert(RouteFrameworkPointerPacket(
        Pointer(DARWIN_ART_POINTER_DOWN, 20, 20, 9901), &active) ==
        DarwinArtInputEnqueueResult::kQueued);
    assert(active.state == window);
    assert(CommitInputRoutingPacket(std::move(active), true, true));
    DarwinArtInputPacket down;
    assert(DequeueInputRoutingPacket(window, &down));
    assert(down.pointer.action == DARWIN_ART_POINTER_DOWN);
    assert(PublishInputRoutingWmsFrame(window, 0, 0, 0, 100, true));
    assert(!SetInputRoutingFocus(window, 991));
    DeliverLocalCancellation(window);
    InputRoutingInflightLease after_cancel;
    assert(!AcquireInputRoutingHead(window, &after_cancel));
  }
  const InputRoutingHandle left = CreateInputRoutingState();
  const InputRoutingHandle right = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(left, 11) == 0);
  assert(SetInputRoutingConsumer(right, 22) == 0);
  SetInputRoutingTransportReady(left, 11, true);
  SetInputRoutingTransportReady(right, 22, true);
  assert(PublishInputRoutingWmsFrame(left, 0, 0, 100, 100, true) == false);
  assert(PublishInputRoutingWmsFrame(right, 100, 0, 200, 100, true) == false);
  assert(SetInputRoutingFocus(left, 11));

  darwin_art::input::InputRoutingAdmission admission;
  DarwinArtInputPacket delivered;
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 25, 30, 1),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  assert(admission.state == left);
  assert(admission.packet.pointer.x == 25 && admission.packet.pointer.y == 30);
  assert(CommitInputRoutingPacket(std::move(admission), true, true));
  assert(darwin_art::input::InputRoutingHasPending(left));
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_DOWN);
  assert(!darwin_art::input::InputRoutingHasPending(left));
  // A pre-handoff retry restores the wake obligation; consuming it clears it.
  assert(darwin_art::input::RequeueInputRoutingPacketFront(left, delivered));
  assert(darwin_art::input::InputRoutingHasPending(left));
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(!darwin_art::input::InputRoutingHasPending(left));

  // Fresh local submissions and writable retries share the dispatch owner;
  // the submit API reserves the exact routed admission before queueing it.
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 30, 30, 101),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  const auto submitted = SubmitInputRoutingAdmission(std::move(admission));
  assert(submitted.result == DarwinArtInputEnqueueResult::kQueued &&
         submitted.wake_local && !submitted.refresh_writable);
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.sequence == 101);

  // A stream remains captured by its down target even when the next point is
  // over another published window; routing applies Android-local geometry.
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 150, 30, 2),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  assert(admission.state == left);
  assert(admission.packet.pointer.x == 150);
  assert(CommitInputRoutingPacket(std::move(admission), true, true));
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_MOVE);

  // Replacing a receiver invalidates the old expected focus publication.
  assert(SetInputRoutingConsumer(left, 33) == 11);
  DeliverLocalCancellation(left);
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(!SetInputRoutingFocus(left, 11));
  SetInputRoutingTransportReady(left, 33, true);
  assert(SetInputRoutingFocus(left, 33));

  // Focus handoff cancels the active stream and drops stale queued payload.
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 25, 30, 3),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(admission), true, true));
  assert(SetInputRoutingFocus(right, 22));
  DeliverLocalCancellation(left);
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_DOWN);
  assert(DequeueInputRoutingPacket(left, &delivered));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(delivered.pointer.sequence == 4);
  assert(!darwin_art::input::InputRoutingHasPending(left));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 150, 30, 5),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kNoFocusedChannel);

  // Replacement between route and commit rejects the old generation; neither
  // DOWN nor a later UP may mutate capture or claim delivery.
  const InputRoutingHandle race = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(race, 44) == 0);
  SetInputRoutingTransportReady(race, 44, true);
  assert(PublishInputRoutingWmsFrame(race, 300, 0, 400, 100, true) == false);
  assert(SetInputRoutingFocus(race, 44));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 350, 30, 8),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease race_lease;
  assert(ReserveInputRoutingPacket(std::move(admission), true, &race_lease));
  assert(AcquireInputRoutingHead(race, &race_lease));
  assert(SetInputRoutingFocus(right, 22));
  assert(!CompleteInputRoutingPacket(
      std::move(race_lease), InputRoutingDeliveryResult::kAccepted));
  // The local DOWN was never enqueued.  Once its recipient epoch is retired,
  // stale completion must not synthesize a cancellation for that receiver.
  assert(!InputRoutingHasPendingCancellation(race));
  // A remote/local admission that cannot be completed yet remains retryable,
  // but its original generation is still checked at the retry linearization.
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 350, 30, 9),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease retry_lease;
  assert(ReserveInputRoutingPacket(std::move(admission), true, &retry_lease));
  assert(AcquireInputRoutingHead(race, &retry_lease));
  assert(!CompleteInputRoutingPacket(
      std::move(retry_lease), InputRoutingDeliveryResult::kBackpressured));
  assert(AcquireInputRoutingRetry(race, &retry_lease));
  assert(CompleteInputRoutingPacket(
      std::move(retry_lease), InputRoutingDeliveryResult::kAccepted));
  assert(SetInputRoutingFocus(race, 44));
  assert(SetInputRoutingConsumer(race, 55) == 44);
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_UP, 350, 30, 9),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kNoFocusedChannel);

  // A remote-accepted DOWN commits capture without entering the local queue;
  // hiding that window retains exactly one CANCEL for the original transport.
  // The policy retains real endpoint objects. Only the incomplete transport
  // identity/lifetime is substituted; routing never performs transport I/O.
  auto endpoint_owner = std::make_shared<int>(42);
  std::shared_ptr<darwin_art::input::InputTransport> transport_identity(
      endpoint_owner,
      reinterpret_cast<darwin_art::input::InputTransport*>(
          endpoint_owner.get()));
  auto endpoint = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{transport_identity, 22});
  SetInputRoutingConsumer(right, 22, endpoint);
  assert(SetInputRoutingFocus(right, 22));
  SetInputRoutingTransportReady(right, 22, false);
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 150, 30, 6),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  assert(admission.state == right);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  // Readiness transition retires the accepted remote stream and retains one
  // old-endpoint cancellation before the window is hidden.
  SetInputRoutingTransportReady(right, 22, true);
  assert(PublishInputRoutingWmsFrame(right, 0, 0, 0, 0, false));
  InputRoutingCancellationLease cancellation_a;
  InputRoutingCancellationLease cancellation_b;
  assert(AcquireInputRoutingCancellation(right, &cancellation_a));
  assert(cancellation_a.Endpoint() == endpoint);
  assert(cancellation_a.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(!AcquireInputRoutingCancellation(right, &cancellation_b));
  // Replacing a live lease must release its exclusive claim before the move.
  cancellation_a = InputRoutingCancellationLease{};
  assert(AcquireInputRoutingCancellation(right, &cancellation_b));
  assert(!CompleteInputRoutingCancellation(std::move(cancellation_b), false));
  assert(AcquireInputRoutingCancellation(right, &cancellation_a));
  assert(CompleteInputRoutingCancellation(std::move(cancellation_a), true));
  // Endpoint terminal notification remains valid after the consumer has been
  // detached: the retained opaque endpoint token identifies the old stream,
  // while no replacement work is touched.
  const auto retained_endpoint = GetInputRoutingEndpoint(right);
  assert(retained_endpoint == endpoint);
  assert(SetInputRoutingConsumer(right, 0, endpoint) == 22);
  TerminateInputRoutingTransport(right, retained_endpoint);
  assert(!InputRoutingHasPendingCancellation(right));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 150, 30, 7),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kNoFocusedChannel);

  // Registration and successive recipients have different endpoint wrappers,
  // but terminal loss belongs to their shared retained transport lane.
  const auto same_lane = CreateInputRoutingState();
  auto first_recipient = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{transport_identity, 201});
  auto next_recipient = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{transport_identity, 202});
  auto registration = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{transport_identity, 0});
  SetInputRoutingConsumer(same_lane, 201, first_recipient);
  assert(!PublishInputRoutingWmsFrame(same_lane, 700, 0, 800, 100, true));
  assert(SetInputRoutingFocus(same_lane, 201));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 750, 30, 20),
                                     &admission) == DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  SetInputRoutingConsumer(same_lane, 202, next_recipient);
  assert(InputRoutingHasPendingCancellation(same_lane));
  assert(SetInputRoutingFocus(same_lane, 202));
  TerminateInputRoutingTransport(same_lane, registration);
  assert(!InputRoutingHasPendingCancellation(same_lane));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 750, 30, 21),
                                     &admission) == DarwinArtInputEnqueueResult::kNoFocusedChannel);

  auto other_owner = std::make_shared<int>(43);
  std::shared_ptr<darwin_art::input::InputTransport> other_transport(
      other_owner, reinterpret_cast<darwin_art::input::InputTransport*>(other_owner.get()));
  auto other_recipient = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{other_transport, 302});
  const auto separate_lane = CreateInputRoutingState();
  SetInputRoutingConsumer(separate_lane, 201, first_recipient);
  assert(!PublishInputRoutingWmsFrame(separate_lane, 700, 0, 800, 100, true));
  assert(SetInputRoutingFocus(separate_lane, 201));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 750, 30, 22),
                                     &admission) == DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  SetInputRoutingConsumer(separate_lane, 302, other_recipient);
  assert(SetInputRoutingFocus(separate_lane, 302));
  TerminateInputRoutingTransport(separate_lane, registration);
  assert(!InputRoutingHasPendingCancellation(separate_lane));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 750, 30, 23),
                                     &admission) == DarwinArtInputEnqueueResult::kQueued);
  assert(admission.endpoint == other_recipient);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  ClearInputRoutingFocus(separate_lane, 302);

  // An admitted old MOVE is allowed to finish before its deferred CANCEL;
  // a live lease cannot be stolen by the retry pump.
  const InputRoutingHandle ordered = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(ordered, 66) == 0);
  SetInputRoutingTransportReady(ordered, 66, false);
  assert(PublishInputRoutingWmsFrame(ordered, 500, 0, 600, 100, true) == false);
  assert(SetInputRoutingFocus(ordered, 66));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 550, 30, 10),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease ordered_down;
  assert(ReserveInputRoutingPacket(std::move(admission), false, &ordered_down));
  assert(AcquireInputRoutingHead(ordered, &ordered_down));
  assert(!AcquireInputRoutingRetry(ordered, &retry_lease));
  assert(AdmitInputRoutingTransportSend(ordered_down));
  assert(CompleteInputRoutingPacket(
      std::move(ordered_down), InputRoutingDeliveryResult::kAccepted));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 560, 30, 11),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease ordered_move;
  assert(ReserveInputRoutingPacket(std::move(admission), false, &ordered_move));
  assert(AcquireInputRoutingHead(ordered, &ordered_move));
  assert(AdmitInputRoutingTransportSend(ordered_move));
  assert(PublishInputRoutingWmsFrame(ordered, 0, 0, 0, 0, false));
  InputRoutingCancellationLease ordered_cancel;
  assert(!AcquireInputRoutingCancellation(ordered, &ordered_cancel));
  assert(!CompleteInputRoutingPacket(
      std::move(ordered_move), InputRoutingDeliveryResult::kAccepted));
  assert(AcquireInputRoutingCancellation(ordered, &ordered_cancel));
  assert(ordered_cancel.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(CompleteInputRoutingCancellation(
      std::move(ordered_cancel),
      darwin_art::input::InputRoutingCancellationResult::kAccepted));

  // Bounded policy rejects non-coalescible packets instead of claiming a
  // fabricated delivery success.
  DarwinArtInputPacket key;
  key.kind = DarwinArtInputPacketKind::kKey;
  for (size_t i = 0; i < 256; ++i) assert(EnqueueInputRoutingPacket(right, key));
  assert(!EnqueueInputRoutingPacket(right, key));

  const InputRoutingHandle saturated = CreateInputRoutingState();
  SetInputRoutingConsumer(saturated, 91);
  assert(!PublishInputRoutingWmsFrame(saturated, 300, 300, 400, 400, true));
  assert(SetInputRoutingFocus(saturated, 91));
  SetInputRoutingTransportReady(saturated, 91, true);
  darwin_art::input::InputRoutingAdmission local_down;
  assert(RouteFrameworkPointerPacket(
      Pointer(DARWIN_ART_POINTER_DOWN, 350, 350, 100), &local_down) ==
      DarwinArtInputEnqueueResult::kQueued);
  assert(local_down.state == saturated);
  assert(CommitInputRoutingPacket(std::move(local_down), true, true));
  DarwinArtInputPacket local_packet;
  assert(DequeueInputRoutingPacket(saturated, &local_packet, 91));
  for (size_t i = 0; i < 256; ++i)
    assert(EnqueueInputRoutingPacket(saturated, key, 91));
  assert(darwin_art::input::ClearInputRoutingFocus(saturated, 91));
  {
    InputRoutingCancellationLease retained_cancel;
    assert(AcquireInputRoutingCancellation(saturated, &retained_cancel));
    assert(!AcquireInputRoutingCancellation(saturated, &retained_cancel));
    assert(retained_cancel.ConsumerId() == 91 &&
           retained_cancel.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  }
  // Exercise the real dispatcher's Begin -> accepted local completion with a
  // full queue. Backpressure must release send admission, not terminal-drop
  // the same durable cancellation on its next retry.
  for (int retry = 0; retry < 2; ++retry) {
    const auto blocked = darwin_art::input::DrainInputRoutingTransport(saturated, 1);
    assert(blocked.backpressured && !blocked.terminal && !blocked.local_queued);
    assert(darwin_art::input::InputRoutingHasPendingCancellation(saturated));
  }
  assert(DequeueInputRoutingPacket(saturated, &local_packet, 91));
  assert(darwin_art::input::RetryInputRoutingCancellations(saturated));
  const auto delivered_cancel = darwin_art::input::DrainInputRoutingTransport(saturated, 1);
  assert(delivered_cancel.local_queued && !delivered_cancel.backpressured &&
         !delivered_cancel.terminal);
  int cancels = 0;
  while (DequeueInputRoutingPacket(saturated, &local_packet, 91))
    if (local_packet.kind == DarwinArtInputPacketKind::kPointer &&
        local_packet.pointer.action == DARWIN_ART_POINTER_CANCEL) ++cancels;
  assert(cancels == 1);

  // A callback for the old receiver cannot pop the replacement's envelope;
  // both receiver and generation survive dequeue/requeue unchanged.
  const InputRoutingHandle envelopes = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(envelopes, 77) == 0);
  assert(EnqueueInputRoutingPacket(envelopes, key, 77));
  InputRoutingPacketEnvelope old_envelope;
  assert(DequeueInputRoutingPacketEnvelope(envelopes, &old_envelope, 77));
  assert(SetInputRoutingConsumer(envelopes, 88) == 77);
  assert(EnqueueInputRoutingPacket(envelopes, key, 88));
  InputRoutingPacketEnvelope new_envelope;
  assert(DequeueInputRoutingPacketEnvelope(envelopes, &new_envelope, 88));
  assert(RequeueInputRoutingPacketEnvelopeFront(envelopes, new_envelope));
  assert(!RequeueInputRoutingPacketEnvelopeFront(envelopes, old_envelope));
  InputRoutingPacketEnvelope observed;
  assert(!DequeueInputRoutingPacketEnvelope(envelopes, &observed, 77));
  assert(DequeueInputRoutingPacketEnvelope(envelopes, &observed, 88));
  assert(observed.consumer_id == 88 &&
         observed.generation == new_envelope.generation);
  // Bounded historical diagnostics must never reopen a closed epoch.
  for (darwin_art::input::ReceiverId id = 1000; id < 1514; ++id)
    SetInputRoutingConsumer(envelopes, id);
  assert(!RequeueInputRoutingPacketEnvelopeFront(envelopes, old_envelope));
  assert(!RequeueInputRoutingPacketEnvelopeFront(envelopes, new_envelope));

  // Reservation and writable retry share one FIFO head. A fresh action cannot
  // overtake a paused head, and repeated backpressure keeps its exact packet.
  const InputRoutingHandle fifo = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(fifo, 501) == 0);
  SetInputRoutingTransportReady(fifo, 501, false);
  assert(!PublishInputRoutingWmsFrame(fifo, 0, 0, 100, 100, true));
  assert(SetInputRoutingFocus(fifo, 501));
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 10, 10, 41),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease head;
  assert(ReserveInputRoutingPacket(std::move(admission), false, &head));
  assert(AcquireInputRoutingHead(fifo, &head));
  assert(head.Packet()->pointer.sequence == 41);
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 11, 10, 42),
                                     &admission) ==
         DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease overtaking;
  assert(ReserveInputRoutingPacket(std::move(admission), false, &overtaking));
  assert(static_cast<bool>(overtaking));
  assert(!CompleteInputRoutingPacket(
      std::move(head), InputRoutingDeliveryResult::kBackpressured));
  assert(AcquireInputRoutingHead(fifo, &head));
  assert(head.Packet()->pointer.sequence == 41);
  assert(!CompleteInputRoutingPacket(
      std::move(head), InputRoutingDeliveryResult::kBackpressured));
  assert(AcquireInputRoutingRetry(fifo, &head));
  assert(head.Packet()->pointer.sequence == 41);
  assert(AdmitInputRoutingTransportSend(head));
  assert(CompleteInputRoutingPacket(
      std::move(head), InputRoutingDeliveryResult::kAccepted));
  assert(AcquireInputRoutingHead(fifo, &head));
  assert(head.Packet()->pointer.sequence == 42);
  head = InputRoutingInflightLease{};  // move assignment unclaims the head.
  assert(AcquireInputRoutingHead(fifo, &overtaking));
  assert(overtaking.Packet()->pointer.sequence == 42);
  assert(CompleteInputRoutingPacket(
      std::move(overtaking), InputRoutingDeliveryResult::kBackpressured) ==
         false);

  // A retained reservation is not an interchangeable claim token. Reject a
  // foreign owner without claiming its head or changing the original lease.
  {
    using namespace darwin_art::input;
    auto first = CreateInputRoutingState();
    auto second = CreateInputRoutingState();
    SetInputRoutingConsumer(first, 906);
    SetInputRoutingConsumer(second, 907);
    PublishInputRoutingWmsFrame(first, 0, 0, 100, 100, true);
    PublishInputRoutingWmsFrame(second, 100, 0, 200, 100, true);
    const auto reserve = [](const InputRoutingHandle& owner,
                            uint64_t sequence, InputRoutingInflightLease* lease) {
      InputRoutingAdmission proposal;
      assert(ValidateInputRoutingSelection(
          owner, SnapshotInputRoutingSelection(owner), &proposal));
      proposal.packet.kind = DarwinArtInputPacketKind::kPointer;
      proposal.packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE, 20, 20, sequence);
      assert(ReserveInputRoutingPacket(std::move(proposal), true, lease));
    };
    InputRoutingInflightLease first_lease, second_lease;
    reserve(first, 1001, &first_lease);
    reserve(second, 1002, &second_lease);
    InputRoutingAdmission replacement;
    assert(ValidateInputRoutingSelection(
        second, SnapshotInputRoutingSelection(second), &replacement));
    replacement.packet.kind = DarwinArtInputPacketKind::kPointer;
    replacement.packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE, 20, 20, 1004);
    assert(!ReserveInputRoutingPacket(std::move(replacement), true, &first_lease));
    assert(!AcquireInputRoutingHead(second, &first_lease));
    assert(first_lease.ConsumerId() == 906 &&
           first_lease.Packet()->pointer.sequence == 1001);
    assert(AcquireInputRoutingHead(second, &second_lease));
    assert(BeginInputRoutingTransportSend(second_lease));
    assert(CompleteInputRoutingPacketWithStatus(std::move(second_lease),
        InputRoutingDeliveryResult::kTerminal) ==
        InputRoutingPacketCompletionStatus::kTerminal);
    assert(!InputRoutingHasRunnableAction(second));
    assert(AcquireInputRoutingHead(first, &first_lease));
    assert(BeginInputRoutingTransportSend(first_lease));
    assert(CompleteInputRoutingPacketWithStatus(std::move(first_lease),
        InputRoutingDeliveryResult::kTerminal) ==
        InputRoutingPacketCompletionStatus::kTerminal);
  }

  // Completion notification may destroy the externally held lease passed by
  // rvalue reference. Settle it before callbacks, and never touch it afterward.
  {
    using namespace darwin_art::input;
    const auto owner = CreateInputRoutingState();
    SetInputRoutingConsumer(owner, 908);
    PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true);
    InputRoutingAdmission proposal;
    assert(ValidateInputRoutingSelection(
        owner, SnapshotInputRoutingSelection(owner), &proposal));
    proposal.packet.kind = DarwinArtInputPacketKind::kPointer;
    proposal.packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE, 20, 20, 1003);
    auto held = std::make_unique<InputRoutingInflightLease>();
    assert(ReserveInputRoutingPacket(std::move(proposal), true, held.get()));
    assert(AcquireInputRoutingHead(owner, held.get()));
    assert(BeginInputRoutingTransportSend(*held));
    struct CompletionObserver {
      InputRoutingHandle owner;
      std::unique_ptr<InputRoutingInflightLease>* held;
      bool invoked = false;
    };
    auto observer = std::make_shared<CompletionObserver>(
        CompletionObserver{owner, &held, false});
    const auto subscription = SubscribeInputRoutingNotifications(owner,
        {.on_notification = [](void* raw, const InputRoutingNotification& event) {
           if (event.kind != InputRoutingNotificationKind::kPacketCompletion) return;
           auto* context = static_cast<CompletionObserver*>(raw);
           assert(context->held->get() != nullptr && !**context->held);
           context->held->reset();
           DarwinArtInputPacket packet;
           assert(DequeueInputRoutingPacket(context->owner, &packet, 908));
           assert(packet.pointer.sequence == 1003);
           auto domain = LockInputRoutingDomain();
           (void)domain.Focused();
           context->invoked = true;
         }, .context = observer.get(), .context_token = observer});
    assert(subscription);
    assert(CompleteInputRoutingPacketWithStatus(std::move(*held),
        InputRoutingDeliveryResult::kAccepted) ==
        InputRoutingPacketCompletionStatus::kAccepted);
    assert(observer->invoked && !held);
  }

  auto batch = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(batch, 900) == 0);
  PublishInputRoutingWmsFrame(batch, 0, 0, 100, 100, true);
  SetInputRoutingTransportReady(batch, 900, true);
  assert(SetInputRoutingFocus(batch, 900));
  for (uint64_t sequence = 1; sequence <= 10; ++sequence) {
    assert(RouteFrameworkPointerPacket(
        Pointer(DARWIN_ART_POINTER_DOWN, 20, 20, sequence),
        &admission) == DarwinArtInputEnqueueResult::kQueued);
    InputRoutingInflightLease reservation;
    assert(ReserveInputRoutingPacket(std::move(admission), true, &reservation));
  }
  auto drained = darwin_art::input::DrainInputRoutingTransport(batch, 3);
  assert(drained.settled == 3 && drained.local_queued && !drained.backpressured);
  assert(drained.continuation_needed);
  drained = darwin_art::input::DrainInputRoutingTransport(batch);
  assert(drained.settled == 7 && drained.local_queued && !drained.remote_admitted);
  assert(!drained.continuation_needed);
  delivered = {};
  for (uint64_t sequence = 1; sequence <= 10; ++sequence) {
    assert(DequeueInputRoutingPacket(batch, &delivered, 900));
    assert(delivered.pointer.sequence == sequence);
  }
  assert(darwin_art::input::ClearInputRoutingFocus(batch, 900));
  drained = darwin_art::input::DrainInputRoutingTransport(batch);
  assert(drained.settled == 1 && drained.local_queued);
  assert(DequeueInputRoutingPacket(batch, &delivered, 900));
  assert(delivered.pointer.action == DARWIN_ART_POINTER_CANCEL);
  // Detach must retain the publication epoch before clearing its identity.
  // No focus/geometry mutation or active stream has recorded this epoch yet.
  auto first_epoch = CreateInputRoutingState();
  assert(SetInputRoutingConsumer(first_epoch, 902) == 0);
  DarwinArtInputPacket first_epoch_packet{};
  first_epoch_packet.kind = DarwinArtInputPacketKind::kPointer;
  first_epoch_packet.pointer = Pointer(DARWIN_ART_POINTER_MOVE, 20, 20, 1);
  assert(EnqueueInputRoutingPacket(first_epoch, first_epoch_packet, 902));
  assert(darwin_art::input::InputRoutingHasPending(first_epoch));
  assert(SetInputRoutingConsumer(first_epoch, 0, {}, 902) == 902);
  assert(!darwin_art::input::InputRoutingHasPending(first_epoch));
  assert(!DequeueInputRoutingPacket(first_epoch, &delivered));

  // Typed publication separates first success from a rejected transaction.
  // Exact-token retirement captures the latest readiness/focus epoch, not a
  // ticket sampled before those changes, and never detaches a same-ID successor.
  {
    using namespace darwin_art::input;
    auto owner = CreateInputRoutingState();
    auto current = PrepareInputRoutingRecipient(owner, 903);
    auto next = PrepareInputRoutingRecipient(owner, 903);
    assert(PublishInputRoutingRecipient({}).status ==
           InputRoutingPublicationStatus::kInvalidRecipient);
    auto published = PublishInputRoutingRecipient(current);
    assert(published.Published() && published.predecessor.ConsumerId() == 0);
    auto rejected = PublishInputRoutingRecipient(next, 999);
    assert(rejected.status == InputRoutingPublicationStatus::kCurrentMismatch);
    assert(RetireInputRoutingRecipient(next).status ==
           InputRoutingRecipientRetirementStatus::kNotPublished);
    assert(!PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true));
    const auto initial_generation = SnapshotInputRoutingSelection(owner).generation;
    SetInputRoutingTransportReady(owner, 903, true);
    assert(SetInputRoutingFocus(owner, 903));
    const auto latest_generation = SnapshotInputRoutingSelection(owner).generation;
    assert(latest_generation > initial_generation);
    assert(EnqueueInputRoutingPacket(owner, first_epoch_packet, 903));
    auto retired_current = RetireInputRoutingRecipient(current);
    assert(retired_current.status == InputRoutingRecipientRetirementStatus::kDetached);
    assert(retired_current.changed && retired_current.ticket.Recipient() == current);
    assert(retired_current.ticket.CutoffGeneration() == latest_generation);
    assert(!InputRoutingConsumerMatches(owner, 903) && !InputRoutingHasPending(owner));
    assert(PublishInputRoutingRecipient(next).Published());
    assert(EnqueueInputRoutingPacket(owner, first_epoch_packet, 903));
    auto retired_history = RetireInputRoutingRecipient(current);
    assert(retired_history.status == InputRoutingRecipientRetirementStatus::kHistorical);
    assert(retired_history.ticket.Recipient() == current);
    assert(retired_history.ticket.CutoffGeneration() == latest_generation);
    assert(InputRoutingConsumerMatches(owner, 903) && InputRoutingHasPending(owner));
    assert(DequeueInputRoutingPacket(owner, &delivered, 903));
    assert(RetireInputRoutingRecipient(InputRoutingRecipientHandle{}).status ==
           InputRoutingRecipientRetirementStatus::kInvalidRecipient);

    // Retirement notifications may synchronously reenter the ledger/domain.
    struct RetirementObserver { InputRoutingHandle owner; bool notified = false; };
    auto observer = std::make_shared<RetirementObserver>();
    observer->owner = owner;
    auto subscription = SubscribeInputRoutingNotifications(owner,
        {.on_notification = [](void* raw, const InputRoutingNotification&) {
           auto* observer = static_cast<RetirementObserver*>(raw);
           assert(!InputRoutingConsumerMatches(observer->owner, 903));
           auto domain = LockInputRoutingDomain();
           (void)domain.Focused();
           observer->notified = true;
         }, .context = observer.get(), .context_token = observer});
    assert(subscription != nullptr);
    auto retired_next = RetireInputRoutingRecipient(next);
    assert(retired_next.status == InputRoutingRecipientRetirementStatus::kDetached);
    assert(observer->notified && retired_next.ticket.Recipient() == next);
  }

  // Newer same-ID/same-endpoint tickets exclude older admitted obligations.
  // Test both an in-flight send and an already accepted stream with CANCEL.
  for (int complete_before_publication = 0; complete_before_publication < 2;
       ++complete_before_publication) {
    using namespace darwin_art::input;
    auto owner = CreateInputRoutingState();
    auto shared_endpoint = std::make_shared<const InputRoutingEndpoint>(
        InputRoutingEndpoint{{}, 904});
    auto old = PrepareInputRoutingRecipient(owner, 904, shared_endpoint);
    auto next = PrepareInputRoutingRecipient(owner, 904, shared_endpoint);
    assert(PublishInputRoutingRecipient(old).Published());
    PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true);
    SetInputRoutingTransportReady(owner, 904, true);
    assert(SetInputRoutingFocus(owner, 904));
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 20, 20, 1),
        &admission) == DarwinArtInputEnqueueResult::kQueued);
    InputRoutingInflightLease sending;
    assert(ReserveInputRoutingPacket(std::move(admission), false, &sending));
    assert(AcquireInputRoutingHead(owner, &sending));
    assert(AdmitInputRoutingTransportSend(sending));
    if (complete_before_publication)
      assert(CompleteInputRoutingPacket(std::move(sending), InputRoutingDeliveryResult::kAccepted));
    auto published_next = PublishInputRoutingRecipient(next);
    assert(published_next.Published() && published_next.predecessor.Recipient() == old);
    // Historical stream cleanup must not associate the old epoch with next.
    auto retired_old = RetireInputRoutingRecipient(old);
    assert(retired_old.status == InputRoutingRecipientRetirementStatus::kHistorical);
    assert(InputRoutingConsumerMatches(owner, 904));
    auto retired_next = RetireInputRoutingRecipient(next);
    assert(QueryInputRoutingRetirement(retired_next.ticket).status ==
           InputRoutingRetirementStatus::kSettled);
    assert(QueryInputRoutingRetirement(published_next.predecessor).status ==
           (complete_before_publication ? InputRoutingRetirementStatus::kRunnable
                                        : InputRoutingRetirementStatus::kAdmittedSend));
    if (!complete_before_publication)
      assert(!CompleteInputRoutingPacket(std::move(sending), InputRoutingDeliveryResult::kAccepted));
    InputRoutingCancellationLease cancel;
    assert(AcquireInputRoutingCancellation(owner, &cancel));
    assert(CompleteInputRoutingCancellation(std::move(cancel), true));
  }
  // A historical packet-only purge is a real mutation and notifies observers.
  {
    using namespace darwin_art::input;
    auto owner = CreateInputRoutingState();
    auto old = PrepareInputRoutingRecipient(owner, 905);
    auto next = PrepareInputRoutingRecipient(owner, 905);
    assert(PublishInputRoutingRecipient(old).Published());
    assert(EnqueueInputRoutingPacket(owner, first_epoch_packet, 905));
    auto published_next = PublishInputRoutingRecipient(next);
    assert(published_next.Published());
    auto count = std::make_shared<int>(0);
    auto subscription = SubscribeInputRoutingNotifications(owner,
        {.on_notification = [](void* raw, const InputRoutingNotification&) {
           ++*static_cast<int*>(raw);
         }, .context = count.get(), .context_token = count});
    const auto before = QueryInputRoutingRetirement(published_next.predecessor, subscription);
    auto retired_old = RetireInputRoutingRecipient(old);
    const auto after = QueryInputRoutingRetirement(retired_old.ticket, subscription);
    assert(retired_old.status == InputRoutingRecipientRetirementStatus::kHistorical);
    assert(retired_old.changed && *count == 1 && after.revision > before.revision);
    assert(!InputRoutingHasPending(owner) && InputRoutingConsumerMatches(owner, 905));
  }

  // JNI receiver retirement purges local envelopes, not a remote peer's
  // outstanding stream barrier. A retained old endpoint must still own it.
  auto retired = CreateInputRoutingState();
  auto retired_endpoint = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{transport_identity, 901});
  SetInputRoutingConsumer(retired, 901, retired_endpoint);
  PublishInputRoutingWmsFrame(retired, 0, 0, 100, 100, true);
  SetInputRoutingTransportReady(retired, 901, true);
  assert(SetInputRoutingFocus(retired, 901));
  assert(RouteFrameworkPointerPacket(
      Pointer(DARWIN_ART_POINTER_DOWN, 20, 20, 1), &admission) ==
      DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(admission), true, false));
  assert(SetInputRoutingConsumer(retired, 0, {}, 901) == 901);
  assert(darwin_art::input::RetireInputRoutingConsumer(retired, 901));
  InputRoutingCancellationLease retirement_cancel;
  assert(AcquireInputRoutingCancellation(retired, &retirement_cancel));
  assert(retirement_cancel.Endpoint() == retired_endpoint);
  assert(retirement_cancel.Packet()->pointer.action == DARWIN_ART_POINTER_CANCEL);
  assert(CompleteInputRoutingCancellation(std::move(retirement_cancel), true));
  assert(!AcquireInputRoutingCancellation(retired, &retirement_cancel));

  // Immutable recipient/ticket handoff: generation changes retain every
  // prior id+endpoint record, and an old ticket cannot detach a same-id
  // successor (even when the transport lane is reused).
  const auto ticket_state = CreateInputRoutingState();
  auto ticket_owner_a = std::make_shared<int>(51);
  auto ticket_transport_a = std::shared_ptr<darwin_art::input::InputTransport>(
      ticket_owner_a,
      reinterpret_cast<darwin_art::input::InputTransport*>(ticket_owner_a.get()));
  auto ticket_endpoint_a = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{ticket_transport_a, 701});
  auto prepared_a = darwin_art::input::PrepareInputRoutingRecipient(
      ticket_state, 701, ticket_endpoint_a);
  assert(prepared_a != nullptr && prepared_a->Routing() == ticket_state &&
         prepared_a->id == 701 && prepared_a->originalendpoint == ticket_endpoint_a);
  auto first_publication = darwin_art::input::PublishInputRoutingRecipient(prepared_a);
  assert(first_publication.Published() && first_publication.predecessor.Routing() == nullptr);
  assert(!PublishInputRoutingWmsFrame(ticket_state, 0, 0, 100, 100, true));
  assert(SetInputRoutingFocus(ticket_state, 701));
  SetInputRoutingTransportReady(ticket_state, 701, false);
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 10, 10, 601),
                                     &admission) == DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease old_send;
  assert(ReserveInputRoutingPacket(std::move(admission), false, &old_send));
  assert(AcquireInputRoutingHead(ticket_state, &old_send));
  assert(AdmitInputRoutingTransportSend(old_send));
  const auto old_generation = old_send.Generation();
  auto prepared_same_id = darwin_art::input::PrepareInputRoutingRecipient(
      ticket_state, 701, ticket_endpoint_a);
  assert(prepared_same_id != nullptr);
  const auto same_id_ticket =
      darwin_art::input::PublishInputRoutingRecipient(prepared_same_id).predecessor;
  assert(same_id_ticket.ConsumerId() == 701 && same_id_ticket.CutoffGeneration() >= old_generation &&
         same_id_ticket.OriginalEndpoint() == ticket_endpoint_a);
  assert(darwin_art::input::QueryInputRoutingRetirement(same_id_ticket).status ==
         darwin_art::input::InputRoutingRetirementStatus::kAdmittedSend);
  assert(darwin_art::input::RetireInputRoutingRecipient(same_id_ticket));
  assert(InputRoutingConsumerMatches(ticket_state, 701));
  assert(GetInputRoutingEndpoint(ticket_state) == ticket_endpoint_a);
  // Completing the old admitted send is exact to its old generation and
  // cannot mutate the successor's capture state.
  assert(!CompleteInputRoutingPacket(
      std::move(old_send), InputRoutingDeliveryResult::kAccepted));
  assert(darwin_art::input::QueryInputRoutingRetirement(same_id_ticket).status !=
         darwin_art::input::InputRoutingRetirementStatus::kAdmittedSend);

  auto ticket_owner_b = std::make_shared<int>(52);
  auto ticket_transport_b = std::shared_ptr<darwin_art::input::InputTransport>(
      ticket_owner_b,
      reinterpret_cast<darwin_art::input::InputTransport*>(ticket_owner_b.get()));
  auto ticket_endpoint_b = std::make_shared<const darwin_art::input::InputRoutingEndpoint>(
      darwin_art::input::InputRoutingEndpoint{ticket_transport_b, 702});
  auto prepared_b = darwin_art::input::PrepareInputRoutingRecipient(
      ticket_state, 701, ticket_endpoint_b);
  assert(prepared_b != nullptr);
  const auto different_transport_ticket =
      darwin_art::input::PublishInputRoutingRecipient(prepared_b).predecessor;
  assert(different_transport_ticket.OriginalEndpoint() == ticket_endpoint_a);
  // The intermediate publication never admitted work. Its shared endpoint
  // does not give it authority over the first publication's pending CANCEL.
  assert(!darwin_art::input::RetireInputRoutingRecipient(different_transport_ticket));
  assert(darwin_art::input::QueryInputRoutingRetirement(different_transport_ticket).status ==
         darwin_art::input::InputRoutingRetirementStatus::kSettled);
  assert(InputRoutingConsumerMatches(ticket_state, 701));
  assert(GetInputRoutingEndpoint(ticket_state) == ticket_endpoint_b);

  // Publication tokens cannot be recycled after a predecessor ticket exists.
  const auto rejected_republication =
      darwin_art::input::PublishInputRoutingRecipient(prepared_same_id);
  assert(rejected_republication.status ==
         darwin_art::input::InputRoutingPublicationStatus::kAlreadyPublished);
  assert(rejected_republication.predecessor.ConsumerId() == 0);
  assert(GetInputRoutingEndpoint(ticket_state) == ticket_endpoint_b);
  (void)darwin_art::input::RetireInputRoutingRecipient(same_id_ticket);
  assert(GetInputRoutingEndpoint(ticket_state) == ticket_endpoint_b);

  // subscribe-before-query and reentrant callbacks: notification delivery is
  // revisioned and occurs with routing/focus locks released.
  auto notification_token = std::make_shared<int>(1);
  size_t notification_count = 0;
  uint64_t last_revision = 0;
  darwin_art::input::InputRoutingNotificationCallbacks callbacks;
  callbacks.context = &notification_count;
  callbacks.context_token = notification_token;
  callbacks.on_notification = [](void* context,
                                 const darwin_art::input::InputRoutingNotification& event) noexcept {
    auto* count = static_cast<size_t*>(context);
    ++*count;
    (void)event;
  };
  const auto subscription = darwin_art::input::SubscribeInputRoutingNotifications(
      ticket_state, callbacks);
  assert(subscription != nullptr);
  const auto before_query = darwin_art::input::QueryInputRoutingRetirement(
      same_id_ticket, subscription);
  last_revision = before_query.revision;
  assert(darwin_art::input::RetireInputRoutingRecipient(same_id_ticket));
  const auto after_query = darwin_art::input::QueryInputRoutingRetirement(
      same_id_ticket, subscription);
  assert(after_query.revision >= last_revision && notification_count > 0);
  notification_token.reset();
  const auto count_before_expiry = notification_count;
  (void)darwin_art::input::RetireInputRoutingRecipient(same_id_ticket);
  assert(notification_count == count_before_expiry);

  // A callback can destroy its externally owned token/subscription and reenter
  // routing. The delivery pin must keep context alive until invocation ends.
  struct ReentrantContext {
    std::shared_ptr<ReentrantContext>* external_owner;
    darwin_art::input::InputRoutingNotificationSubscriptionHandle* subscription;
    const darwin_art::input::InputRoutingRetirementTicket* ticket;
    bool* destroyed;
    bool* invoked;
    ~ReentrantContext() { *destroyed = true; }
  };
  bool context_destroyed = false;
  bool context_invoked = false;
  std::shared_ptr<ReentrantContext> context;
  darwin_art::input::InputRoutingNotificationSubscriptionHandle reentrant_subscription;
  context = std::make_shared<ReentrantContext>(
      ReentrantContext{&context, &reentrant_subscription,
                       &same_id_ticket, &context_destroyed,
                       &context_invoked});
  // Construction's temporary is not the delivered context.
  context_destroyed = false;
  darwin_art::input::InputRoutingNotificationCallbacks reentrant_callbacks;
  reentrant_callbacks.context = context.get();
  reentrant_callbacks.context_token = context;
  reentrant_callbacks.on_notification = [](void* raw, const auto&) {
    auto* current = static_cast<ReentrantContext*>(raw);
    current->external_owner->reset();
    current->subscription->reset();
    assert(!*current->destroyed);
    (void)darwin_art::input::QueryInputRoutingRetirement(*current->ticket);
    *current->invoked = true;
  };
  reentrant_subscription = darwin_art::input::SubscribeInputRoutingNotifications(
      ticket_state, reentrant_callbacks);
  assert(reentrant_subscription);
  (void)darwin_art::input::RetireInputRoutingRecipient(same_id_ticket);
  assert(context_invoked && context_destroyed && !context && !reentrant_subscription);

  // The first subscriber destroys the caller's ticket. Delivery must retain
  // its own routing handle while traversing subsequent weak subscribers.
  size_t later_notifications = 0;
  auto later_token = std::make_shared<int>(1);
  auto later_callbacks = callbacks;
  later_callbacks.context = &later_notifications;
  later_callbacks.context_token = later_token;
  const auto later_subscription = darwin_art::input::SubscribeInputRoutingNotifications(
      ticket_state, later_callbacks);
  auto ticket_holder = std::make_unique<darwin_art::input::InputRoutingRetirementTicket>(
      same_id_ticket);
  auto destroy_callbacks = callbacks;
  destroy_callbacks.context = &ticket_holder;
  destroy_callbacks.context_token = later_token;
  destroy_callbacks.on_notification = [](void* raw, const auto&) {
    static_cast<decltype(ticket_holder)*>(raw)->reset();
  };
  const auto destroying_subscription =
      darwin_art::input::SubscribeInputRoutingNotifications(ticket_state, destroy_callbacks);
  assert(later_subscription && destroying_subscription);
  (void)darwin_art::input::RetireInputRoutingRecipient(*ticket_holder);
  assert(!ticket_holder && later_notifications == 1);

  // Exhaust the action FIFO while a pointer stream is active. Replacement
  // cannot allocate its CANCEL yet, then retirement purges the old key actions.
  // The original stream obligation must survive and retry exactly once.
  const auto cancel_state = CreateInputRoutingState();
  SetInputRoutingConsumer(cancel_state, 801);
  (void)PublishInputRoutingWmsFrame(cancel_state, 2000, 2000, 2100, 2100, true);
  assert(SetInputRoutingFocus(cancel_state, 801));
  DeliverFocusForKeys(cancel_state, 1);
  SetInputRoutingTransportReady(cancel_state, 801, true);
  darwin_art::input::InputRoutingAdmission cancel_admission;
  assert(RouteFrameworkPointerPacket(
      Pointer(DARWIN_ART_POINTER_DOWN, 2050, 2050, 900), &cancel_admission) ==
      DarwinArtInputEnqueueResult::kQueued);
  assert(CommitInputRoutingPacket(std::move(cancel_admission), true, true));
  DarwinArtInputPacket cancel_packet;
  assert(DequeueInputRoutingPacket(cancel_state, &cancel_packet, 801));
  InputRoutingInflightLease pending_keys[256];
  for (auto& pending_key : pending_keys) {
    assert(darwin_art::input::RouteFrameworkKeyPacket(key.key, &cancel_admission) ==
           DarwinArtInputEnqueueResult::kQueued);
    assert(ReserveInputRoutingPacket(std::move(cancel_admission), true, &pending_key));
  }
  const auto successor = darwin_art::input::PrepareInputRoutingRecipient(cancel_state, 802);
  const auto pending_cancel_ticket =
      darwin_art::input::PublishInputRoutingRecipient(successor).predecessor;
  assert(darwin_art::input::RetireInputRoutingRecipient(pending_cancel_ticket));
  assert(darwin_art::input::QueryInputRoutingRetirement(pending_cancel_ticket).status !=
         darwin_art::input::InputRoutingRetirementStatus::kSettled);
  assert(darwin_art::input::RetryInputRoutingCancellations(cancel_state));
  InputRoutingCancellationLease pending_cancel;
  assert(AcquireInputRoutingCancellation(cancel_state, &pending_cancel));
  assert(pending_cancel.ConsumerId() == 801);
  assert(darwin_art::input::CompleteInputRoutingCancellation(
      std::move(pending_cancel), darwin_art::input::InputRoutingCancellationResult::kAccepted));
  assert(darwin_art::input::QueryInputRoutingRetirement(pending_cancel_ticket).status ==
         darwin_art::input::InputRoutingRetirementStatus::kSettled);

  // Readiness advances generations, but cannot slip between conditional
  // snapshot and detach or turn a missed detach into apparent success.
  for (int iteration = 0; iteration < 32; ++iteration) {
    const auto concurrent = CreateInputRoutingState();
    SetInputRoutingConsumer(concurrent, 901);
    std::atomic<bool> start{false};
    std::thread readiness([&] {
      while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
      for (int transition = 0; transition < 100; ++transition)
        SetInputRoutingTransportReady(concurrent, 901, transition % 2 != 0);
    });
    start.store(true, std::memory_order_release);
    assert(SetInputRoutingConsumer(concurrent, 0, {}, 901) == 901);
    readiness.join();
    assert(!InputRoutingConsumerMatches(concurrent, 901));
    SetInputRoutingConsumer(concurrent, 902);
    assert(SetInputRoutingConsumer(concurrent, 903, {}, 901) == 0);
    assert(InputRoutingConsumerMatches(concurrent, 902));
    assert(SetInputRoutingConsumer(concurrent, 0, {}, 901) == 0);
    assert(InputRoutingConsumerMatches(concurrent, 902));
  }

  std::puts("input-routing: PASS geometry/capture/cancel/replacement/bound/batch-drain/retired-barrier");
  return 0;
}
