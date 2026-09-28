#pragma once
#include <cstdint>

namespace aim::loader {
// Exact public NDK surface backed by the original AOSP graphics objects.
uintptr_t GraphicsNdkSymbol(const char* symbol);
}
