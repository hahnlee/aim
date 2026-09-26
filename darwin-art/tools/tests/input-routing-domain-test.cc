// Isolated domain contract test: the channel ledger is a test-only provider.
// Real channel snapshot/admission and FIFO/CANCEL run in input-routing-test.
#include "runtime/framework/input/input_routing_domain.h"
#include <cassert>
#include <cstdio>
#include <type_traits>
#include <atomic>
#include <thread>
#include <stdexcept>
#include <unistd.h>
#include <vector>

namespace darwin_art::input {
struct InputRoutingState { InputRoutingSelectionSnapshot selection; };
InputRoutingSelectionSnapshot SnapshotInputRoutingSelection(const InputRoutingHandle& channel) {
  return channel ? channel->selection : InputRoutingSelectionSnapshot{};
}
bool ValidateInputRoutingSelection(const InputRoutingHandle& channel,
    const InputRoutingSelectionSnapshot& snapshot, InputRoutingAdmission* admission) {
  if (!channel || !admission || !snapshot.eligible ||
      snapshot.generation != channel->selection.generation) return false;
  admission->state = channel;
  admission->generation = snapshot.generation;
  admission->consumer_id = snapshot.consumer_id;
  return true;
}
static InputRoutingHandle Channel(uint64_t order, InputWindowFrame frame) {
  auto channel = std::make_shared<InputRoutingState>();
  channel->selection.eligible = true;
  channel->selection.generation = 1;
  channel->selection.consumer_id = order;
  channel->selection.recipient = std::make_shared<InputRoutingRecipient>(channel, order,
      InputRoutingEndpointHandle{});
  channel->selection.focus_order = order;
  channel->selection.frame = frame;
  auto domain = LockInputRoutingDomain();
  domain.Register(channel);
  return channel;
}
}
using namespace darwin_art::input;
using darwin_art::DarwinArtInputEnqueueResult;
static_assert(!std::is_move_constructible_v<InputRoutingDomainTransaction>);
static_assert(!std::is_copy_constructible_v<InputRoutingDomainTransaction>);
static DarwinArtPointerEventV2 Pointer(uint32_t action, float x, float y) {
  DarwinArtPointerEventV2 packet{};
  packet.action = action; packet.x = x; packet.y = y;
  return packet;
}
static void ReentrantLastOwner(int promotion, bool unwind) {
  std::atomic<bool> start{false}, released{false}, reentered{false};
  auto channel = InputRoutingHandle(new InputRoutingState, [&](InputRoutingState* raw) {
    // Represents a final endpoint/transport close callback, not a fake app.
    auto domain = LockInputRoutingDomain();
    (void)domain.Focused();
    reentered.store(true);
    delete raw;
  });
  const auto weak = std::weak_ptr<InputRoutingState>(channel);
  {
    auto domain = LockInputRoutingDomain();
    domain.Register(channel);
    if (promotion == 1) domain.SetFocused(channel);
    if (promotion == 2) {
      InputRoutingAdmission accepted;
      accepted.state = channel; accepted.generation = 1; accepted.pointer_down = true;
      domain.CommitAcceptedPointer(accepted);
    }
  }
  std::thread owner([pin = std::move(channel), &start, &released]() mutable {
    while (!start.load()) std::this_thread::yield();
    pin.reset(); released.store(true);
  });
  try {
    auto domain = LockInputRoutingDomain();
    auto candidates = promotion == 0 ? domain.Candidates() : std::vector<InputRoutingHandle>{};
    if (promotion == 1) (void)domain.Focused(); // Temporary strong pin.
    if (promotion == 2) (void)domain.Captured();
    start.store(true);
    while (!released.load()) std::this_thread::yield();
    candidates.clear();
    assert(!weak.expired() && !reentered.load());
    if (unwind) throw std::runtime_error("test exception unwind");
  } catch (const std::runtime_error&) { assert(unwind); }
  owner.join();
  assert(weak.expired() && reentered.load());
}
int main() {
  // A lock-order regression must terminate rather than hang the build runner.
  alarm(10);
  auto lower = Channel(1, {0, 0, 100, 100});
  auto upper = Channel(2, {20, 30, 120, 130});
  InputRoutingAdmission admission;
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 50, 50), &admission) == DarwinArtInputEnqueueResult::kQueued);
  assert(admission.state == upper && admission.packet.pointer.x == 30 && admission.packet.pointer.y == 20);
  // Selection and failed DOWN do not commit capture.
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 500, 500), &admission) == DarwinArtInputEnqueueResult::kNoFocusedChannel);
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 50, 50), &admission) == DarwinArtInputEnqueueResult::kQueued);
  { auto domain = LockInputRoutingDomain(); domain.CommitAcceptedPointer(admission); }
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 500, 500), &admission) == DarwinArtInputEnqueueResult::kQueued);
  assert(admission.state == upper && admission.packet.pointer.x == 480 && admission.packet.pointer.y == 470);
  // Stale UP cannot clear capture of another epoch.
  InputRoutingAdmission stale_up;
  stale_up.state = admission.state; stale_up.pointer_end = true; stale_up.generation = 2;
  { auto domain = LockInputRoutingDomain(); domain.CommitAcceptedPointer(stale_up); }
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 500, 500), &admission) == DarwinArtInputEnqueueResult::kQueued);
  ++upper->selection.generation;
  assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_MOVE, 500, 500), &admission) == DarwinArtInputEnqueueResult::kNoFocusedChannel);
  { auto domain = LockInputRoutingDomain(); domain.ClearCapture(upper); domain.SetFocused(lower); }
  // Model the authoritative receiver callback having completed successfully;
  // This isolated ledger fixture supplies the exact delivery identity; the
  // actual focus owner and JNI callback are exercised in separate suites.
  lower->selection.focus_ready = true;
  lower->selection.focus_grant = true;
  lower->selection.focus_revoked = false;
  lower->selection.focus_epoch = 1;
  lower->selection.focus_cache_revision = 0;
  lower->selection.focus_recipient = lower->selection.recipient;
  lower->selection.focus_ready_recipient = lower->selection.recipient;
  lower->selection.focus_ready_epoch = 1;
  lower->selection.focus_ready_cache_revision = 0;
  lower->selection.authoritative_focus = true;
  { auto domain = LockInputRoutingDomain();
    domain.FocusEpochDomain().highest_observed_epoch = 1;
    domain.FocusEpochDomain().gain_provenance_present = true;
    domain.FocusEpochDomain().gain_channel = lower;
  }
  DarwinArtKeyEventV1 key{};
  assert(RouteFrameworkKeyPacket(key, &admission) == DarwinArtInputEnqueueResult::kQueued && admission.state == lower);
  // The domain retains channels weakly; candidate snapshots pin only temporarily.
  admission = {}; stale_up = {};
  std::weak_ptr<InputRoutingState> weak = lower;
  lower.reset();
  assert(weak.expired());
  { auto domain = LockInputRoutingDomain(); assert(!domain.Focused()); assert(domain.Candidates().size() == 1); }
  for (int promotion = 0; promotion < 3; ++promotion) {
    ReentrantLastOwner(promotion, false);
    ReentrantLastOwner(promotion, true);
  }
  // InputDispatcher window policy: WMS layer before focus recency, touch
  // modal windows take outside DOWNs, watchers above the touched window get
  // ACTION_OUTSIDE in their own frame, not-touchable windows are skipped.
  upper.reset();
  {
    const uint32_t popup_layer = 11000u << kInputWindowLayerShift;
    auto base = Channel(10, {0, 0, 360, 640});
    base->selection.input_flags = kInputWindowTouchModal;
    auto popup = Channel(0, {100, 100, 200, 200});
    popup->selection.input_flags = popup_layer | kInputWindowWatchOutsideTouch;
    std::vector<InputRoutingAdmission> outside;
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 150, 150), &admission,
                                       &outside) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == popup && outside.empty());
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 10, 10), &admission,
                                       &outside) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == base && outside.size() == 1 && outside[0].state == popup);
    assert(outside[0].packet.pointer.action == DARWIN_ART_POINTER_OUTSIDE);
    assert(outside[0].packet.pointer.x == -90 && outside[0].packet.pointer.y == -90);
    assert(!outside[0].pointer_down && !outside[0].pointer_end);
    auto modal = Channel(0, {100, 100, 200, 200});
    modal->selection.input_flags = popup_layer | kInputWindowTouchModal;
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 10, 10), &admission,
                                       &outside) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == modal && outside.empty());
    assert(admission.packet.pointer.x == -90 && admission.packet.pointer.y == -90);
    modal->selection.input_flags |= kInputWindowNotTouchable;
    assert(RouteFrameworkPointerPacket(Pointer(DARWIN_ART_POINTER_DOWN, 150, 150), &admission,
                                       &outside) == DarwinArtInputEnqueueResult::kQueued);
    assert(admission.state == popup);
    admission = {};
    outside.clear();
  }
  alarm(0);
  std::puts("input-routing-domain: PASS overlap/offset/accepted-capture/stale-epoch/weak-lifetime/reentrant-final-close-unwind/window-policy");
}
