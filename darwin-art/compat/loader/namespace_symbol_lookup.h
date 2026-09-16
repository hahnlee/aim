#pragma once
#include "darwin_art_linker_namespace.h"
#include <string>
namespace darwin_art::loader {
// Caller holds a complete namespace operation. Returned addresses borrow the
// live library's resources; lookup adds no logical opens or visibility grants.
uintptr_t LookupNamespaceSymbol(LinkerRegistry*, const LinkerImageLease*,
    const char*, std::string*, const char* version = nullptr,
    LinkerImageLease** defining_image = nullptr);
// Optional defining_image receives an owned clone of the exact symbol source,
// not the requesting library. It is cleared on entry, stays null on failure,
// and must be released by the caller. This does not create a logical dlopen.
// Original local-group walk for NEXT: skip through this exact image before
// testing namespace accessibility. Caller supplies the original group root.
uintptr_t LookupNamespaceSymbolAfter(LinkerRegistry*, const LinkerImageLease* root,
    const LinkerImageLease* after, const char*, std::string*, const char* version = nullptr,
    LinkerImageLease** defining_image = nullptr);
// Single-image resolver: 0 found, 1 absent, negative malformed owner/error.
int ResolveNamespaceImageSymbol(const LinkerImageLease*, const char*, uintptr_t*, std::string*,
    const char* version = nullptr);
}
