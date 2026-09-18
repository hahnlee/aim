#include "runtime/framework/input/input_routing_focus.h"
#include "runtime/framework/input/input_routing_domain.h"

#include <atomic>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <new>

using namespace darwin_art::input;

// Test-only allocation injection. The objects under test are the production
// routing channel, domain, and focus executor.
static std::atomic<bool> fail_allocations{false};
static int allocations_before_failure = 0;
void* operator new(std::size_t size) {
  if (fail_allocations.load() && allocations_before_failure-- <= 0)
    throw std::bad_alloc();
  if (void* allocation = std::malloc(size == 0 ? 1 : size)) return allocation;
  throw std::bad_alloc();
}
void operator delete(void* allocation) noexcept { std::free(allocation); }

static InputRoutingRecipientHandle Publish(const InputRoutingHandle& owner,
                                           ReceiverId id) {
  auto recipient = PrepareInputRoutingRecipient(owner, id);
  assert(recipient != nullptr && PublishInputRoutingRecipient(recipient).Published());
  assert(PublishInputRoutingWmsFrame(owner, 0, 0, 100, 100, true) == false);
  return recipient;
}

int main() {
  const auto a = CreateInputRoutingState();
  const auto b = CreateInputRoutingState();
  const auto a_old = Publish(a, 6101);
  const auto b_current = Publish(b, 6102);

  DarwinArtKeyEventV1 key{};
  assert(SetInputRoutingFocus(a, a_old->id));
  InputRoutingAdmission legacy_key;
  assert(RouteFrameworkKeyPacket(key, &legacy_key) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  assert(legacy_key.legacy_key_focus && !legacy_key.focus_ready);
  const auto legacy_generation = legacy_key.generation;

  auto result = ApplyInputRoutingFocusControl(a_old, FocusControl{8, true});
  assert(result.Accepted() && result.ShouldNotify());
  assert(SnapshotInputRoutingSelection(a).generation == legacy_generation);
  InputRoutingInflightLease stale_legacy;
  assert(!ReserveInputRoutingPacket(std::move(legacy_key), true, &stale_legacy));
  InputRoutingAdmission pending_key;
  assert(RouteFrameworkKeyPacket(key, &pending_key) ==
         darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel);
  assert(CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));
  assert(SnapshotInputRoutingSelection(a).focus_ready);
  InputRoutingAdmission key_admission;
  assert(RouteFrameworkKeyPacket(key, &key_admission) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  assert(key_admission.state == a && key_admission.focus_ready &&
         key_admission.focus_epoch == 8);
  const auto duplicate = ApplyInputRoutingFocusControl(a_old, FocusControl{8, true});
  assert(duplicate.Accepted() && !duplicate.ShouldNotify());
  assert(ReplayCachedInputRoutingFocus(a_old).Accepted());
  assert(SnapshotInputRoutingSelection(a).focus_ready);

  // A same-epoch gain on another channel conflicts, while an older gain is
  // stale. Neither mutates the previously authoritative recipient.
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{8, true});
  assert(result.decision == InputFocusEpochEvaluator::Decision::kRejectedConflictingGain);
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{7, true});
  assert(result.decision == InputFocusEpochEvaluator::Decision::kRejectedStaleGain);

  // Replacement retains the channel's cached grant but requires an exact
  // recipient replay; the predecessor cannot apply a late control.
  auto a_new = PrepareInputRoutingRecipient(a, 6103);
  assert(PublishInputRoutingRecipient(a_new).Published());
  result = ApplyInputRoutingFocusControl(a_old, FocusControl{9, false});
  assert(result.decision == InputFocusEpochEvaluator::Decision::kRejectedNotCurrentRecipient);
  result = ReplayCachedInputRoutingFocus(a_new);
  assert(result.Accepted() && result.ShouldNotify());
  assert(result.control.epoch == 8 && result.control.focused);
  assert(CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));

  // Explicit revocation is a cache barrier: replay cannot resurrect an old
  // grant, but a later exact loss remains authoritative.
  assert(RevokeInputRoutingFocus(a_new, 10));
  assert(!RevokeInputRoutingFocus(a_new, 10));
  result = ReplayCachedInputRoutingFocus(a_new);
  assert(result.decision == InputFocusEpochEvaluator::Decision::kRejectedNoCachedGrant);
  result = ApplyInputRoutingFocusControl(a_new, FocusControl{11, false});
  assert(result.Accepted() && result.ShouldNotify());
  assert(CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));
  assert(!SnapshotInputRoutingSelection(a).focus_ready);
  key_admission = {};
  assert(RouteFrameworkKeyPacket(key, &key_admission) ==
         darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel);

  // Preparation fails before the focus/domain cache commit. The old channel
  // remains focused and the failed successor has no cache mutation.
  const auto c = CreateInputRoutingState();
  const auto c_current = Publish(c, 6104);
  result = ApplyInputRoutingFocusControl(a_new, FocusControl{20, true});
  assert(result.Accepted());
  assert(CommitInputRoutingFocusNotification(result));
  const auto before_c = SnapshotInputRoutingSelection(c);
  bool preparation_threw = false;
  fail_allocations = true;
  try {
    (void)ApplyInputRoutingFocusControl(c_current, FocusControl{21, true});
  } catch (const std::bad_alloc&) {
    preparation_threw = true;
  }
  fail_allocations = false;
  allocations_before_failure = 0;
  assert(preparation_threw);
  {
    auto domain = LockInputRoutingDomain();
    assert(domain.Focused() == a && domain.FocusEpochDomain().highest_observed_epoch == 20);
  }
  const auto after_c = SnapshotInputRoutingSelection(c);
  assert(after_c.generation == before_c.generation &&
         after_c.focus_order == before_c.focus_order &&
         after_c.recipient == before_c.recipient);

  // Reentrant post-JNI commit rejects a replaced recipient.
  result = ApplyInputRoutingFocusControl(a_new, FocusControl{30, true});
  assert(result.Accepted());
  auto a_latest = PrepareInputRoutingRecipient(a, 6105);
  assert(PublishInputRoutingRecipient(a_latest).Published());
  assert(!CommitInputRoutingFocusNotification(result));

  result = ApplyInputRoutingFocusControl(a_latest, FocusControl{40, true});
  assert(result.Accepted());
  const auto a_notification = result;
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{41, true});
  assert(result.Accepted());
  // Another channel's newer gain does not invalidate the fact that Java was
  // invoked for this exact recipient; only this channel's cache revision can
  // make that callback stale.
  assert(CommitInputRoutingFocusNotification(a_notification));
  // The invoked marker is independent, but successful readiness also needs
  // the current domain provenance; B's newer epoch makes A ineligible.
  assert(!CommitInputRoutingFocusReadiness(a_notification, true));
  assert(CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));

  result = ApplyInputRoutingFocusControl(a_latest, FocusControl{40, false});
  assert(result.Accepted() && result.ShouldNotify());
  assert(CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));

  // A false callback can reenter with a newer false control on the same
  // recipient; the older callback must not regress notification freshness.
  const auto old_loss = ApplyInputRoutingFocusControl(b_current,
                                                       FocusControl{42, false});
  assert(old_loss.Accepted() && old_loss.ShouldNotify());
  const auto new_loss = ApplyInputRoutingFocusControl(b_current,
                                                       FocusControl{43, false});
  assert(new_loss.Accepted() && new_loss.ShouldNotify());
  assert(!CommitInputRoutingFocusNotification(old_loss));
  assert(CommitInputRoutingFocusNotification(new_loss));
  assert(!CommitInputRoutingFocusReadiness(old_loss, true));
  assert(CommitInputRoutingFocusReadiness(new_loss, true));

  // Atomic control commit sees a key enqueued after BeforeControl's earlier
  // empty observation. The control stays deferred; the legitimate FIFO key
  // is drained, never purged to fabricate a focus transition.
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{80, true});
  assert(result.Accepted() && CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, true));
  InputRoutingAdmission earlier_key;
  assert(RouteFrameworkKeyPacket(key, &earlier_key) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  InputRoutingInflightLease earlier;
  assert(ReserveInputRoutingPacket(std::move(earlier_key), true, &earlier));
  assert(AcquireInputRoutingHead(b, &earlier) && BeginInputRoutingTransportSend(earlier));
  assert(CompleteInputRoutingPacketWithStatus(std::move(earlier),
             InputRoutingDeliveryResult::kAccepted) == InputRoutingPacketCompletionStatus::kAccepted);
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{81, true});
  assert(result.decision == InputFocusEpochEvaluator::Decision::kDeferredPendingLocalInput);
  darwin_art::DarwinArtInputPacket drained;
  assert(DequeueInputRoutingPacket(b, &drained, b_current->id));
  assert(drained.kind == darwin_art::DarwinArtInputPacketKind::kKey);
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{81, true});
  assert(result.Accepted() && CommitInputRoutingFocusNotification(result));
  assert(CommitInputRoutingFocusReadiness(result, false));
  assert(RouteFrameworkKeyPacket(key, &pending_key) ==
         darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel);
  assert(!ReplayCachedInputRoutingFocus(b_current).ShouldNotify());
  assert(!SnapshotInputRoutingSelection(b).focus_ready);

  // Newer same-channel epochs do not advance routing generation. Both a
  // saved proposal and a send-admitted but not-yet-enqueued local action must
  // retire honestly instead of reporting retained queue backpressure.
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{82, true});
  assert(CommitInputRoutingFocusNotification(result) &&
         CommitInputRoutingFocusReadiness(result, true));
  InputRoutingAdmission saved;
  assert(RouteFrameworkKeyPacket(key, &saved) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  result = ApplyInputRoutingFocusControl(b_current, FocusControl{83, true});
  assert(!ReserveInputRoutingPacket(std::move(saved), true, &earlier));
  assert(CommitInputRoutingFocusNotification(result) &&
         CommitInputRoutingFocusReadiness(result, true));
  assert(RouteFrameworkKeyPacket(key, &saved) ==
         darwin_art::DarwinArtInputEnqueueResult::kQueued);
  assert(ReserveInputRoutingPacket(std::move(saved), true, &earlier));
  assert(AcquireInputRoutingHead(b, &earlier) && BeginInputRoutingTransportSend(earlier));
  assert(ApplyInputRoutingFocusControl(b_current, FocusControl{84, true}).Accepted());
  assert(CompleteInputRoutingPacketWithStatus(std::move(earlier),
             InputRoutingDeliveryResult::kAccepted) == InputRoutingPacketCompletionStatus::kTerminal);
  assert(!HasInputRoutingPackets(b));
  assert(SetInputRoutingFocus(a, a_latest->id));
  assert(RouteFrameworkKeyPacket(key, &pending_key) ==
         darwin_art::DarwinArtInputEnqueueResult::kNoFocusedChannel);

  std::puts("input-routing-focus: PASS delivery barrier/legacy fence/FIFO deferral/typed stale completion");
}
