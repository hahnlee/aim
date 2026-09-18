#pragma once

#include "input_routing.h"
#include "input_focus_epoch.h"
#include "input_window_state.h"
#include <mutex>
#include <array>
#include <vector>

namespace darwin_art::input {
// Immutable selection facts only. Channel action/FIFO/stream state is never
// visible to the domain. The original recipient and endpoint pin this epoch.
struct InputRoutingSelectionSnapshot {
  bool eligible = false;
  InputWindowFrame frame;
  uint64_t focus_order = 0;
  uint64_t generation = 0;
  ReceiverId consumer_id = 0;
  InputRoutingRecipientHandle recipient;
  InputRoutingEndpointHandle endpoint;
  bool transport_ready = false;
  // A key admission must carry the exact successful-Java focus fence. These
  // fields are populated only for authoritative key routing.
  bool focus_ready = false;
  bool focus_grant = false;
  bool focus_revoked = true;
  uint64_t focus_epoch = 0;
  uint64_t focus_cache_revision = 0;
  InputRoutingRecipientHandle focus_recipient;
  std::weak_ptr<const InputRoutingRecipient> focus_ready_recipient;
  uint64_t focus_ready_epoch = 0;
  uint64_t focus_ready_cache_revision = 0;
  bool authoritative_focus = false;
};
InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(
    const InputRoutingHandle& channel);
bool ValidateInputRoutingSelection(const InputRoutingHandle& channel,
    const InputRoutingSelectionSnapshot& selection, InputRoutingAdmission* admission);

struct InputRoutingCaptureSnapshot {
  InputRoutingHandle channel;
  uint64_t generation = 0;
  int32_t offset_x = 0;
  int32_t offset_y = 0;
};
struct InputRoutingDomainState;
// Lock order: domain transaction -> channel mutex. Never acquire this while
// holding a channel lock. Revocation/capture and ledger cutoffs linearize in
// the same transaction; all notifications occur after BOTH locks are released.
class InputRoutingDomainTransaction final {
 public:
  ~InputRoutingDomainTransaction();
  InputRoutingDomainTransaction(InputRoutingDomainTransaction&&) = delete;
  InputRoutingDomainTransaction(const InputRoutingDomainTransaction&) = delete;
  InputRoutingDomainTransaction& operator=(const InputRoutingDomainTransaction&) = delete;
  InputRoutingHandle Focused() const;
  InputRoutingCaptureSnapshot Captured() const;
  void SetFocused(const InputRoutingHandle& channel);
  void ClearFocus(const InputRoutingHandle& channel);
  void ClearCapture(const InputRoutingHandle& channel, uint64_t generation = 0);
  void CommitAcceptedPointer(const InputRoutingAdmission& admission);
  // Narrow policy accessor.  The domain transaction owns the lock; callers
  // must not retain the reference after this transaction is destroyed.
  InputFocusEpochEvaluator::DomainRecord& FocusEpochDomain();
  const InputFocusEpochEvaluator::DomainRecord& FocusEpochDomain() const;
  uint64_t NextFocusOrder();
  void Register(const InputRoutingHandle& channel);
  std::vector<InputRoutingHandle> Candidates();
 private:
  explicit InputRoutingDomainTransaction(InputRoutingDomainState& state);
  InputRoutingDomainState* state_;
  std::unique_lock<std::mutex> lock_;
  // Every weak promotion remains pinned through unlock, including temporaries
  // and exception paths. Final channel/endpoint destruction may reenter routing.
  mutable std::vector<InputRoutingHandle> pins_;
  // Focus/capture revocation must remain allocation-free at ledger cutoffs.
  mutable std::array<InputRoutingHandle, 8> inline_pins_;
  InputRoutingHandle Pin(const std::weak_ptr<InputRoutingState>& channel) const;
  friend InputRoutingDomainTransaction LockInputRoutingDomain();
};
InputRoutingDomainTransaction LockInputRoutingDomain();
}  // namespace darwin_art::input
