#pragma once

#include "darwin_surface_bridge.h"

namespace aim {

enum class AimInputPacketKind : uint32_t {
  kPointer = 1,
  kKey = 2,
};

struct AimInputPacket {
  AimInputPacketKind kind = AimInputPacketKind::kPointer;
  AimPointerEventV2 pointer{};
  AimKeyEventV1 key{};
};

enum class AimInputEnqueueResult : uint32_t {
  kNoFocusedChannel = 0,
  kQueued = 1,
  kBackpressured = 2,
};

// Payload bridge used by the explicit production Android input sink. The
// channel implementation owns bounded storage; no surface mailbox or replay
// consumer exists when no focused channel is available.
AimInputEnqueueResult EnqueueFrameworkPointerPacket(
    const AimPointerEventV2& packet);

}  // namespace aim
