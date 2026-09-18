#pragma once

// Private lock-coherent storage for publication/local packets, action/stream
// delivery, exact packet claims and the focus executor. Each writer uses its
// own subsystem operations; public channel/receiver interfaces stay opaque.
#include "input_routing.h"
#include "input_focus_epoch.h"
#include "input_window_state.h"
#include <atomic>
#include <deque>
#include <mutex>
#include <unordered_map>
#include <vector>

namespace darwin_art::input {
struct InputRoutingNotificationLink;
namespace routing_internal {
inline constexpr size_t kMaxRoutingPackets = 256;
struct InputRoutingStateData {
  struct QueuedPacket {
    darwin_art::DarwinArtInputPacket packet;
    ReceiverId consumer_id = 0;
    uint64_t generation = 0;
    // Immutable publication identity captured when the packet enters the
    // local FIFO.  Numeric IDs are insufficient across replacement epochs.
    InputRoutingRecipientHandle recipient;
    // Packet leases claim without removing the head.  claim_id is owned by
    // the queue record and lets completion/rollback reject stale leases.
    uint64_t claim_id = 0;
    RootKeyRoutingFence key_fence;
  };
  enum class ActionKind : uint8_t { kPacket, kCancellation };
  struct ActionRecord {
    darwin_art::DarwinArtInputPacket packet;
    ReceiverId consumer_id = 0;
    uint64_t generation = 0;
    int32_t offset_x = 0, offset_y = 0;
    bool pointer_down = false, pointer_end = false, local_delivery = false;
    bool focus_ready = false;
    bool legacy_key_focus = false;
    uint64_t focus_epoch = 0, focus_cache_revision = 0;
    InputRoutingRecipientHandle focus_recipient;
    ActionKind kind = ActionKind::kPacket;
    uint64_t id = 0;
    bool claimed = false, send_admitted = false;
    InputRoutingEndpointHandle endpoint;
    RootKeyRoutingFence key_fence;
  };
  struct RetiredRecipient {
    InputRoutingRecipientHandle recipient;
    uint64_t generation = 0;
  };
  InputRoutingRecipientHandle recipient;
  // Exact history outlives traffic and generation changes, but not its owner.
  std::vector<RetiredRecipient> recipient_generations;
  mutable std::mutex mutex;
  std::deque<QueuedPacket> packets;
  ReceiverId consumer_id = 0;
  InputRoutingEndpointHandle endpoint;
  bool transport_ready = false;
  InputWindowState window;
  uint64_t focus_order = 0;
  uint64_t generation = 1;
  struct StreamLedger {
    uint64_t generation = 0;
    ReceiverId consumer_id = 0;
    bool remote = false, active = true, cancel_pending = false, cancel_queued = false;
    DarwinArtPointerEventV2 last_pointer{};
    InputRoutingEndpointHandle endpoint;
  };
  std::unordered_map<uint64_t, StreamLedger> streams;
  uint64_t active_stream_generation = 0;
  // Reservation-order FIFO, including durable retirement CANCEL barriers.
  std::deque<std::shared_ptr<ActionRecord>> actions;
  std::vector<InputRoutingEndpointHandle> terminated_endpoints;
  uint64_t next_lease_id = 1;
  std::atomic<bool> pending_input{false};
  uint64_t revision = 0;
  std::shared_ptr<const InputRoutingNotificationLink> notification_subscriptions;
  // Focus policy is caller-owned per channel.  The executor is the only
  // writer; routing packet/action/stream ledgers remain below this boundary.
  InputFocusEpochEvaluator::ChannelCache focus_cache;
};

// Domain -> channel locks are held by the executor. Ledger preparation may
// throw; commit uses that preparation and preserves durable CANCEL obligations.
// Only the ledger owner touches packet/action/history/stream containers.
void PrepareFocusGenerationLocked(InputRoutingStateData* data);
bool HasPendingRecipientPacketsLocked(
    const InputRoutingStateData& data,
    const InputRoutingRecipientHandle& recipient);
void AdvanceFocusGenerationLocked(InputRoutingStateData* data, bool retire_stream);
bool FocusEndpointTerminatedLocked(const InputRoutingStateData& data);
// Geometry/terminal owners invalidate only policy cache state through this
// narrow helper; packet/action/stream state remains ledger-owned.
bool RevokeFocusCacheLocked(InputRoutingStateData* data,
                            uint64_t revocation_epoch = 0) noexcept;

// Delivery is kept in the routing owner; extracted packet-lease code may
// publish advisory events only through this narrow out-of-lock seam.
void NotifyInputRoutingState(const InputRoutingHandle& state,
                             InputRoutingNotificationKind kind, ReceiverId id,
                             uint64_t generation,
                             const InputRoutingEndpointHandle& endpoint);
}  // namespace routing_internal
struct InputRoutingState { routing_internal::InputRoutingStateData data; };
}  // namespace darwin_art::input
