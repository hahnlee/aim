#pragma once
#include <cstdint>
namespace darwin_art::loader {
// Existing statically linked Android NDK subsystem exports only. No host
// process-wide dlsym. 0 found, 1 absent/version unsupported, -1 ambiguous.
int ResolveLibandroidPlatformSymbol(const char*, const char* version, uintptr_t*);
}
