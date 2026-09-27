#pragma once
#include <cstddef>
#include <cstdint>

// Rust profile protocol owner. No native wire decoder or system-UID fallback.
extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t pid);
struct DarwinArtRegisteredProcessIdentity {
  int32_t uid;
  char package[256];
};
extern "C" bool darwin_art_runtime_registered_process_identity(
    uint32_t pid, DarwinArtRegisteredProcessIdentity* output);
// The Intent the registered process's launch requested: 1 with NUL-terminated
// action and data (data empty when absent), 0 for none, -1 on failure.
extern "C" int32_t darwin_art_runtime_registered_launch_intent(
    uint32_t pid, char* action, size_t action_capacity, char* data,
    size_t data_capacity);
