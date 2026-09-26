#pragma once

#include <cstdint>
#include <memory>
#include <utility>
#include <vector>

#include "../../../compat/darwin_framework_input_hint.h"
#include "receiver_registry.h"
#include "input_routing_endpoint.h"

namespace darwin_art::input {

// Opaque Android input-routing state.  Transport, JNI and Parcel ownership
// stay in channel_owner; this handle contains only routing policy and its
// bounded packet queue.
struct InputRoutingState;
using InputRoutingHandle = std::shared_ptr<InputRoutingState>;
using InputRoutingEndpointHandle = std::shared_ptr<const InputRoutingEndpoint>;

// A recipient is an immutable publication token.  It is deliberately
// allocated before publication so an allocation failure cannot leave a
// partially-published receiver in the routing owner.  endpoint is retained
// as the original endpoint; routing never reconstructs it from the current
// channel endpoint when retiring an older generation.
struct InputRoutingRetirementTicket;
struct InputRoutingPublicationResult;
struct InputRoutingRecipient {
  // Weak by design: the routing state owns the published/history record and
  // must not be kept alive by its immutable recipient token.
  const std::weak_ptr<InputRoutingState> routing;
  const ReceiverId id;
  const InputRoutingEndpointHandle originalendpoint;

  InputRoutingRecipient(InputRoutingHandle owner, ReceiverId receiver_id,
                        InputRoutingEndpointHandle endpoint)
      : routing(owner), id(receiver_id),
        originalendpoint(std::move(endpoint)) {}
  InputRoutingHandle Routing() const { return routing.lock(); }
  InputRoutingEndpointHandle OriginalEndpoint() const { return originalendpoint; }
 private:
  // Protected by this recipient's routing mutex. A prepared publication token
  // is single-use; it cannot be recycled to give a stale ticket new authority.
  mutable bool published_ = false;
  friend InputRoutingPublicationResult PublishInputRoutingRecipient(
      const std::shared_ptr<const InputRoutingRecipient>&, ReceiverId);
};
using InputRoutingRecipientHandle = std::shared_ptr<const InputRoutingRecipient>;

class RootKeyAuthorityTicket;
using RootKeyAuthorityTicketHandle = std::shared_ptr<const RootKeyAuthorityTicket>;
// Immutable original key authority/readiness facts. Queueing/claiming is not
// delivery admission; assigned keys may never acquire a successor fence.
struct RootKeyRoutingFence {
  RootKeyAuthorityTicketHandle ticket;
  uint64_t focus_epoch = 0;
  uint64_t focus_cache_revision = 0;
  InputRoutingRecipientHandle focus_recipient;
  bool required = false;
};

// Exact handoff record returned by PublishInputRoutingRecipient.  The
// recipient token is retained privately by the value (and is exposed only
// through Recipient()) so a stale ticket cannot detach a later publication
// with the same numeric id and transport.
struct InputRoutingRetirementTicket {
  InputRoutingHandle Routing() const { return routing_; }
  ReceiverId ConsumerId() const { return id_; }
  uint64_t CutoffGeneration() const { return cutoffgeneration_; }
  InputRoutingEndpointHandle OriginalEndpoint() const { return originalendpoint_; }
  InputRoutingRecipientHandle Recipient() const { return recipient_; }

