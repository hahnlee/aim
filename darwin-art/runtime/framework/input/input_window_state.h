#pragma once
#include <cstdint>

namespace darwin_art::input {
struct InputWindowFrame {
  int32_t left = 0, top = 0, right = 0, bottom = 0;
  bool Valid() const { return right > left && bottom > top; }
};
// A window's input policy as WMS publishes it (InputDispatcher WindowInfo):
// kTouchModal takes touches outside its frame, kWatchOutsideTouch receives
// ACTION_OUTSIDE for a DOWN another window takes, kNotTouchable takes none.
// Bits 16-31 carry the WMS window layer; among equal layers the more recently
// focused, then the later window is above.
enum InputWindowFlags : uint32_t {
  kInputWindowTouchModal = 1u << 0,
  kInputWindowWatchOutsideTouch = 1u << 1,
  kInputWindowNotTouchable = 1u << 2,
};
constexpr uint32_t kInputWindowLayerShift = 16;
inline uint32_t InputWindowLayer(uint32_t flags) { return flags >> kInputWindowLayerShift; }
struct InputWindowEligibilityTransition {
  bool was_eligible;
  bool is_eligible;
  bool Revoked() const { return was_eligible && !is_eligible; }
};
// Routing serializes this value with its own mutex. No receiver, JNI, focus,
// generation or CANCEL ownership belongs here. WMS is the eligibility owner.
class InputWindowState final {
 public:
  InputWindowEligibilityTransition PublishWmsFrame(InputWindowFrame frame, bool visible,
                                                  uint32_t flags = 0) {
    const bool previous = eligible_;
    frame_ = frame;
    flags_ = flags;
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
  uint32_t Flags() const { return flags_; }
 private:
  InputWindowFrame frame_;
  uint32_t flags_ = 0;
  bool wms_published_ = false;
  bool eligible_ = false;
};
}
