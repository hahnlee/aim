#pragma once

#include "darwin_art_linker_namespace.h"

#include <cstdint>
#include <string>

namespace darwin_art::loader {

// libnativewindow is an Android ABI facade over an ANativeWindow supplied by
// Surface/BufferQueue.  This is a private typed image kind, never a dyld
// handle and never a Bionic provider image.
// Publish the process-owned libnativewindow facade.  The canonical path is
// retained as immutable image identity; the underlying Darwin Surface owner
// remains process-owned and is not closed by this image.
bool PublishNativeWindowImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error);

// Resolve only the public symbols implemented by the current Darwin
// Surface/BufferQueue owner.  Returns 0 for found, 1 for missing/unsupported,
// and -1 for invalid image or arguments.  `out` is cleared on every failure.
int ResolveNativeWindowImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);

// Safe owner check: does not dereference an arbitrary image payload.
bool IsNativeWindowImage(const LinkerImageLease* lease);

}  // namespace darwin_art::loader
