#pragma once

#import <AppKit/AppKit.h>
#include "appkit_content_view.h"
#include "../darwin_surface_bridge.h"

namespace darwin_art::window {

// Backing allocation stays with the surface owner. This AppKit delegate
// reports host resize/lifecycle facts, not Android display/focus policy.
using SurfaceResize = DarwinArtSurfaceResult (*)(DarwinArtSurface*, uint32_t,
    uint32_t, bool, uint32_t, uint32_t);
id<NSWindowDelegate> CreateSurfaceWindowDelegate(DarwinArtSurface* surface,
                                               SurfaceResize resize);

}  // namespace darwin_art::window
