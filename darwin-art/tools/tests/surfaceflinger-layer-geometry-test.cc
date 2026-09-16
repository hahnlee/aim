#include "../../compat/surfaceflinger/layer_geometry.h"

#include <cassert>
#include <cstdio>
#include <unordered_map>

using darwin_art::surfaceflinger::GeometryRect;
using darwin_art::surfaceflinger::LayerGeometryState;
using darwin_art::surfaceflinger::ResolveLayerGeometry;

int main() {
  std::unordered_map<uint32_t, LayerGeometryState> layers{
      {1, {.parent_id = 0, .position_x = 10, .position_y = 20,
           .scale_x = 2.0f, .scale_y = 2.0f, .alpha = 0.5f,
           .has_crop = true, .crop = {0, 0, 100, 200}}},
      {2, {.parent_id = 1, .position_x = 5, .position_y = 7,
           .scale_x = 1.0f, .scale_y = 1.0f, .alpha = 0.5f}},
      {3, {.parent_id = 2}},
  };
  const auto resolved = ResolveLayerGeometry(3, layers, {0, 0, 720, 1280});
  assert(resolved.visible);
  assert(resolved.destination.left == 20);
  assert(resolved.destination.top == 34);
  assert(resolved.destination.right == 1460);
  assert(resolved.destination.bottom == 2594);
  assert(resolved.has_clip);
  assert(resolved.clip.left == 10);
  assert(resolved.clip.top == 20);
  assert(resolved.clip.right == 210);
  assert(resolved.clip.bottom == 420);
  assert(resolved.inherited_alpha == 0.25f);

  layers[2].hidden = true;
  assert(!ResolveLayerGeometry(3, layers, {0, 0, 10, 10}).visible);
  layers[2].hidden = false;
  layers[1].parent_id = 3;
  assert(!ResolveLayerGeometry(3, layers, {0, 0, 10, 10}).visible);

  std::puts("surfaceflinger-layer-geometry: PASS");
}
