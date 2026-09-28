#include "display_output.h"
#include "../darwin_surface_internal.h"

AimSurfaceResult AttachDisplayOutput(AimSurface* surface,
    const char* endpoint, uint32_t android_width, uint32_t android_height) {
  if (![NSThread isMainThread]) return AIM_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || endpoint == nullptr || endpoint[0] == '\0' ||
      android_width == 0 || android_height == 0 || surface->backing->surface() == nullptr)
    return AIM_SURFACE_INVALID_ARGUMENT;
  if (surface->window_closed.load(std::memory_order_acquire) ||
      surface->output_owner != nullptr)
    return AIM_SURFACE_INVALID_ARGUMENT;
  aim::surfaceflinger::OutputRequest request{};
  request.iosurface_id = IOSurfaceGetID(surface->backing->surface());
  request.physical_width = surface->backing->physical_width();
  request.physical_height = surface->backing->physical_height();
  request.logical_width = android_width;
  request.logical_height = android_height;
  auto owner = aim::surfaceflinger::OutputOwner::Register(endpoint, request);
  if (!owner) return AIM_SURFACE_ALLOCATION_FAILED;
  surface->output_owner = std::move(owner);
  return AIM_SURFACE_OK;
}

void RetireDisplayOutput(AimSurface* surface) {
  if (surface != nullptr && surface->output_owner != nullptr)
    surface->output_owner->Retire();
}

AimSurfaceResult ConfigureDisplayOutputExtent(AimSurface* surface,
    uint32_t android_width, uint32_t android_height) {
  if (![NSThread isMainThread]) return AIM_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || android_width == 0 || android_height == 0)
    return AIM_SURFACE_INVALID_ARGUMENT;
  if (surface->backing->logical_width() == android_width &&
      surface->backing->logical_height() == android_height)
    return surface->window_closed.load(std::memory_order_acquire)
        ? AIM_SURFACE_INVALID_ARGUMENT : AIM_SURFACE_OK;
  auto candidate = aim::graphics::RetainSurfaceBacking(
      surface->backing->surface(), surface->backing->texture(),
      surface->backing->physical_width(), surface->backing->physical_height(),
      android_width, android_height, surface->backing->bytes_per_row(),
      surface->backing->revision() + 1);
  if (!candidate) return AIM_SURFACE_ALLOCATION_FAILED;
  const auto result = ReplaceDisplayOutput(surface, surface->backing->surface(),
      surface->backing->physical_width(), surface->backing->physical_height(), android_width, android_height);
  if (result != AIM_SURFACE_OK) return result;
  if (!surface->backing_owner.Publish(surface->backing, candidate)) {
    RetireDisplayOutput(surface);
    surface->window_closed.store(true, std::memory_order_release);
    return AIM_SURFACE_ALLOCATION_FAILED;
  }
  surface->backing = std::move(candidate);
  return AIM_SURFACE_OK;
}

AimSurfaceResult ReplaceDisplayOutput(AimSurface* surface,
    void* candidate_iosurface, uint32_t physical_width, uint32_t physical_height,
    uint32_t android_width, uint32_t android_height) {
  if (![NSThread isMainThread]) return AIM_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || candidate_iosurface == nullptr ||
      physical_width == 0 || physical_height == 0 ||
      android_width == 0 || android_height == 0)
    return AIM_SURFACE_INVALID_ARGUMENT;
  if (surface->window_closed.load(std::memory_order_acquire))
    return AIM_SURFACE_INVALID_ARGUMENT;
  // Local fixtures have no central output until an explicit ready attachment.
  if (!surface->output_owner) return AIM_SURFACE_OK;
  aim::surfaceflinger::OutputRequest request{};
  request.iosurface_id = IOSurfaceGetID(static_cast<IOSurfaceRef>(candidate_iosurface));
  request.physical_width = physical_width;
  request.physical_height = physical_height;
  request.logical_width = android_width;
  request.logical_height = android_height;
  if (surface->output_owner->Replace(request)) return AIM_SURFACE_OK;
  if (!surface->output_owner->locally_open()) {
    // Ambiguous IPC failure retired even the old registration. Do not present
    // the old tuple as an authorized output or silently register another one.
    surface->window_closed.store(true, std::memory_order_release);
  }
  return AIM_SURFACE_ALLOCATION_FAILED;
}
