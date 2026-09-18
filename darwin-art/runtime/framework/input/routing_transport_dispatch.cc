#include "routing_transport_dispatch.h"
#include "input_routing_endpoint.h"
#include "input_transport.h"
#include <utility>

namespace darwin_art::input {
namespace {

RoutingTransportDrainResult DrainHead(
    const InputRoutingHandle& routing, InputRoutingInflightLease&& head) {
  RoutingTransportDrainResult result;
  if (!head) return result;
  if (head.LocalDelivery()) {
    // Local delivery has the same admission linearization as remote sends;
    // this revalidates a retained key's successful-Java focus fence before it
    // can cross the queue boundary.
    if (!BeginInputRoutingTransportSend(head)) {
      (void)CompleteInputRoutingPacketWithStatus(
          std::move(head), InputRoutingDeliveryResult::kTerminal);
      result.terminal = true;
      result.settled = 1;
      return result;
    }
    const auto completion = CompleteInputRoutingPacketWithStatus(
        std::move(head), InputRoutingDeliveryResult::kAccepted);
    if (completion != InputRoutingPacketCompletionStatus::kAccepted) {
      result.backpressured =
          completion == InputRoutingPacketCompletionStatus::kBackpressured;
      result.terminal =
          completion == InputRoutingPacketCompletionStatus::kTerminal;
      if (result.terminal) result.settled = 1;
      return result;
    }
    result.local_queued = true;
    result.settled = 1;
    return result;
  }
  const auto endpoint = head.Endpoint();
  const auto transport = endpoint == nullptr ? nullptr : endpoint->transport;
  if (transport == nullptr || transport->RemoteEndpointFd() < 0) {
    (void)CompleteInputRoutingPacketWithStatus(
        std::move(head), InputRoutingDeliveryResult::kTerminal);
    TerminateInputRoutingTransport(routing, endpoint);
    result.terminal = true;
    result.settled = 1;
    return result;
  }
  if (!BeginInputRoutingTransportSend(head)) {
    (void)CompleteInputRoutingPacketWithStatus(
        std::move(head), InputRoutingDeliveryResult::kTerminal);
    result.terminal = true;
    result.settled = 1;
    return result;
  }
  const auto status = SendInputTransportPacket(transport.get(), *head.Packet());
  const auto completion = CompleteInputRoutingPacketWithStatus(
      std::move(head), status == InputTransportStatus::kAccepted
                          ? InputRoutingDeliveryResult::kAccepted
                          : status == InputTransportStatus::kTerminal
                                ? InputRoutingDeliveryResult::kTerminal
                                : InputRoutingDeliveryResult::kBackpressured);
  if (status == InputTransportStatus::kBackpressured) {
    result.backpressured = true;
    return result;
  }
  result.settled = 1;
  if (status == InputTransportStatus::kTerminal) {
    result.terminal = true;
    TerminateInputRoutingTransport(routing, endpoint);
  } else {
    // Accepted transport bytes with a stale routing epoch are consumed by the
    // owner but are not a successful framework enqueue.
    result.terminal =
        completion != InputRoutingPacketCompletionStatus::kAccepted;
    result.remote_admitted = true;  // Physical progress survives epoch retirement.
  }
  return result;
}

}  // namespace

RoutingTransportDrainResult DrainInputRoutingTransport(
    const InputRoutingHandle& routing, size_t budget) {
  RoutingTransportDrainResult result;
  (void)RetryInputRoutingCancellations(routing);
  for (size_t i = 0; i < budget; ++i) {
    InputRoutingInflightLease head;
    if (!AcquireInputRoutingHead(routing, &head)) break;
    const auto one = DrainHead(routing, std::move(head));
    result.settled += one.settled;
    result.remote_admitted |= one.remote_admitted;
    result.local_queued |= one.local_queued;
    result.terminal |= one.terminal;
    if (one.backpressured) {
      result.backpressured = true;
      break;  // Same retained head must wait for capacity, not busy-spin.
    }
  }
  // A full FIFO may have prevented the initial CANCEL allocation. Reconsider
  // obligations after this bounded drain frees capacity, before deciding that
  // there is no runnable continuation. OOM alone must not cause a busy loop.
  (void)RetryInputRoutingCancellations(routing);
  result.continuation_needed = !result.backpressured &&
      InputRoutingHasRunnableAction(routing);
  return result;
}

RoutingTransportSubmitResult SubmitInputRoutingAdmission(
    InputRoutingAdmission&& admission) {
  RoutingTransportSubmitResult result;
  // Snapshot before the move into the routing owner. The moved-from admission
  // is never consulted for status, packet, endpoint, or wake decisions.
  const auto routing = admission.state;
  const auto consumer_id = admission.consumer_id;
  const auto selected_endpoint = admission.endpoint;
  const bool local_delivery = admission.local_transport_ready;
  if (routing == nullptr) return result;

  InputRoutingInflightLease lease;
  if (!ReserveInputRoutingPacket(std::move(admission), local_delivery, &lease)) {
    result.result = InputRoutingConsumerMatches(routing, consumer_id)
                        ? DarwinArtInputEnqueueResult::kBackpressured
                        : DarwinArtInputEnqueueResult::kNoFocusedChannel;
    return result;
  }
  if (!AcquireInputRoutingHead(routing, &lease)) {
    result.result = DarwinArtInputEnqueueResult::kQueued;
    result.wake_local = true;
    return result;
  }
  const auto drained = DrainHead(routing, std::move(lease));
  // A concurrent submit may have retained a successor while this head owned
  // admission. Its local wake cannot schedule a remote-only endpoint.
  const auto continuation = drained.backpressured
      ? RoutingTransportDrainResult{}
      : DrainInputRoutingTransport(routing);
  result.refresh_writable =
      selected_endpoint != nullptr &&
      (drained.remote_admitted || continuation.remote_admitted);
  result.wake_local = drained.local_queued || continuation.local_queued;
  result.continuation_needed = continuation.continuation_needed;
  result.result = drained.terminal
                      ? DarwinArtInputEnqueueResult::kNoFocusedChannel
                      : DarwinArtInputEnqueueResult::kQueued;
  return result;
}

}  // namespace darwin_art::input
