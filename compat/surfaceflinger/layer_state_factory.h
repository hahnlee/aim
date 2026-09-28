#pragma once

#include <cstdint>
#include <memory>

namespace android::surfaceflinger::frontend {

struct RequestedLayerState;

std::unique_ptr<RequestedLayerState> makeDarwinRequestedLayerState(
    uint32_t layerId, uint32_t parentId);

}  // namespace android::surfaceflinger::frontend
