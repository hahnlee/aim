#pragma once

#include "input_routing.h"

namespace darwin_art::input {

enum class ReceiverRoutingPublishStatus : uint8_t {
  kCompleted,
  kClosed,
  kBusy,
  kAlreadyAttempted,
};
struct ReceiverRoutingPublishResult {
  ReceiverRoutingPublishStatus status = ReceiverRoutingPublishStatus::kClosed;
  InputRoutingPublicationResult publication;
};
enum class ReceiverRoutingCloseStatus : uint8_t { kClosed, kDeferred };
struct ReceiverRoutingLifecycleSnapshot {
  bool close_requested = false;
  bool logical_closed = false;
  bool operation_pending = false;
  InputRoutingPublicationResult publication;
  InputRoutingRecipientRetirementResult retirement;
};

// Android InputEventReceiver's exact routing publication/revocation owner.
// No JNI, descriptor, Looper or transport-pump ownership. Logical closure is
// NOT ticket settlement, admission quiescence or permission to close an FD.
// Prepare before registry visibility. Calls pin shared control so notification
// reentry may destroy the containing receiver without invalidating an operation.
class ReceiverRoutingLifecycle final {
 public:
  explicit ReceiverRoutingLifecycle(InputRoutingRecipientHandle recipient);
  // No allocation. Retirement coordination must retain this independent handle
  // BEFORE receiver visibility/removal; a receiver member alone cannot preserve
  // retries after callback destruction plus an unexpected ledger exception.
  ReceiverRoutingLifecycle Retain() const;
  InputRoutingEndpointHandle OriginalEndpoint() const;
  ReceiverRoutingLifecycle(const ReceiverRoutingLifecycle&) = delete;
  ReceiverRoutingLifecycle& operator=(const ReceiverRoutingLifecycle&) = delete;
  // A returned ledger result consumes the attempt, even a current-ID mismatch.
  // A thrown preparation failure permits retry unless Close was requested.
  ReceiverRoutingPublishResult Publish(ReceiverId expected_current_id = 0);
  // Irreversible; publication/retirement reentry defers without waiting. The
  // elected operation completes the close. An unexpected exception retains
  // control and permits another Close attempt; caller must retain this owner.
  ReceiverRoutingCloseStatus Close();
  ReceiverRoutingLifecycleSnapshot Snapshot() const;
 private:
  struct Control;
  explicit ReceiverRoutingLifecycle(std::shared_ptr<Control> control)
      : control_(std::move(control)) {}
  static ReceiverRoutingCloseStatus CloseControl(const std::shared_ptr<Control>&);
  const std::shared_ptr<Control> control_;
};

}  // namespace darwin_art::input
