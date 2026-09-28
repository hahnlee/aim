#pragma once

#include "../darwin_surface_bridge.h"

// AppKit owner-only output lifetime boundary. Call after service readiness,
// never from a producer/present callback. Android extent is explicit and is
// not derived from NSView points or embedded child geometry.
AimSurfaceResult AttachDisplayOutput(AimSurface* surface,
    const char* endpoint, uint32_t android_width, uint32_t android_height);
void RetireDisplayOutput(AimSurface* surface);
AimSurfaceResult ReplaceDisplayOutput(AimSurface* surface,
    void* candidate_iosurface, uint32_t physical_width, uint32_t physical_height,
    uint32_t android_width, uint32_t android_height);
AimSurfaceResult ConfigureDisplayOutputExtent(AimSurface* surface,
    uint32_t android_width, uint32_t android_height);