 private:
  friend struct InputRoutingTicketAccess;
  InputRoutingHandle routing_;
  ReceiverId id_ = 0;
  uint64_t cutoffgeneration_ = 0;
  InputRoutingEndpointHandle originalendpoint_;
  InputRoutingRecipientHandle recipient_;
};
using InputRoutingTicket = InputRoutingRetirementTicket;

InputRoutingRecipientHandle PrepareInputRoutingRecipient(
    const InputRoutingHandle& state, ReceiverId id,
    InputRoutingEndpointHandle original_endpoint = {});
enum class InputRoutingPublicationStatus : uint8_t {
  kInvalidRecipient,
  kCurrentMismatch,
  kAlreadyPublished,
  kPublished,
};
struct InputRoutingPublicationResult {
  InputRoutingPublicationStatus status = InputRoutingPublicationStatus::kInvalidRecipient;
  // This ticket belongs to the predecessor, never the newly published token.
  InputRoutingRetirementTicket predecessor;
  bool Published() const { return status == InputRoutingPublicationStatus::kPublished; }
};
// Successful first publication has kPublished with an empty predecessor.
InputRoutingPublicationResult PublishInputRoutingRecipient(
    const InputRoutingRecipientHandle& recipient,
    ReceiverId expected_current_id = 0);
bool RetireInputRoutingRecipient(const InputRoutingRetirementTicket& ticket);

enum class InputRoutingRecipientRetirementStatus : uint8_t {
  kInvalidRecipient,
  kNotPublished,
  kDetached,
  kHistorical,
};
struct InputRoutingRecipientRetirementResult {
  InputRoutingRecipientRetirementStatus status =
      InputRoutingRecipientRetirementStatus::kInvalidRecipient;
  InputRoutingRetirementTicket ticket;
  bool changed = false;
};
// Capture the latest exact-token cutoff and retire in one domain→channel
// transaction. A generation change cannot slip between capture and detach.
// Historical retirement never reconstructs authority from a numeric ID.
InputRoutingRecipientRetirementResult RetireInputRoutingRecipient(
    const InputRoutingRecipientHandle& recipient);

enum class InputRoutingRetirementStatus : uint8_t {
  kAdmittedSend,
  kFifoHeadBlocked,
  kRunnable,
  kTransportCapacity,
  kSettled,
  kTerminal,
  AdmittedSend = kAdmittedSend,
  FIFOHeadBlocked = kFifoHeadBlocked,
  Runnable = kRunnable,
  TransportCapacity = kTransportCapacity,
  Settled = kSettled,
  Terminal = kTerminal,
};
using InputRoutingTicketStatus = InputRoutingRetirementStatus;

struct InputRoutingRetirementQuery {
  InputRoutingRetirementStatus status = InputRoutingRetirementStatus::kSettled;
  uint64_t revision = 0;
  // Terminal routing does not revoke an already admitted transport call.
  // Settlement/FD handoff must independently wait for this fact to clear.
  bool admitted_send = false;
};

InputRoutingRetirementQuery QueryInputRoutingRetirement(
    const InputRoutingRetirementTicket& ticket);
inline InputRoutingRetirementQuery QueryInputRoutingTicket(
    const InputRoutingRetirementTicket& ticket) {
  return QueryInputRoutingRetirement(ticket);
}
inline InputRoutingRetirementStatus QueryInputRoutingStatus(
    const InputRoutingRetirementTicket& ticket) {
  return QueryInputRoutingRetirement(ticket).status;
}

enum class InputRoutingNotificationKind : uint8_t {
  kPacketCompletion,
  kCancellationCompletion,
  kActionUnclaimed,
  kLeaseReleased,
  kRetirement,
  kTerminalCleanup,
  // The bounded local queue changed from full to having capacity. Advisory:
  // a subscriber must re-query the exact retirement ticket before retrying.
  kLocalCapacity,
  // Emitted only after the exact receiver-focus readiness transaction unlocks.
  kFocusReadiness,
};

struct InputRoutingNotification {
  InputRoutingNotificationKind kind =
      InputRoutingNotificationKind::kPacketCompletion;
  uint64_t revision = 0;
  ReceiverId id = 0;
  uint64_t generation = 0;
  InputRoutingEndpointHandle originalendpoint;
};

struct InputRoutingNotificationCallbacks {
  void (*on_notification)(void*, const InputRoutingNotification&) = nullptr;
  void* context = nullptr;
  // Notification delivery is weak: the routing owner never retains the
  // callback context.  Subscribers should retain this token themselves.
  std::weak_ptr<void> context_token;
};

struct InputRoutingNotificationSubscription;
using InputRoutingNotificationSubscriptionHandle =
    std::shared_ptr<InputRoutingNotificationSubscription>;

InputRoutingNotificationSubscriptionHandle SubscribeInputRoutingNotifications(
    const InputRoutingHandle& state,
    InputRoutingNotificationCallbacks callbacks);
InputRoutingRetirementQuery QueryInputRoutingRetirement(
    const InputRoutingRetirementTicket& ticket,
    const InputRoutingNotificationSubscriptionHandle& subscription);

// A route is only a proposal.  The channel owner reserves an opaque lease
// before touching a local queue or remote endpoint; completion revalidates the
// immutable generation/receiver snapshot so replacement/hidden publication
// cannot commit stale input.
struct InputRoutingAdmission {
  InputRoutingAdmission() = default;
  InputRoutingAdmission(const InputRoutingAdmission&) = delete;
  InputRoutingAdmission& operator=(const InputRoutingAdmission&) = delete;
  InputRoutingAdmission(InputRoutingAdmission&&) = default;
  InputRoutingAdmission& operator=(InputRoutingAdmission&&) = default;

