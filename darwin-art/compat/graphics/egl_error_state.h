#pragma once

#include <cstdint>

namespace darwin_art::graphics {

// EGL errors are one-shot per calling thread.  All public dispatch surfaces
// (JNI, the Android C facade, and the ELF resolver) use this same owner;
// ANGLE's raw eglGetError remains only a provider callback in the backend
// table and is never exposed as a second error queue.
constexpr std::int32_t kEglSuccess = 0x3000;
constexpr std::int32_t kEglNotInitialized = 0x3001;
constexpr std::int32_t kEglBadAlloc = 0x3003;
constexpr std::int32_t kEglBadDisplay = 0x3008;
constexpr std::int32_t kEglBadMatch = 0x3009;
constexpr std::int32_t kEglBadNativeWindow = 0x300B;
constexpr std::int32_t kEglBadParameter = 0x300C;

void SetEglError(std::int32_t error);
std::int32_t PeekEglError();
std::int32_t ConsumeEglError();

}  // namespace darwin_art::graphics
