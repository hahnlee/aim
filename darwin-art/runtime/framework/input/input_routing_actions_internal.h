#pragma once

// Explicit private seam for the routing action/stream ledger.  The public
// routing API remains in input_routing.h; this header is used only by the
// routing owner and its independently compiled action owner.
#include "input_routing.h"

namespace darwin_art::input {

namespace routing_internal {
struct InputRoutingStateData;

// All Locked ports require the caller's channel mutex. Retirement also runs
// in the enclosing domain -> channel transaction; none performs IO/callbacks.

// Queue ownership remains in input_routing.cc. Action completion uses this
// narrow locked callback to publish local delivery without touching transport.
bool QueueRoutingPacketLocked(
    InputRoutingStateData* state,
    const darwin_art::DarwinArtInputPacket& packet,
    ReceiverId consumer_id, uint64_t generation,
    InputRoutingRecipientHandle recipient = {},
    RootKeyRoutingFence key_fence = {});
bool HasRoutingPacketCapacityLocked(const InputRoutingStateData& state);

bool IsRoutingEndpointTerminatedLocked(
    const InputRoutingStateData& state,
    const InputRoutingEndpointHandle& endpoint);
bool HasRoutingEndpointReferencesLocked(
    const InputRoutingStateData& state,
    const InputRoutingEndpointHandle& endpoint);
bool MarkRoutingEndpointTerminatedLocked(
    InputRoutingStateData* state, const InputRoutingEndpointHandle& endpoint);
// Publication/history owner records the original epoch before retirement,
// or advances it within this same held transaction (prepared focus handoff).
bool RetireActiveRoutingStreamLocked(InputRoutingStateData* data);
bool RoutingEpochBelongsToTicketLocked(
    const InputRoutingStateData& data,
    const InputRoutingRetirementTicket& ticket, ReceiverId id,
    uint64_t generation);

// Publication/terminal owners use these operations instead of editing the
// action or stream containers directly.  Packet FIFO cleanup stays with the
// routing owner and is performed by the caller around these seams.
bool RetireRoutingActionsForRecipientLocked(
    InputRoutingStateData* data, const InputRoutingRetirementTicket& ticket);
struct RoutingActionLedgerQuery {
  bool admitted = false;
  bool exact_action = false;
  bool runnable = false;
  bool unallocated_cancellation = false;
  bool fifo_empty = true;
};
RoutingActionLedgerQuery QueryRoutingActionLedgerLocked(
    const InputRoutingStateData& data,
    const InputRoutingRetirementTicket& ticket);
void TerminateRoutingActionsForEndpointLocked(
    InputRoutingStateData* data, const InputRoutingEndpointHandle& endpoint);
bool HasPendingRoutingActionOrCancellationLocked(
    const InputRoutingStateData& data);
bool HasRoutingActionsLocked(const InputRoutingStateData& data);
bool IsRoutingActionHeadRunnableLocked(const InputRoutingStateData& data);
bool RetryRoutingCancellationsLocked(InputRoutingStateData* data);

}  // namespace routing_internal
}  // namespace darwin_art::input
