#include "layer_state_factory.h"

// LayerCreationArgs owns an sp<Client>; Client must be complete in the
// translation unit which destroys it. Keep that Android ownership detail out
// of the pointer-free Darwin transaction bridge.
#include "Client.h"
#include "FrontEnd/LayerCreationArgs.h"
#include "FrontEnd/RequestedLayerState.h"

#include <optional>

namespace android::surfaceflinger::frontend {

std::unique_ptr<RequestedLayerState> makeDarwinRequestedLayerState(
    uint32_t layerId, uint32_t parentId) {
  surfaceflinger::LayerCreationArgs creation{
      std::optional<uint32_t>(layerId)};
  creation.name = "DarwinLayer";
  creation.parentId =
      parentId == 0 ? UNASSIGNED_LAYER_ID : parentId;
  creation.addToRoot = parentId == 0;
  return std::make_unique<RequestedLayerState>(creation);
}

}  // namespace android::surfaceflinger::frontend
