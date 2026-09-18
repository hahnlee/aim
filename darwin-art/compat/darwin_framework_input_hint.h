#pragma once

#include "darwin_surface_bridge.h"

namespace darwin_art {

enum class DarwinArtInputPacketKind : uint32_t {
  kPointer = 1,
  kKey = 2,
};

struct DarwinArtInputPacket {
  DarwinArtInputPacketKind kind = DarwinArtInputPacketKind::kPointer;
  DarwinArtPointerEventV2 pointer{};
  DarwinArtKeyEventV1 key{};
};

enum class DarwinArtInputEnqueueResult : uint32_t {
  kNoFocusedChannel = 0,
  kQueued = 1,
  kBackpressured = 2,
};

// Payload bridge used by the explicit production Android input sink. The
// channel implementation owns bounded storage; no surface mailbox or replay
// consumer exists when no focused channel is available.
DarwinArtInputEnqueueResult EnqueueFrameworkPointerPacket(
    const DarwinArtPointerEventV2& packet);

}  // namespace darwin_art
