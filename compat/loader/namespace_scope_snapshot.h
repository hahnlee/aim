#pragma once
#include "namespace_elf_group.h"
namespace aim::loader {
class NamespaceScopeSnapshot {
 public:
  bool Capture(AimElfDiscoveredGraph*, LinkerDiscovery*, LinkerRegistry*,
      uint64_t query_namespace, std::string*);
  const std::vector<AimElfGlobalSource>& globals() const { return globals_; }
  bool ready() const { return ready_; }
 private:
  bool ready_ = false;
  std::vector<std::unique_ptr<NamespaceElfGroup>> owners_;
  std::vector<AimElfGlobalSource> globals_;
};
}
