#pragma once
#include "aim_elf_loader.h"
#include <cstdint>
#include <string>

namespace aim::loader {
// Exact image lookup, not a replacement for Android global/local symbol order.
// The caller selects the image in that order. Graph owns the returned address's
// image lease and must outlive every use (including loaded-image finalizers).
// 0 resolved, 1 not admitted/not exported/version mismatch, -1 invalid state.
int ResolveAdmittedProviderSymbol(const AimElfDiscoveredGraph*,
    const char* soname, const char* symbol, const char* version,
    uintptr_t* address, std::string* error);
// ELF resolver ABI: context borrows a discovered graph; request must contain
// exactly one image selected by ordered graph lookup. No C++ unwind crosses ABI.
AimElfResolveStatus ResolveAdmittedProvider(void* context,
    const AimElfSymbolRequest*, uintptr_t*, AimElfErrorBuffer*) noexcept;
}
