#include "desktop_root_surface.h"
#include "../darwin_surface_internal.h"

namespace darwin_art::window {
DarwinArtSurfaceResult RetainSurfaceDesktopRoot(
    DarwinArtSurface* surface, std::shared_ptr<DesktopRootEvents>* owner) noexcept {
  if (surface == nullptr || owner == nullptr)
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  auto retained = surface->desktop_root_events;
  if (retained == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  if (retained->closed()) return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  // All surface reads precede destruction of the caller's previous owner.
  *owner = std::move(retained);
  return DARWIN_ART_SURFACE_OK;
}

DarwinArtSurfaceResult InitializeDesktopRoot(DarwinArtSurface* surface) {
  if (![NSThread isMainThread]) return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || surface->window == nil)
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  if (surface->desktop_root_events != nullptr)
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  auto owner = DesktopRootEvents::Create(surface->window);
  if (owner == nullptr) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  auto target = DesktopRootTarget::Create(owner, surface->window);
  if (target == nullptr) return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  if (!target->Publish()) {
    (void)target->Close();
    return DARWIN_ART_SURFACE_ALLOCATION_FAILED;
  }
  surface->desktop_root_events = std::move(owner);
  surface->desktop_root_target = std::move(target);
  return DARWIN_ART_SURFACE_OK;
}
}

DarwinArtSurfaceResult darwin_art_surface_set_desktop_root_observer(
    DarwinArtSurface* surface, const DarwinArtDesktopRootObserver* observer) {
  if (![NSThread isMainThread]) return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  // Read all raw-surface state before callback-capable retention/release.
  // The observer may synchronously destroy the surface; these snapshots are
  // the only authorities touched afterward.
  auto events = surface->desktop_root_events;
  auto target = surface->desktop_root_target;
  if (events == nullptr || target == nullptr)
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  if (target->incarnation() == 0) return DARWIN_ART_SURFACE_WINDOW_CLOSED;
  const bool accepted = observer == nullptr
      ? events->Unbind()
      : target->Bind(*observer);
  if (accepted) return DARWIN_ART_SURFACE_OK;
  return events->closed() ? DARWIN_ART_SURFACE_WINDOW_CLOSED
                          : DARWIN_ART_SURFACE_INVALID_ARGUMENT;
}

DarwinArtSurfaceResult darwin_art_surface_get_desktop_root_incarnation(
    DarwinArtSurface* surface, uint64_t* incarnation) {
  if (![NSThread isMainThread]) return DARWIN_ART_SURFACE_NOT_MAIN_THREAD;
  if (surface == nullptr || incarnation == nullptr)
    return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  auto target = surface->desktop_root_target;
  if (target == nullptr) return DARWIN_ART_SURFACE_INVALID_ARGUMENT;
  *incarnation = target->incarnation();
  return DARWIN_ART_SURFACE_OK;
}
