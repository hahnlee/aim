#pragma once

#include <cstddef>
#include <cstdint>
#include <type_traits>

#include "darwin_framework_input_hint.h"

namespace darwin_art::input {

// Transport-level focus is deliberately only a versioned value. Selection,
// admission, and application of this value remain policy-owner concerns.
struct FocusControl {
  uint64_t epoch = 0;
  bool focused = false;
};

namespace transport_wire {

constexpr uint32_t kInputFrameMagic = 0x44414950;  // DAIP
constexpr uint32_t kWindowFrameMagic = 0x44415747;  // DAWG
constexpr uint32_t kAckFrameMagic = 0x4441414b;  // DAAK
constexpr uint32_t kFocusControlFrameMagic = 0x44414643;  // DAFC
constexpr uint32_t kFrameVersion = 1;

struct InputFrame {
  uint32_t magic = kInputFrameMagic;
  uint32_t version = kFrameVersion;
  uint32_t kind = 0;
  uint32_t payload_size = sizeof(DarwinArtInputPacket);
  DarwinArtInputPacket payload{};
};

struct WindowFrame {
  uint32_t magic = kWindowFrameMagic;
  uint32_t version = kFrameVersion;
  int32_t left = 0;
  int32_t top = 0;
  int32_t right = 0;
  int32_t bottom = 0;
  uint32_t visible = 0;
  uint32_t reserved = 0;
};

struct AckFrame {
  uint32_t magic = kAckFrameMagic;
  uint32_t version = kFrameVersion;
  uint32_t sequence = 0;
  uint32_t handled = 0;
};

// ACK v2 identifies the original publisher packet, not the Java dispatch ID.
// Keeping v1 distinct prevents old framework IDs from masquerading as packets.
struct AckFrameV2 {
  uint32_t magic = kAckFrameMagic;
  uint32_t version = 2;
  uint64_t sequence = 0;
  uint32_t handled = 0;
  uint32_t reserved = 0;
};
static_assert(sizeof(AckFrame) == 16);
static_assert(sizeof(AckFrameV2) == 24);
static_assert(offsetof(AckFrameV2, sequence) == 8);
static_assert(offsetof(AckFrameV2, handled) == 16);
static_assert(std::is_trivially_copyable_v<AckFrameV2>);

// FocusControl uses a uint32 wire boolean and an explicit reserved word so
// the byte representation never depends on C++ bool layout.
struct FocusControlFrame {
  uint32_t magic = kFocusControlFrameMagic;
  uint32_t version = kFrameVersion;
  uint64_t epoch = 0;
  uint32_t focused = 0;
  uint32_t reserved = 0;
};

static_assert(std::is_trivially_copyable_v<InputFrame>);
static_assert(std::is_trivially_copyable_v<WindowFrame>);
static_assert(std::is_trivially_copyable_v<AckFrame>);
static_assert(std::is_trivially_copyable_v<FocusControlFrame>);
static_assert(sizeof(FocusControlFrame) == 24);

inline FocusControlFrame EncodeFocusControl(FocusControl control) noexcept {
  FocusControlFrame frame{};
  frame.epoch = control.epoch;
  frame.focused = control.focused ? 1u : 0u;
  return frame;
}

inline bool DecodeFocusControl(const FocusControlFrame& frame,
                              FocusControl* control) noexcept {
  if (frame.magic != kFocusControlFrameMagic ||
      frame.version != kFrameVersion || frame.epoch == 0 ||
      frame.focused > 1 || frame.reserved != 0)
    return false;
  if (control != nullptr)
    *control = FocusControl{frame.epoch, frame.focused != 0};
  return true;
}

}  // namespace transport_wire
}  // namespace darwin_art::input
