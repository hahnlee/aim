#include "input_routing_domain.h"
#include <algorithm>
#include <stdexcept>

namespace darwin_art::input {
struct InputRoutingDomainState {
  std::mutex mutex;
  std::vector<std::weak_ptr<InputRoutingState>> candidates;
  std::weak_ptr<InputRoutingState> focused;
  std::weak_ptr<InputRoutingState> captured;
  uint64_t capture_generation = 0;
  int32_t capture_offset_x = 0, capture_offset_y = 0;
  uint64_t next_focus_order = 1;
  InputFocusEpochEvaluator::DomainRecord focus_epoch;
};
namespace { InputRoutingDomainState g_domain; }
InputRoutingDomainTransaction::InputRoutingDomainTransaction(InputRoutingDomainState& state)
    : state_(&state), lock_(state.mutex) {}
InputRoutingDomainTransaction::~InputRoutingDomainTransaction() {
  // pins_ is destroyed only after this body. Transport close/deleter callbacks
  // must be allowed to acquire the domain again, even during exception unwind.
  lock_.unlock();
}
InputRoutingHandle InputRoutingDomainTransaction::Pin(
    const std::weak_ptr<InputRoutingState>& weak) const {
  for (auto& pin : inline_pins_) {
    if (!pin) { pin = weak.lock(); return pin; }
  }
  // Allocate BEFORE promotion: failure cannot destroy a newly promoted last
  // owner under the domain lock. Existing pins unwind after explicit unlock.
  if (pins_.size() == pins_.capacity()) {
    if (pins_.size() == pins_.max_size()) throw std::length_error("domain pins");
    pins_.reserve(pins_.size() + 1);
  }
  auto channel = weak.lock();
  if (channel) pins_.push_back(channel);
  return channel;
}
InputRoutingDomainTransaction LockInputRoutingDomain() {
  return InputRoutingDomainTransaction(g_domain);
}
InputRoutingHandle InputRoutingDomainTransaction::Focused() const {
  return Pin(state_->focused);
}
InputRoutingCaptureSnapshot InputRoutingDomainTransaction::Captured() const {
  return {Pin(state_->captured), state_->capture_generation,
          state_->capture_offset_x, state_->capture_offset_y};
}
void InputRoutingDomainTransaction::SetFocused(const InputRoutingHandle& channel) {
  state_->focused = channel;
}
void InputRoutingDomainTransaction::ClearFocus(const InputRoutingHandle& channel) {
  if (Pin(state_->focused) == channel) state_->focused.reset();
}
void InputRoutingDomainTransaction::ClearCapture(const InputRoutingHandle& channel,
                                               uint64_t generation) {
  if (Pin(state_->captured) != channel ||
      (generation != 0 && generation != state_->capture_generation)) return;
  state_->captured.reset();
  state_->capture_generation = 0;
  state_->capture_offset_x = state_->capture_offset_y = 0;
}
void InputRoutingDomainTransaction::CommitAcceptedPointer(const InputRoutingAdmission& admission) {
  if (admission.pointer_down) {
    state_->captured = admission.state;
    state_->capture_generation = admission.generation;
    state_->capture_offset_x = admission.offset_x;
    state_->capture_offset_y = admission.offset_y;
  } else if (admission.pointer_end) {
    ClearCapture(admission.state, admission.generation);
  }
}
InputFocusEpochEvaluator::DomainRecord&
InputRoutingDomainTransaction::FocusEpochDomain() {
  return state_->focus_epoch;
}
const InputFocusEpochEvaluator::DomainRecord&
InputRoutingDomainTransaction::FocusEpochDomain() const {
  return state_->focus_epoch;
}
uint64_t InputRoutingDomainTransaction::NextFocusOrder() { return state_->next_focus_order++; }
void InputRoutingDomainTransaction::Register(const InputRoutingHandle& channel) {
  std::erase_if(state_->candidates, [](const auto& channel) { return channel.expired(); });
  state_->candidates.emplace_back(channel);
}
std::vector<InputRoutingHandle> InputRoutingDomainTransaction::Candidates() {
  std::erase_if(state_->candidates, [](const auto& channel) { return channel.expired(); });
  std::vector<InputRoutingHandle> channels;
  channels.reserve(state_->candidates.size());
  if (state_->candidates.size() > pins_.max_size() - pins_.size())
    throw std::length_error("domain candidates");
  pins_.reserve(pins_.size() + state_->candidates.size());
  for (const auto& weak : state_->candidates) {
    if (auto channel = weak.lock()) {
      pins_.push_back(channel);
      channels.push_back(std::move(channel));
    }
  }
  return channels;
}

DarwinArtInputEnqueueResult RouteFrameworkPointerPacket(
    const DarwinArtPointerEventV2& packet, InputRoutingAdmission* admission,
    std::vector<InputRoutingAdmission>* outside) {
  if (!admission) return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  *admission = {};
  if (outside) outside->clear();
  auto domain = LockInputRoutingDomain();
  InputRoutingHandle channel;
  InputRoutingSelectionSnapshot selection;
  int32_t offset_x = 0, offset_y = 0;
  if (packet.action == DARWIN_ART_POINTER_DOWN) {
    // InputDispatcher::findTouchedWindowAtLocked: front to back (WMS layer,
    // then the more recently focused, then the later window), the first
    // touchable window that contains the point or is touch modal takes the
    // stream; the watching windows above it receive ACTION_OUTSIDE.
    struct Candidate {
      InputRoutingHandle channel;
      InputRoutingSelectionSnapshot facts;
      size_t added;
    };
    std::vector<Candidate> ordered;
    size_t added = 0;
    for (const auto& candidate : domain.Candidates()) {
      auto facts = SnapshotInputRoutingSelection(candidate);
      if (facts.eligible) ordered.push_back({candidate, std::move(facts), added});
      ++added;
    }
    std::sort(ordered.begin(), ordered.end(), [](const Candidate& a, const Candidate& b) {
      const uint32_t layer_a = InputWindowLayer(a.facts.input_flags);
      const uint32_t layer_b = InputWindowLayer(b.facts.input_flags);
      if (layer_a != layer_b) return layer_a > layer_b;
      if (a.facts.focus_order != b.facts.focus_order)
        return a.facts.focus_order > b.facts.focus_order;
      return a.added > b.added;
    });
    std::vector<const Candidate*> watching;
    for (const auto& candidate : ordered) {
      const uint32_t flags = candidate.facts.input_flags;
      if (flags & kInputWindowNotTouchable) continue;
      const auto& frame = candidate.facts.frame;
      if ((packet.x >= frame.left && packet.y >= frame.top && packet.x < frame.right &&
           packet.y < frame.bottom) || (flags & kInputWindowTouchModal)) {
        channel = candidate.channel;
        selection = candidate.facts;
        break;
      }
      if (flags & kInputWindowWatchOutsideTouch) watching.push_back(&candidate);
    }
    if (!channel) {
      watching.clear();
      channel = domain.Focused();
      selection = SnapshotInputRoutingSelection(channel);
    }
    offset_x = selection.frame.left;
    offset_y = selection.frame.top;
    if (!outside) watching.clear();
    for (const Candidate* watcher : watching) {
      InputRoutingAdmission event;
      if (!ValidateInputRoutingSelection(watcher->channel, watcher->facts, &event)) continue;
      // Same-uid windows keep ACTION_OUTSIDE coordinates, in their frame.
      event.packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
      event.packet.pointer = packet;
      event.packet.pointer.action = DARWIN_ART_POINTER_OUTSIDE;
      event.packet.pointer.x -= static_cast<float>(watcher->facts.frame.left);
      event.packet.pointer.y -= static_cast<float>(watcher->facts.frame.top);
      event.offset_x = watcher->facts.frame.left;
      event.offset_y = watcher->facts.frame.top;
      outside->push_back(std::move(event));
    }
  } else {
    const auto capture = domain.Captured();
    channel = capture.channel;
    selection = SnapshotInputRoutingSelection(channel);
    if (selection.generation != capture.generation)
      return DarwinArtInputEnqueueResult::kNoFocusedChannel;
    offset_x = capture.offset_x;
    offset_y = capture.offset_y;
  }
  if (!ValidateInputRoutingSelection(channel, selection, admission))
    return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  admission->packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
  admission->packet.pointer = packet;
  admission->packet.pointer.x -= static_cast<float>(offset_x);
  admission->packet.pointer.y -= static_cast<float>(offset_y);
  admission->offset_x = offset_x;
  admission->offset_y = offset_y;
  admission->pointer_down = packet.action == DARWIN_ART_POINTER_DOWN;
  admission->pointer_end = packet.action == DARWIN_ART_POINTER_UP || packet.action == DARWIN_ART_POINTER_CANCEL;
  return DarwinArtInputEnqueueResult::kQueued;
}
DarwinArtInputEnqueueResult RouteFrameworkKeyPacket(
    const DarwinArtKeyEventV1& packet, InputRoutingAdmission* admission) {
  if (!admission) return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  *admission = {};
  auto domain = LockInputRoutingDomain();
  const auto channel = domain.Focused();
  const auto selection = SnapshotInputRoutingSelection(channel);
  const bool legacy = InputFocusEpochEvaluator::IsLegacyKeyAdmission(
      domain.FocusEpochDomain(), selection.authoritative_focus);
  if ((!legacy && (!selection.focus_ready ||
      !InputFocusEpochEvaluator::IsFocusReady(
          domain.FocusEpochDomain(), channel, selection.recipient,
          selection.focus_ready, selection.focus_grant, selection.focus_revoked,
          selection.focus_epoch, selection.focus_cache_revision,
          selection.focus_ready_recipient, selection.focus_ready_epoch,
          selection.focus_ready_cache_revision, selection.focus_epoch,
          selection.focus_cache_revision))) ||
      !ValidateInputRoutingSelection(channel, selection, admission))
    return DarwinArtInputEnqueueResult::kNoFocusedChannel;
  admission->focus_ready = !legacy;
  admission->legacy_key_focus = legacy;
  admission->focus_epoch = selection.focus_epoch;
  admission->focus_cache_revision = selection.focus_cache_revision;
  admission->focus_recipient = selection.focus_recipient;
  admission->packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  admission->packet.key = packet;
  return DarwinArtInputEnqueueResult::kQueued;
}
}  // namespace darwin_art::input