  InputRoutingHandle state;
  darwin_art::DarwinArtInputPacket packet;
  uint64_t generation = 0;
  ReceiverId consumer_id = 0;
  InputRoutingEndpointHandle endpoint;
  int32_t offset_x = 0;
  int32_t offset_y = 0;
  bool pointer_down = false;
  bool pointer_end = false;
  bool local_transport_ready = false;
  // Key admissions are valid only while this exact recipient/cache
  // revision/epoch remains successfully delivered to Java.
  bool focus_ready = false;
  bool legacy_key_focus = false;
  uint64_t focus_epoch = 0;
  uint64_t focus_cache_revision = 0;
  InputRoutingRecipientHandle focus_recipient;
  RootKeyRoutingFence key_fence;
};

enum class InputRoutingDeliveryResult : uint8_t;

struct RoutingActionLeaseState;

struct InputRoutingInflightLease {
  InputRoutingInflightLease() = default;
  ~InputRoutingInflightLease();
  InputRoutingInflightLease(const InputRoutingInflightLease&) = delete;
  InputRoutingInflightLease& operator=(const InputRoutingInflightLease&) = delete;
  InputRoutingInflightLease(InputRoutingInflightLease&& other) noexcept;
  InputRoutingInflightLease& operator=(InputRoutingInflightLease&& other) noexcept;
  const darwin_art::DarwinArtInputPacket* Packet() const;
  InputRoutingEndpointHandle Endpoint() const;
  ReceiverId ConsumerId() const;
  uint64_t Generation() const;
  int32_t OffsetX() const;
  int32_t OffsetY() const;
  bool PointerDown() const;
  bool PointerEnd() const;
  bool LocalDelivery() const;
  explicit operator bool() const;

 private:
  std::shared_ptr<RoutingActionLeaseState> state_;
  friend struct RoutingActionLeaseAccess;
};

// A non-destructive lease over the exact head of the local packet FIFO.  The
// receiver owner uses this while invoking Java: destruction before explicit
// completion leaves a live-publication packet at the head so a deferred
// callback cannot lose it or allow a later packet to overtake it. If retirement
// replaced the publication while the callback was deferred, destruction
// settles that stale record as a terminal rejection so successor traffic is
// not stranded. Completion is the only live-publication consume operation (and
// publishes local capacity).
struct InputRoutingPacketLeaseState;
struct InputRoutingPacketLease {
  InputRoutingPacketLease() = default;
  ~InputRoutingPacketLease();
  InputRoutingPacketLease(const InputRoutingPacketLease&) = delete;
  InputRoutingPacketLease& operator=(const InputRoutingPacketLease&) = delete;
  InputRoutingPacketLease(InputRoutingPacketLease&& other) noexcept;
  InputRoutingPacketLease& operator=(InputRoutingPacketLease&& other) noexcept;

  const darwin_art::DarwinArtInputPacket* Packet() const;
  InputRoutingRecipientHandle Recipient() const;
  InputRoutingEndpointHandle Endpoint() const;
  // Consume an invoked packet.  The bool/result overloads consume either an
  // invocation or an explicit rejection; backpressure rolls the claim back.
  bool Complete();
  bool Complete(bool invoked);
  bool Complete(InputRoutingDeliveryResult result);
  RootKeyRoutingFence KeyFence() const;
  explicit operator bool() const;

 private:
  std::shared_ptr<InputRoutingPacketLeaseState> state_;
  friend bool AcquireInputRoutingPacketLease(
      const InputRoutingHandle&, const InputRoutingRecipientHandle&,
      InputRoutingPacketLease*);
  friend bool BeginInputRoutingPacketDelivery(InputRoutingPacketLease&,
      const InputRoutingRecipientHandle&, const InputRoutingHandle&);
};
// Single-use irreversible Java admission, after callback-capable JNI work.
// Claiming a FIFO head alone grants no delivery authority.
bool BeginInputRoutingPacketDelivery(InputRoutingPacketLease&,
    const InputRoutingRecipientHandle&, const InputRoutingHandle&);

// Cancellation is acquired exclusively by one channel-owner pump.  The
// packet remains in the routing owner until Complete/Discard reports endpoint
// admission, so concurrent pumps cannot pop or duplicate an old-epoch cancel.
struct InputRoutingCancellationLease {
  InputRoutingCancellationLease() = default;
  ~InputRoutingCancellationLease();
  InputRoutingCancellationLease(const InputRoutingCancellationLease&) = delete;
  InputRoutingCancellationLease& operator=(const InputRoutingCancellationLease&) = delete;
  InputRoutingCancellationLease(InputRoutingCancellationLease&& other) noexcept;
  InputRoutingCancellationLease& operator=(InputRoutingCancellationLease&& other) noexcept;
  const darwin_art::DarwinArtInputPacket* Packet() const;
  InputRoutingEndpointHandle Endpoint() const;
  ReceiverId ConsumerId() const;
  uint64_t Generation() const;
  explicit operator bool() const;

