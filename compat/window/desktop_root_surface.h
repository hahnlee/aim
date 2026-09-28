#pragma once
#include "../darwin_surface_bridge.h"
#include "desktop_root_target.h"

#ifdef __cplusplus
extern "C" {
#endif

// AppKit-main-only. Facts are observed from the original window; this API
// neither grants Android focus nor redirects keys. Null removes the binding.
// Callback context must not dereference a surface after its destruction.
AimSurfaceResult aim_surface_set_desktop_root_observer(
    AimSurface* surface, const AimDesktopRootObserver* observer);
AimSurfaceResult aim_surface_get_desktop_root_incarnation(
    AimSurface* surface, uint64_t* incarnation);

#ifdef __cplusplus
}

namespace aim::window {
AimSurfaceResult InitializeDesktopRoot(AimSurface* surface);
// Owner-thread acquisition from the exact live surface. Caller must hold the
// surface ownership excluding concurrent destruction. The root field is
// initialized before publication and immutable until surface destruction.
// Returned owner does not retain the surface, select a root or grant focus.
AimSurfaceResult RetainSurfaceDesktopRoot(
    AimSurface* surface, std::shared_ptr<DesktopRootEvents>* owner) noexcept;
}
#endif
