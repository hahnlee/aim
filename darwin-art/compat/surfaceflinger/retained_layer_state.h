#pragma once

#include "composition_protocol.h"

#include <cstdint>

namespace darwin_art::surfaceflinger {

// Copies a bounded, valid-only transparent-region hint into destination.
// The return value is the number of rectangles written.
uint32_t CopyTransparentRegion(const DarwinArtTransparentRegionRect* source,
                               uint32_t source_count,
                               DarwinArtTransparentRegionRect* destination);

// Applies one SurfaceControl layer delta to the retained wire-layer state.
// Android owns the change-bit semantics; this module only merges the typed
// pointer-free wire representation used by the SurfaceFlinger boundary.
void MergeRetainedLayer(WireLayer& destination, const WireLayer& source);

}  // namespace darwin_art::surfaceflinger
