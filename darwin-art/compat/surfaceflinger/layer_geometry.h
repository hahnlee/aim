#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace darwin_art::surfaceflinger {

struct GeometryRect {
  int32_t left = 0;
  int32_t top = 0;
  int32_t right = 0;
  int32_t bottom = 0;

  bool valid() const { return right > left && bottom > top; }
};

struct LayerGeometryState {
  uint32_t parent_id = 0;
  int32_t position_x = 0;
  int32_t position_y = 0;
  float scale_x = 1.0f;
  float scale_y = 1.0f;
  float alpha = 1.0f;
  bool hidden = false;
  bool has_crop = false;
  GeometryRect crop{};
};

struct ResolvedLayerGeometry {
  bool visible = false;
  GeometryRect destination{};
  bool has_clip = false;
  GeometryRect clip{};
  float inherited_alpha = 1.0f;
};

namespace detail {

struct AxisTransform {
  double scale_x = 1.0;
  double scale_y = 1.0;
  double translate_x = 0.0;
  double translate_y = 0.0;
};

inline int32_t RoundCoordinate(double value) {
  constexpr double kIntMin = static_cast<double>(INT32_MIN);
  constexpr double kIntMax = static_cast<double>(INT32_MAX);
  return static_cast<int32_t>(std::llround(std::clamp(value, kIntMin, kIntMax)));
}

inline GeometryRect MapRect(const GeometryRect& rect,
                            const AxisTransform& transform) {
  const double left = rect.left * transform.scale_x + transform.translate_x;
  const double right = rect.right * transform.scale_x + transform.translate_x;
  const double top = rect.top * transform.scale_y + transform.translate_y;
  const double bottom = rect.bottom * transform.scale_y + transform.translate_y;
  return {
      .left = RoundCoordinate(std::min(left, right)),
      .top = RoundCoordinate(std::min(top, bottom)),
      .right = RoundCoordinate(std::max(left, right)),
      .bottom = RoundCoordinate(std::max(top, bottom)),
  };
}

inline GeometryRect Intersect(const GeometryRect& first,
                              const GeometryRect& second) {
  return {
      .left = std::max(first.left, second.left),
      .top = std::max(first.top, second.top),
      .right = std::min(first.right, second.right),
      .bottom = std::min(first.bottom, second.bottom),
  };
}

}  // namespace detail

// Resolves the Android SurfaceControl parent contract for one buffered layer.
// `local_destination` is already the buffered layer's own destination frame;
// only structural ancestors are applied here. Display/Retina projection is a
// separate output step and must happen after this function.
inline ResolvedLayerGeometry ResolveLayerGeometry(
    uint32_t layer_id,
    const std::unordered_map<uint32_t, LayerGeometryState>& layers,
    const GeometryRect& local_destination) {
  ResolvedLayerGeometry result{};
  if (layer_id == 0 || !local_destination.valid()) return result;
  const auto layer = layers.find(layer_id);
  if (layer == layers.end() || layer->second.hidden) return result;

  std::vector<const LayerGeometryState*> ancestors;
  std::unordered_set<uint32_t> visited{layer_id};
  uint32_t current = layer->second.parent_id;
  while (current != 0) {
    if (!visited.insert(current).second) return result;
    const auto found = layers.find(current);
    if (found == layers.end()) return result;
    ancestors.push_back(&found->second);
    current = found->second.parent_id;
  }

  detail::AxisTransform world{};
  for (auto iterator = ancestors.rbegin(); iterator != ancestors.rend();
       ++iterator) {
    const LayerGeometryState& ancestor = **iterator;
    if (ancestor.hidden || ancestor.scale_x == 0.0f ||
        ancestor.scale_y == 0.0f) {
      return result;
    }
    world.translate_x += world.scale_x * ancestor.position_x;
    world.translate_y += world.scale_y * ancestor.position_y;
    world.scale_x *= ancestor.scale_x;
    world.scale_y *= ancestor.scale_y;
    result.inherited_alpha *= std::clamp(ancestor.alpha, 0.0f, 1.0f);
    if (ancestor.has_crop && ancestor.crop.valid()) {
      const GeometryRect crop = detail::MapRect(ancestor.crop, world);
      result.clip = result.has_clip ? detail::Intersect(result.clip, crop) : crop;
      result.has_clip = true;
      if (!result.clip.valid()) return ResolvedLayerGeometry{};
    }
  }

  result.destination = detail::MapRect(local_destination, world);
  if (!result.destination.valid()) return ResolvedLayerGeometry{};
  if (result.has_clip &&
      !detail::Intersect(result.destination, result.clip).valid()) {
    return ResolvedLayerGeometry{};
  }
  result.visible = result.inherited_alpha > 0.0f;
  return result;
}

}  // namespace darwin_art::surfaceflinger