 private:
  std::shared_ptr<RoutingActionLeaseState> state_;
  friend struct RoutingActionLeaseAccess;
};

InputRoutingHandle CreateInputRoutingState();

// Consumer/geometry publication is performed at the framework receiver and
// WindowManager boundaries. SetConsumer returns the replaced consumer ID;
// expected_current_id makes disposal a single atomic compare-and-update.
ReceiverId SetInputRoutingConsumer(const InputRoutingHandle& state,
                                   ReceiverId consumer_id,
                                   InputRoutingEndpointHandle endpoint = {},
                                   ReceiverId expected_current_id = 0);
// Retire one receiver identity without touching a replacement published on
// the same channel. Old envelopes/ledgers are removed or left only until
// their already-admitted endpoint action settles.
bool RetireInputRoutingConsumer(const InputRoutingHandle& state,
                                ReceiverId consumer_id);
bool InputRoutingConsumerMatches(const InputRoutingHandle& state,
                                 ReceiverId consumer_id);
// Exact publication check for receiver owners. Numeric IDs alone are not
// sufficient when a channel replaces a recipient between callbacks.
bool IsInputRoutingRecipientCurrent(
    const InputRoutingHandle& state,
    const InputRoutingRecipientHandle& recipient);
bool InputRoutingTransportReady(const InputRoutingHandle& state);
void SetInputRoutingTransportReady(const InputRoutingHandle& state,
                                   ReceiverId consumer_id, bool ready);
InputRoutingEndpointHandle GetInputRoutingEndpoint(
    const InputRoutingHandle& state);
// Terminal endpoint loss drains only actions owned by the terminated
// endpoint; it never reports them as accepted or touches a replacement.
void TerminateInputRoutingTransport(
    const InputRoutingHandle& state,
    const InputRoutingEndpointHandle& endpoint);
// Only WMS window publication establishes input eligibility and the
// window's InputWindowFlags.
bool PublishInputRoutingWmsFrame(const InputRoutingHandle& state, int32_t left,
                                 int32_t top, int32_t right, int32_t bottom,
                                 bool visible, uint32_t input_flags = 0);
// Receiver coordinates never override a WMS publication or enable routing.
bool UpdateInputRoutingReceiverGeometry(const InputRoutingHandle& state,
                                        int32_t left, int32_t top,
                                        int32_t right, int32_t bottom,
                                        ReceiverId expected_id);
// Legacy Init-time focus bookkeeping. It never establishes authoritative
// Java focus readiness; key routing requires the focus notification fence.
bool SetInputRoutingFocus(const InputRoutingHandle& state,
                          ReceiverId expected_id = 0);
bool ClearInputRoutingFocus(const InputRoutingHandle& state,
                            ReceiverId expected_id = 0);

// Queue operations used by channel_owner's transport/looper callbacks.
bool EnqueueInputRoutingPacket(const InputRoutingHandle& state,
                               const darwin_art::DarwinArtInputPacket& packet,
                               ReceiverId expected_id = 0);

// Claim the exact local FIFO head for one published recipient.  The token is
// compared by identity (not numeric receiver ID), so a replacement recipient
// cannot consume an older publication's packet.  This claim is deliberately
// non-destructive; the caller must explicitly Complete it after invocation or
// rejection, and may safely let the move-only lease destruct to defer work.
bool AcquireInputRoutingPacketLease(
    const InputRoutingHandle& state,
    const InputRoutingRecipientHandle& expected_recipient,
    InputRoutingPacketLease* lease);
// Commit a route after its selected transport admitted it.  local_delivery
// queues the packet here; remote delivery updates only routing/capture state.
enum class InputRoutingDeliveryResult : uint8_t {
  kAccepted,
  kBackpressured,
  kTerminal,
};
enum class InputRoutingPacketCompletionStatus : uint8_t {
  kAccepted,
  kBackpressured,
  kTerminal,
};

// Appends an immutable action reservation and returns an unclaimed opaque
// lease for that exact record.  The caller must immediately use
// AcquireInputRoutingHead before touching a transport; the reservation never
// bypasses an earlier FIFO action.
// Output must be empty. Rejection preserves a nonempty output; allocation
// failure leaves a fresh output empty with no published reservation.
bool ReserveInputRoutingPacket(InputRoutingAdmission&& admission,
                               bool local_delivery,
                               InputRoutingInflightLease* lease);
// Claims the first packet or CANCEL action in the state-owned FIFO. A claimed action remains
// in place until Complete reports acceptance/terminal or backpressure
// unclaims it; fresh submissions and writable retries use this same path.
bool AcquireInputRoutingHead(const InputRoutingHandle& state,
                             InputRoutingInflightLease* lease);
bool BeginInputRoutingTransportSend(InputRoutingInflightLease& lease);
bool CompleteInputRoutingPacket(InputRoutingInflightLease&& lease,
                                InputRoutingDeliveryResult result);
InputRoutingPacketCompletionStatus CompleteInputRoutingPacketWithStatus(
    InputRoutingInflightLease&& lease, InputRoutingDeliveryResult result);
// Linearization point immediately before channel_owner touches an endpoint.
// Packet sends require a live generation; CANCEL retains its retired recipient.
bool AdmitInputRoutingTransportSend(const InputRoutingInflightLease& lease);
bool AcquireInputRoutingRetry(const InputRoutingHandle& state,
                              InputRoutingInflightLease* lease);
bool CommitInputRoutingPacket(InputRoutingAdmission&& admission,
                              bool accepted, bool local_delivery);
// Focus/window teardown may need to notify the original remote endpoint.  The
// packet is retained until channel_owner reports transport admission, so
// backpressure and endpoint loss never fabricate delivery.
bool InputRoutingHasPendingCancellation(const InputRoutingHandle& state);
// Output must be empty; rejection must not release an existing claim.
bool AcquireInputRoutingCancellation(const InputRoutingHandle& state,
                                     InputRoutingCancellationLease* lease);
enum class InputRoutingCancellationResult : uint8_t {
  kAccepted,
  kBackpressured,
  kTerminal,
};
bool CompleteInputRoutingCancellation(
    InputRoutingCancellationLease&& lease,
    InputRoutingCancellationResult result);
bool CompleteInputRoutingCancellation(InputRoutingCancellationLease&& lease,
                                      bool accepted);
struct InputRoutingPacketEnvelope {
  darwin_art::DarwinArtInputPacket packet;
  ReceiverId consumer_id = 0;
  uint64_t generation = 0;
};
bool DequeueInputRoutingPacketEnvelope(const InputRoutingHandle& state,
                                       InputRoutingPacketEnvelope* envelope,
                                       ReceiverId expected_id = 0);
bool RequeueInputRoutingPacketEnvelopeFront(
    const InputRoutingHandle& state,
    const InputRoutingPacketEnvelope& envelope);
bool DequeueInputRoutingPacket(const InputRoutingHandle& state,
                               darwin_art::DarwinArtInputPacket* packet,
                               ReceiverId expected_id = 0);
bool RequeueInputRoutingPacketFront(
    const InputRoutingHandle& state,
    const darwin_art::DarwinArtInputPacket& packet, ReceiverId expected_id = 0);
bool InputRoutingHasPending(const InputRoutingHandle& state);
// Reports only local receiver packets, excluding remote actions and deferred
// cancellation obligations. A claimed head still counts as pending.
bool HasInputRoutingPackets(const InputRoutingHandle& state);
// A nonclaimed head can make progress without a transport-capacity event.
// Local queue capacity is checked here to avoid a deferred-task busy loop.
bool InputRoutingHasRunnableAction(const InputRoutingHandle& state);
// Queue recoverable local/remote CANCEL obligations through the original FIFO.
// Returns actual cancellation-action presence, not merely terminal settlement.
bool RetryInputRoutingCancellations(const InputRoutingHandle& state);
void ClearInputRoutingPending(const InputRoutingHandle& state);

// Routes host packets using published Android window geometry and focus.  The
// selected state is returned so channel_owner can perform the transport wake;
// routing itself never touches a file descriptor.
// A DOWN also yields the ACTION_OUTSIDE admissions of the windows watching
// outside touches above the touched window, when |outside| is supplied.
DarwinArtInputEnqueueResult RouteFrameworkPointerPacket(
    const DarwinArtPointerEventV2& packet, InputRoutingAdmission* admission,
    std::vector<InputRoutingAdmission>* outside = nullptr);
DarwinArtInputEnqueueResult RouteFrameworkKeyPacket(
    const DarwinArtKeyEventV1& packet, InputRoutingAdmission* admission);

}  // namespace darwin_art::input
