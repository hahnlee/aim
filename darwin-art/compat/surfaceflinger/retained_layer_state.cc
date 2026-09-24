#include "retained_layer_state.h"

#include "transaction_bridge.h"

#include <algorithm>

namespace darwin_art::surfaceflinger {

uint32_t CopyTransparentRegion(const DarwinArtTransparentRegionRect* source,
                               uint32_t source_count,
                               DarwinArtTransparentRegionRect* destination) {
  if (source == nullptr || destination == nullptr) return 0;
  const uint32_t bounded_count = std::min(
      source_count, static_cast<uint32_t>(kDarwinArtMaxTransparentRegionRects));
  uint32_t copied = 0;
  for (uint32_t index = 0; index < bounded_count; ++index) {
    const auto& rect = source[index];
    if (rect.right <= rect.left || rect.bottom <= rect.top) continue;
    destination[copied++] = rect;
  }
  return copied;
}

void MergeRetainedLayer(WireLayer& destination, const WireLayer& source) {
  destination.owner_process_id = source.owner_process_id;
  destination.layer_id = source.layer_id;
  destination.what = source.what;
  if ((source.what & DARWIN_ART_SF_REPARENT) != 0) {
    destination.parent_owner_process_id = source.parent_owner_process_id;
    destination.parent_id = source.parent_id;
  }
  if ((source.what & DARWIN_ART_SF_RELATIVE_LAYER_CHANGED) != 0) {
    destination.relative_parent_owner_process_id =
        source.relative_parent_owner_process_id;
    destination.relative_parent_id = source.relative_parent_id;
  }
  if ((source.what & DARWIN_ART_SF_LAYER_CHANGED) != 0) {
    destination.relative_parent_owner_process_id = 0;
    destination.relative_parent_id = 0;
  }
  if ((source.what & DARWIN_ART_SF_FLAGS_CHANGED) != 0) {
    destination.flags =
        (destination.flags & ~source.mask) | (source.flags & source.mask);
  }
  if ((source.what & DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED) != 0)
    destination.transform = source.transform;
  if ((source.what & DARWIN_ART_SF_MATRIX_CHANGED) != 0) {
    destination.scale_x = source.scale_x;
    destination.scale_y = source.scale_y;
  }
  if ((source.what & DARWIN_ART_SF_LAYER_CHANGED) != 0)
    destination.z = source.z;
  if ((source.what & DARWIN_ART_SF_ALPHA_CHANGED) != 0)
    destination.alpha = source.alpha;
  if ((source.what & DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED) != 0) {
    destination.transparent_region_count = CopyTransparentRegion(
        source.transparent_region, source.transparent_region_count,
        destination.transparent_region);
  }
  if ((source.what & DARWIN_ART_SF_BUFFER_CHANGED) != 0) {
    destination.iosurface_id = source.iosurface_id;
    destination.width = source.width;
    destination.height = source.height;
    destination.producer_bottom_left = source.producer_bottom_left;
    destination.source_left = source.source_left;
    destination.source_top = source.source_top;
    destination.source_right = source.source_right;
    destination.source_bottom = source.source_bottom;
    if ((source.what & DARWIN_ART_SF_BUFFER_DEFINES_BOUNDS) != 0 ||
        destination.destination_right <= destination.destination_left ||
        destination.destination_bottom <= destination.destination_top) {
      destination.destination_right = destination.destination_left +
          static_cast<int32_t>(source.width);
      destination.destination_bottom = destination.destination_top +
          static_cast<int32_t>(source.height);
    }
  }
  if ((source.what & DARWIN_ART_SF_POSITION_CHANGED) != 0) {
    const int32_t width = std::max<int32_t>(
        0, destination.destination_right - destination.destination_left);
    const int32_t height = std::max<int32_t>(
        0, destination.destination_bottom - destination.destination_top);
    destination.destination_left = source.destination_left;
    destination.destination_top = source.destination_top;
    destination.destination_right = source.destination_left + width;
    destination.destination_bottom = source.destination_top + height;
    destination.position_x = source.position_x;
    destination.position_y = source.position_y;
  }
  if ((source.what & DARWIN_ART_SF_CROP_CHANGED) != 0) {
    destination.has_crop = source.has_crop;
    destination.crop_left = source.crop_left;
    destination.crop_top = source.crop_top;
    destination.crop_right = source.crop_right;
    destination.crop_bottom = source.crop_bottom;
  }
  if ((source.what & DARWIN_ART_SF_DESTINATION_FRAME_CHANGED) != 0) {
    destination.destination_left = source.destination_left;
    destination.destination_top = source.destination_top;
    destination.destination_right = source.destination_right;
    destination.destination_bottom = source.destination_bottom;
  }
}

}  // namespace darwin_art::surfaceflinger
