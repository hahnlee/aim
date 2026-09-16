#pragma once
#include "namespace_elf_group.h"
namespace darwin_art::loader {
class NamespaceScopeSnapshot {
 public:
  bool Capture(DarwinArtElfDiscoveredGraph*, LinkerDiscovery*, LinkerRegistry*,
      uint64_t query_namespace, std::string*);
  const std::vector<DarwinArtElfGlobalSource>& globals() const { return globals_; }
  bool ready() const { return ready_; }
 private:
  bool ready_ = false;
  std::vector<std::unique_ptr<NamespaceElfGroup>> owners_;
  std::vector<DarwinArtElfGlobalSource> globals_;
};
}
