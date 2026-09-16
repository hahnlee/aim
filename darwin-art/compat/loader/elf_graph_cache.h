#pragma once

#include <cstdint>
#include <string>
#include <vector>
#include "darwin_art_elf_loader.h"

namespace android {
// Owns cloned graph references per (ClassLoader identity, SONAME), in load order. Lookups
// never cross namespace boundaries. Public/shared links belong to the namespace
// policy owner, not an implicit process-global fallback in this cache.
// Graph FFI calls and final release run outside the cache mutex. Registration
// callers keep the source handle live until return. Lookup retains a graph for
// the operation; users of a returned address still need their owning load
// handle (this API does not transfer a lifetime lease with the raw address).
std::vector<std::string> SnapshotCachedElfSonames(uint64_t namespace_id);
bool RegisterCachedElfGraph(uint64_t namespace_id, const char* root_soname,
                            DarwinArtElfGraphHandle* graph, std::string* error);
// Failed or stale owners cannot evict another graph with the same SONAME.
void UnregisterCachedElfGraph(uint64_t namespace_id, const char* root_soname,
                              DarwinArtElfGraphHandle* source);
DarwinArtElfResolveStatus ResolveCachedElfProvider(
    uint64_t namespace_id, const DarwinArtElfSymbolRequest* request,
    uintptr_t* out_address, DarwinArtElfErrorBuffer* error);
}  // namespace android
