#include "surface_control_state.h"

#include <algorithm>
#include <cstddef>
#include <new>
#include <utility>

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer);
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer);

namespace darwin_art::window {
namespace {

const SurfaceControlStateView* FindControl(
    const std::vector<SurfaceControlStateView>& controls,
    ASurfaceControl* opaque) {
  const auto found = std::find_if(
      controls.begin(), controls.end(),
      [opaque](const SurfaceControlStateView& control) {
        return control.opaque == opaque;
      });
  return found == controls.end() ? nullptr : &*found;
}

const SurfaceControlUpdateView* FindUpdate(
    const std::vector<SurfaceControlUpdateView>& updates,
    ASurfaceControl* opaque) {
  const auto found = std::find_if(
      updates.begin(), updates.end(),
      [opaque](const SurfaceControlUpdateView& update) {
        return update.opaque == opaque;
      });
  return found == updates.end() ? nullptr : &*found;
}

void PopulateTransparentRegion(DarwinArtMetalComposerLayer* state,
                               const std::vector<ARect>& region,
                               bool changed) {
  if (state == nullptr || !changed) return;
  state->what |= DARWIN_ART_SF_TRANSPARENT_REGION_CHANGED;
  state->transparent_region_count = 0;
  const size_t count = std::min(
      region.size(), kDarwinArtMaxTransparentRegionRects);
  for (size_t index = 0; index < count; ++index) {
    const ARect& rect = region[index];
    if (rect.right <= rect.left || rect.bottom <= rect.top) continue;
    state->transparent_region[state->transparent_region_count++] = {
        .left = rect.left,
        .top = rect.top,
        .right = rect.right,
        .bottom = rect.bottom,
    };
  }
}

void SetDamage(SurfaceControlPresentation* presentation,
               const std::vector<ARect>& damage) {
  if (presentation == nullptr || damage.empty()) return;
  ARect bounds = damage.front();
  for (const ARect& rect : damage) {
    bounds.left = std::min(bounds.left, rect.left);
    bounds.top = std::min(bounds.top, rect.top);
    bounds.right = std::max(bounds.right, rect.right);
    bounds.bottom = std::max(bounds.bottom, rect.bottom);
  }
  presentation->has_damage = true;
  presentation->damage = bounds;
}

SurfaceControlPresentation MakePresentation(
    const SurfaceControlStateView& control,
    const SurfaceControlUpdateView* update) {
  AHardwareBuffer_Desc description{};
  AHardwareBuffer_describe(control.buffer, &description);
  const ARect source = control.has_geometry
      ? control.source
      : (control.has_crop
             ? control.crop
             : ARect{0, 0, static_cast<int32_t>(description.width),
                     static_cast<int32_t>(description.height)});
  const ARect destination = control.has_geometry
      ? control.destination
      : ARect{control.position_x,
              control.position_y,
              control.position_x + static_cast<int32_t>(
                  (source.right - source.left) * control.scale_x),
              control.position_y + static_cast<int32_t>(
                  (source.bottom - source.top) * control.scale_y)};
  AHardwareBuffer_acquire(control.buffer);
  SurfaceControlPresentation result{
      .opaque = control.opaque,
      .buffer = control.buffer,
      .owner_process_id = control.owner_process_id,
      .layer_id = control.layer_id,
      .parent_owner_process_id = control.parent_owner_process_id,
      .parent_id = control.parent_id,
      .relative_parent_owner_process_id =
          control.relative_parent_owner_process_id,
      .relative_parent_id = control.relative_parent_id,
      .source = source,
      .destination = destination,
      .explicit_geometry = control.has_geometry,
      .alpha = control.alpha,
      .z_order = control.z_order,
      .name = control.name,
      .transform = control.transform,
      .reparented = update != nullptr && update->has_parent,
  };
  if (update != nullptr && update->has_damage) {
    SetDamage(&result, update->damage);
  }
  return result;
}

}  // namespace

SurfaceControlSnapshot::SurfaceControlSnapshot(
    SurfaceControlSnapshot&& other) noexcept
    : frontend_updates(std::move(other.frontend_updates)),
      control_states(std::move(other.control_states)),
      presentations(std::move(other.presentations)) {}

SurfaceControlSnapshot& SurfaceControlSnapshot::operator=(
    SurfaceControlSnapshot&& other) noexcept {
  if (this == &other) return *this;
  this->~SurfaceControlSnapshot();
  new (this) SurfaceControlSnapshot(std::move(other));
  return *this;
}

SurfaceControlSnapshot::~SurfaceControlSnapshot() {
  for (const auto& presentation : presentations) {
    if (presentation.buffer != nullptr) {
      AHardwareBuffer_release(presentation.buffer);
    }
  }
}

bool ApplySurfaceControlUpdateOverlay(
    SurfaceControlStateView* control,
    const SurfaceControlUpdateView& update) {
  if (control == nullptr || control->opaque != update.opaque) return false;
  if (update.has_buffer) control->buffer = update.buffer;
  if (update.has_position) {
    control->position_x = update.position_x;
    control->position_y = update.position_y;
  }
  if (update.has_z_order) {
    control->z_order = update.z_order;
    // An absolute z-order supersedes a relative layer.  Keep the projected
    // identity fields coherent with the registry's relation mutation pass.
    if (!update.has_relative_layer) {
      control->relative_parent_owner_process_id = 0;
      control->relative_parent_id = 0;
    }
  }
  if (update.has_alpha) control->alpha = update.alpha;
  if (update.has_scale) {
    control->scale_x = update.scale_x;
    control->scale_y = update.scale_y;
  }
  if (update.has_visibility) control->visible = update.visible;
  if (update.has_parent) {
    control->parent_owner_process_id = update.parent_owner_process_id;
    control->parent_id = update.parent_id;
  }
  if (update.has_relative_layer) {
    control->relative_parent_owner_process_id =
        update.relative_parent_owner_process_id;
    control->relative_parent_id = update.relative_parent_id;
    control->z_order = update.z_order;
  }
  if (update.has_transform) control->transform = update.transform;
  if (update.has_crop) {
    control->crop = update.crop;
    control->has_crop = true;
  }
  if (update.has_geometry) {
    control->source = update.source;
    control->destination = update.destination;
    control->has_geometry = true;
  }
  if (update.has_transparent_region)
    control->transparent_region = update.transparent_region;
  return true;
}

bool BuildSurfaceControlFrontendUpdates(
    const std::vector<SurfaceControlStateView>& controls,
    const std::vector<SurfaceControlUpdateView>& updates,
    std::vector<DarwinArtSurfaceFlingerLayerUpdate>* out) {
  if (out == nullptr) return false;
  try {
    std::vector<DarwinArtSurfaceFlingerLayerUpdate> result;
    result.reserve(updates.size());
    for (const auto& update : updates) {
      const auto* control = FindControl(controls, update.opaque);
      if (control == nullptr) continue;
      uint64_t what = 0;
      if (update.has_position) what |= DARWIN_ART_SF_POSITION_CHANGED;
      if (update.has_z_order) what |= DARWIN_ART_SF_LAYER_CHANGED;
      if (update.has_alpha) what |= DARWIN_ART_SF_ALPHA_CHANGED;
      if (update.has_scale) what |= DARWIN_ART_SF_MATRIX_CHANGED;
      if (update.has_visibility) what |= DARWIN_ART_SF_FLAGS_CHANGED;
      if (update.has_parent) what |= DARWIN_ART_SF_REPARENT;
      if (update.has_relative_layer)
        what |= DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
      if (update.has_transform)
        what |= DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED;
      if (update.has_crop) what |= DARWIN_ART_SF_CROP_CHANGED;
      if (update.has_buffer) what |= DARWIN_ART_SF_BUFFER_CHANGED;
      if (update.has_damage) what |= DARWIN_ART_SF_DAMAGE_CHANGED;
      if (update.has_geometry)
        what |= DARWIN_ART_SF_DESTINATION_FRAME_CHANGED;

      const uint32_t parent_id = update.has_parent
          ? update.parent_id
          : control->parent_id;
      const uint32_t relative_id = update.has_relative_layer
          ? update.relative_parent_id
          : control->relative_parent_id;
      const ARect destination = update.has_geometry
          ? update.destination
          : control->destination;
      result.push_back({
          .layer_id = control->layer_id,
          .parent_id = parent_id,
          .relative_parent_id = relative_id,
          .what = what,
          .flags = update.has_visibility
              ? (update.visible ? 0u : 1u)
              : (control->visible ? 0u : 1u),
          .mask = update.has_visibility ? 1u : 0u,
          .transform = update.has_transform
              ? static_cast<uint32_t>(update.transform)
              : static_cast<uint32_t>(control->transform),
          .x = update.has_position ? static_cast<float>(update.position_x)
                                   : static_cast<float>(control->position_x),
          .y = update.has_position ? static_cast<float>(update.position_y)
                                   : static_cast<float>(control->position_y),
          .scale_x = update.has_scale ? update.scale_x : control->scale_x,
          .scale_y = update.has_scale ? update.scale_y : control->scale_y,
          .z = (update.has_z_order || update.has_relative_layer)
              ? update.z_order
              : control->z_order,
          .alpha = update.has_alpha ? update.alpha : control->alpha,
          .destination_left = destination.left,
          .destination_top = destination.top,
          .destination_right = destination.right,
          .destination_bottom = destination.bottom,
          .crop_left = update.has_crop ? update.crop.left : control->crop.left,
          .crop_top = update.has_crop ? update.crop.top : control->crop.top,
          .crop_right = update.has_crop ? update.crop.right : control->crop.right,
          .crop_bottom = update.has_crop ? update.crop.bottom : control->crop.bottom,
      });
    }
    *out = std::move(result);
    return true;
  } catch (...) {
    return false;
  }
}

bool BuildSurfaceControlSnapshot(
    const std::vector<SurfaceControlStateView>& controls,
    const std::vector<SurfaceControlUpdateView>& updates,
    uint32_t local_process_id,
    bool central_surfaceflinger,
    SurfaceControlSnapshot* out) {
  if (out == nullptr) return false;
  try {
    SurfaceControlSnapshot snapshot;
    if (!BuildSurfaceControlFrontendUpdates(
            controls, updates, &snapshot.frontend_updates)) {
      return false;
    }
    snapshot.control_states.reserve(updates.size() + controls.size());
    snapshot.presentations.reserve(controls.size());

    for (const auto& update : updates) {
      const auto* control = FindControl(controls, update.opaque);
      if (control == nullptr) continue;
      uint64_t what = 0;
      if (update.has_position) what |= DARWIN_ART_SF_POSITION_CHANGED;
      if (update.has_z_order) what |= DARWIN_ART_SF_LAYER_CHANGED;
      if (update.has_alpha) what |= DARWIN_ART_SF_ALPHA_CHANGED;
      if (update.has_scale) what |= DARWIN_ART_SF_MATRIX_CHANGED;
      if (update.has_visibility) what |= DARWIN_ART_SF_FLAGS_CHANGED;
      if (update.has_parent) what |= DARWIN_ART_SF_REPARENT;
      if (update.has_relative_layer)
        what |= DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
      if (update.has_transform)
        what |= DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED;
      if (update.has_crop) what |= DARWIN_ART_SF_CROP_CHANGED;
      if (update.has_buffer) what |= DARWIN_ART_SF_BUFFER_CHANGED;
      if (update.has_damage) what |= DARWIN_ART_SF_DAMAGE_CHANGED;
      if (update.has_geometry)
        what |= DARWIN_ART_SF_DESTINATION_FRAME_CHANGED;

      const uint32_t parent_owner = update.has_parent
          ? update.parent_owner_process_id
          : control->parent_owner_process_id;
      const uint32_t parent_id = update.has_parent
          ? update.parent_id
          : control->parent_id;
      const uint32_t relative_owner = update.has_relative_layer
          ? update.relative_parent_owner_process_id
          : control->relative_parent_owner_process_id;
      const uint32_t relative_id = update.has_relative_layer
          ? update.relative_parent_id
          : control->relative_parent_id;
      const ARect destination = update.has_geometry
          ? update.destination
          : control->destination;
      const bool has_transparent_region =
          update.has_transparent_region || !control->transparent_region.empty();
      const uint64_t structural_what =
          what & ~static_cast<uint64_t>(DARWIN_ART_SF_BUFFER_CHANGED);
      if (structural_what != 0 || has_transparent_region) {
        snapshot.control_states.push_back({
            .owner_process_id = control->owner_process_id,
            .layer_id = control->layer_id,
            .parent_owner_process_id = parent_owner,
            .parent_id = parent_id,
            .relative_parent_owner_process_id = relative_owner,
            .relative_parent_id = relative_id,
            .what = structural_what,
            .flags = update.has_visibility
                ? (update.visible ? 0u : 1u)
                : (control->visible ? 0u : 1u),
            .mask = update.has_visibility ? 1u : 0u,
            .transform = update.has_transform
                ? static_cast<uint32_t>(update.transform)
                : static_cast<uint32_t>(control->transform),
            .iosurface = nullptr,
            .destination_left = update.has_position
                ? update.position_x : destination.left,
            .destination_top = update.has_position
                ? update.position_y : destination.top,
            .destination_right = destination.right,
            .destination_bottom = destination.bottom,
            .position_x = update.has_position
                ? update.position_x : control->position_x,
            .position_y = update.has_position
                ? update.position_y : control->position_y,
            .scale_x = update.has_scale ? update.scale_x : control->scale_x,
            .scale_y = update.has_scale ? update.scale_y : control->scale_y,
            .has_crop = update.has_crop || control->has_crop,
            .crop_left = update.has_crop ? update.crop.left : control->crop.left,
            .crop_top = update.has_crop ? update.crop.top : control->crop.top,
            .crop_right = update.has_crop ? update.crop.right : control->crop.right,
            .crop_bottom = update.has_crop ? update.crop.bottom : control->crop.bottom,
            .z = (update.has_z_order || update.has_relative_layer)
                ? update.z_order : control->z_order,
            .alpha = update.has_alpha ? update.alpha : control->alpha,
        });
        const auto& region = update.has_transparent_region
            ? update.transparent_region : control->transparent_region;
        PopulateTransparentRegion(&snapshot.control_states.back(), region,
                                  has_transparent_region);
      }
    }

    for (const auto& control : controls) {
      if (control.buffer != nullptr ||
          control.owner_process_id != local_process_id) {
        continue;
      }
      const bool already_present = std::any_of(
          snapshot.control_states.begin(), snapshot.control_states.end(),
          [&control](const DarwinArtMetalComposerLayer& state) {
            return state.owner_process_id == control.owner_process_id &&
                   state.layer_id == control.layer_id;
          });
      if (already_present) continue;
      const uint64_t ordering_change = control.relative_parent_id == 0
          ? DARWIN_ART_SF_LAYER_CHANGED
          : DARWIN_ART_SF_RELATIVE_LAYER_CHANGED;
      snapshot.control_states.push_back({
          .owner_process_id = control.owner_process_id,
          .layer_id = control.layer_id,
          .parent_owner_process_id = control.parent_owner_process_id,
          .parent_id = control.parent_id,
          .relative_parent_owner_process_id =
              control.relative_parent_owner_process_id,
          .relative_parent_id = control.relative_parent_id,
          .what = DARWIN_ART_SF_POSITION_CHANGED | ordering_change |
              DARWIN_ART_SF_ALPHA_CHANGED | DARWIN_ART_SF_FLAGS_CHANGED |
              DARWIN_ART_SF_MATRIX_CHANGED |
              (control.has_crop ? DARWIN_ART_SF_CROP_CHANGED : 0) |
              (control.parent_id == 0 ? 0 : DARWIN_ART_SF_REPARENT) |
              DARWIN_ART_SF_BUFFER_TRANSFORM_CHANGED,
          .flags = control.visible ? 0u : 1u,
          .mask = 1u,
          .transform = static_cast<uint32_t>(control.transform),
          .iosurface = nullptr,
          .destination_left = control.position_x,
          .destination_top = control.position_y,
          .destination_right = control.position_x,
          .destination_bottom = control.position_y,
          .position_x = control.position_x,
          .position_y = control.position_y,
          .scale_x = control.scale_x,
          .scale_y = control.scale_y,
          .has_crop = control.has_crop,
          .crop_left = control.crop.left,
          .crop_top = control.crop.top,
          .crop_right = control.crop.right,
          .crop_bottom = control.crop.bottom,
          .z = control.z_order,
          .alpha = control.alpha,
      });
      PopulateTransparentRegion(&snapshot.control_states.back(),
                                control.transparent_region,
                                !control.transparent_region.empty());
    }

    for (const auto& control : controls) {
      if (!control.visible || control.buffer == nullptr ||
          (!central_surfaceflinger && !control.attached_to_root)) {
        continue;
      }
      snapshot.presentations.push_back(
          MakePresentation(control, FindUpdate(updates, control.opaque)));
    }
    *out = std::move(snapshot);
    return true;
  } catch (...) {
    return false;
  }
}

}  // namespace darwin_art::window
