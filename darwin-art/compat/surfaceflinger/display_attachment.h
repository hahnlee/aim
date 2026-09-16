#pragma once

#include <cstdint>
#include <set>
#include <unordered_map>
#include <unordered_set>

namespace darwin_art::surfaceflinger {

// SurfaceFlinger assigns a layer to an output through its current structural
// ancestry. Only roots explicitly registered with an output are anchors;
// previously presented descendants must not retain stale display membership
// after a reparent.
inline bool IsAttachedToDisplayAnchor(
    uint32_t layer_id,
    const std::unordered_map<uint32_t, uint32_t>& parent_by_layer,
    const std::set<uint32_t>& display_anchors) {
  if (layer_id == 0 || display_anchors.empty()) return false;
  std::unordered_set<uint32_t> visited;
  uint32_t current = layer_id;
  while (current != 0) {
    if (!visited.insert(current).second) return false;
    const auto parent = parent_by_layer.find(current);
    if (parent == parent_by_layer.end()) return false;
    if (display_anchors.contains(current)) return parent->second == 0;
    current = parent->second;
  }
  return false;
}

}  // namespace darwin_art::surfaceflinger
