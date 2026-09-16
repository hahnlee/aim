#pragma once
#include <cstdint>

// Rust profile protocol owner. No native wire decoder or system-UID fallback.
extern "C" int32_t darwin_art_runtime_registered_process_uid(uint32_t pid);
struct DarwinArtRegisteredProcessIdentity {
  int32_t uid;
  char package[256];
};
extern "C" bool darwin_art_runtime_registered_process_identity(
    uint32_t pid, DarwinArtRegisteredProcessIdentity* output);
