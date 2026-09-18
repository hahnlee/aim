#pragma once

#include "input_focus_epoch.h"

namespace darwin_art::input {

// Result of one exact-recipient focus transaction.  A successful result pins
// the recipient long enough for a JNI owner to invoke its callback after the
// domain/channel locks have been released.
struct InputRoutingFocusResult final {
  InputFocusEpochEvaluator::Decision decision =
      InputFocusEpochEvaluator::Decision::kRejectedNoLiveChannel;
  InputRoutingRecipientHandle recipient;
  FocusControl control;
  bool notification_present = false;
  // Revalidation fence captured after the authoritative cache commit.  A
  // callback that reenters this channel's focus policy advances this value;
  // unrelated channel activity does not invalidate an already-invoked Java
  // callback.
  uint64_t cache_revision = 0;

  bool Accepted() const noexcept {
    return decision == InputFocusEpochEvaluator::Decision::kAcceptedGain ||
           decision == InputFocusEpochEvaluator::Decision::kAcceptedLoss ||
           decision == InputFocusEpochEvaluator::Decision::kAcceptedReplay ||
           decision == InputFocusEpochEvaluator::Decision::kDuplicate;
  }
  bool ShouldNotify() const noexcept { return notification_present; }
  // Convenient for existing callback/pump code that only needs admission.
  operator bool() const noexcept { return Accepted(); }
};

// Apply a control only when expected is the exact currently published
// recipient.  The loss path is intentionally allowed to clear an old grant
// after a newer gain on another channel.
InputRoutingFocusResult ApplyInputRoutingFocusControl(
    const InputRoutingRecipientHandle& expected, FocusControl control);

// Replay the caller-owned cached grant after exact recipient replacement.
InputRoutingFocusResult ReplayCachedInputRoutingFocus(
    const InputRoutingRecipientHandle& expected);

// Preferred post-JNI form.  The result carries the exact publication and
// revisions that produced the notification, so reentrant newer loss/gain
// controls cannot make an older callback appear fresh.
// Call after the JNI callback returns.  invoked must be true once Java's
// onFocusEvent was actually invoked, including when Java threw or disposed
// the receiver reentrantly.
bool CommitInputRoutingFocusNotification(
    const InputRoutingFocusResult& result, bool invoked = true);

// Commit successful Java focus delivery separately from the invoked marker.
// A delivered=false callback never grants key admission, even when invoked is
// true because Java threw or disposed the receiver reentrantly.
bool CommitInputRoutingFocusReadiness(const InputRoutingFocusResult& result,
                                      bool delivered);

// Geometry/terminal owners use this narrow cache invalidation helper.  It
// never changes routing generations or packet/action/stream state.
bool RevokeInputRoutingFocus(const InputRoutingRecipientHandle& expected,
                             uint64_t revocation_epoch = 0);

}  // namespace darwin_art::input
