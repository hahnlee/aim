#pragma once

#import <AppKit/AppKit.h>
#include "appkit_content_view.h"
#include "../darwin_surface_bridge.h"

namespace aim::window {

// The Android raster scale of a desktop root window (AIM_WINDOW_SCALE
// or the window's display backing scale).
uint32_t SurfaceRasterScale(NSWindow* window);
// CGDirectDisplayID of the screen showing a desktop root window, 0 if none.
uint32_t SurfaceDisplayId(NSWindow* window);
// Installs the process's NSApplication delegate once: Dock reopen shows a
// hidden root, Quit asks Android to remove the task.
void InstallSurfaceApplicationDelegate(NSApplication* application);

// Backing allocation stays with the surface owner. This AppKit delegate
// reports host resize/lifecycle facts, not Android display/focus policy.
using SurfaceResize = AimSurfaceResult (*)(AimSurface*, uint32_t,
    uint32_t, bool, uint32_t, uint32_t);
id<NSWindowDelegate> CreateSurfaceWindowDelegate(AimSurface* surface,
                                               SurfaceResize resize);

}  // namespace aim::window
