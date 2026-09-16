#pragma once

#include "darwin_art_linker_namespace.h"

#include <cstdint>
#include <string>

namespace darwin_art::loader {

// Publishes one process-owned ANGLE dispatch image. Only libEGL.so,
// libGLESv1_CM.so, libGLESv2.so and libGLESv3.so are valid; the path is retained in the
// private image payload.
bool PublishAngleImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error);

// Resolves public symbols for the image's own SONAME and supported Android version.
// Returns zero for a symbol, one for an unsupported/missing symbol, and -1
// for an invalid lease or argument.
int ResolveAngleImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);

// Identifies an image published by PublishAngleImage. A generic image
// publication is not sufficient to pass this check.
bool IsAngleImage(const LinkerImageLease* lease);

}  // namespace darwin_art::loader
