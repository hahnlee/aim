#pragma once

#include "darwin_art_linker_namespace.h"

#include <cstdint>
#include <string>

namespace darwin_art::loader {

// ld-android.so is an Android linker-owned ABI boundary, not a guest DSO.
// Its image identity is retained in the registry while its symbols dispatch
// to the process-owned linker namespace implementation.
bool PublishLinkerImage(LinkerRegistry* registry, uint64_t id,
    const char* soname, const char* canonical, std::string* error);

// Resolves the private __loader_* functions exported by the linker owner.
// Returns zero for a symbol, one for a missing/unsupported symbol, and -1 for
// an invalid lease or argument. Versioned requests are currently unsupported
// because this native image has no real loader version metadata.
int ResolveLinkerImage(const LinkerImageLease* lease, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);

// Identifies an image published by PublishLinkerImage without dereferencing an
// arbitrary payload. A generic image publication is not sufficient.
bool IsLinkerImage(const LinkerImageLease* lease);

}  // namespace darwin_art::loader
