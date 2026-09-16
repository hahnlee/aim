#pragma once
#include <cstddef>
#include "application_memory.h"

namespace darwin_art::memory {
// Java-visible descriptors belong to the Rust filesystem table. None of these
// APIs publishes a Darwin descriptor number or falls back to interpreting one.
int CreateApplicationDescriptor(size_t size) noexcept;
void* MapApplicationDescriptor(int fd, size_t size, bool writable) noexcept;
int DuplicateApplicationDescriptorReader(int fd) noexcept;
int CloseApplicationDescriptor(int fd) noexcept;
}
