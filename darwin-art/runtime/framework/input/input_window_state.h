#pragma once
#include <cstdint>

namespace darwin_art::input {
struct InputWindowFrame {
  int32_t left = 0, top = 0, right = 0, bottom = 0;
  bool Valid() const { return right > left && bottom > top; }
};
struct InputWindowEligibilityTransition {
  bool was_eligible;
  bool is_eligible;
  bool Revoked() const { return was_eligible && !is_eligible; }
};
// Routing serializes this value with its own mutex. No receiver, JNI, focus,
// generation or CANCEL ownership belongs here. WMS is the eligibility owner.
class InputWindowState final {
 public:
  InputWindowEligibilityTransition PublishWmsFrame(InputWindowFrame frame, bool visible) {
    const bool previous = eligible_;
    frame_ = frame;
    wms_published_ = true;
    eligible_ = visible && frame.Valid();
    return {previous, eligible_};
  }
  bool UpdateReceiverFrame(InputWindowFrame frame) {
    if (wms_published_) return false;
    frame_ = frame;
    return true; // Coordinates alone never enable routing.
  }
  bool Eligible() const { return eligible_; }
  const InputWindowFrame& Frame() const { return frame_; }
 private:
  InputWindowFrame frame_;
  bool wms_published_ = false;
  bool eligible_ = false;
};
}
