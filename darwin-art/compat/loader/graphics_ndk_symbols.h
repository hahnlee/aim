#pragma once
#include <cstdint>

namespace darwin_art::loader {
// Exact public NDK surface backed by the original AOSP graphics objects.
uintptr_t GraphicsNdkSymbol(const char* symbol);
}
