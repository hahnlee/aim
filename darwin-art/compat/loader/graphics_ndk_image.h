#pragma once
#include "darwin_art_linker_namespace.h"
#include <cstdint>
#include <string>

namespace darwin_art::loader {
bool PublishGraphicsNdkImage(LinkerRegistry*, uint64_t namespace_id,
    const char* soname, const char* canonical, std::string* error);
bool IsGraphicsNdkImage(const LinkerImageLease*);
int ResolveGraphicsNdkImage(const LinkerImageLease*, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);
// Strong original implementation references, scoped to the public NDK module.
uintptr_t GraphicsNdkSymbol(const char* symbol);
}
