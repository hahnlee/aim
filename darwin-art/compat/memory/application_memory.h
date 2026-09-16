#pragma once
#include <cstddef>

namespace darwin_art::memory {
// Platform operations used by the original ApplicationSharedMemory JNI owner.
int CreateApplicationMemory(size_t size) noexcept;
void* MapApplicationMemory(int fd, size_t size, bool writable) noexcept;
int UnmapApplicationMemory(void* address, size_t size) noexcept;
int DuplicateApplicationMemoryReader(int fd) noexcept;
void AbandonApplicationMemory(int fd) noexcept;
}
