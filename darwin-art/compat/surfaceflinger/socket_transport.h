#pragma once

#include <cstddef>

namespace darwin_art::surfaceflinger {
// Profile endpoint byte/SCM transport only. No layer or Android commit policy.
bool WriteAll(int fd, const void* data, size_t size);
bool ReadAll(int fd, void* data, size_t size);
bool SendDescriptor(int socket_fd, int descriptor);
bool ReceiveDescriptor(int socket_fd, bool expected, int* descriptor);
// Owned native descriptor; caller closes it. Suppresses SIGPIPE and sets CLOEXEC.
int Connect(const char* path);
}  // namespace darwin_art::surfaceflinger
