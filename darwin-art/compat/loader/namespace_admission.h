#pragma once
#include "darwin_art_elf_loader.h"
#include "darwin_art_linker_namespace.h"
#include <map>
#include <vector>

namespace darwin_art::loader {
// Synchronous discovery adapter. File context is borrowed; resident leases
// transfer to graph. All image IDs belong to the Rust discovery owner.
struct NamespaceAdmission {
  explicit NamespaceAdmission(LinkerDiscovery* owner) : files(owner) {}
  LinkerDiscovery* files;
  // Immutable pointer arrays; strings remain owned by retained selected images.
  std::map<void*, std::vector<const char*>> dependencies;
};
int ReadNamespaceResidentDependencies(void*, void*, const char* const**, size_t*);
int AdmitNamespaceDependency(void*, uint64_t, const char*, const char*,
                             const char*, DarwinArtElfAdmission*);
}
