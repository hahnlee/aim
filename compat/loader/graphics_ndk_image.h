#pragma once
#include "aim_linker_namespace.h"
#include "graphics_ndk_symbols.h"
#include <cstdint>
#include <string>

namespace aim::loader {
bool PublishGraphicsNdkImage(LinkerRegistry*, uint64_t namespace_id,
    const char* soname, const char* canonical, std::string* error);
bool IsGraphicsNdkImage(const LinkerImageLease*);
int ResolveGraphicsNdkImage(const LinkerImageLease*, const char* symbol,
    const char* version, uintptr_t* out, std::string* error);
}
