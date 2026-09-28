#pragma once
#include "aim_elf_loader.h"
#include "aim_linker_namespace.h"
#include <memory>
#include <string>
#include <vector>

namespace aim::loader {
// Only the selected ELF's exports; no private dependency/global fallback.
// Lease owns returned address. 0 found, 1 absent, -1 invalid/error.
int ResolveNamespaceElf(const LinkerImageLease*, const char* symbol,
    const char* version, uintptr_t* address, std::string* error);
// Transports the namespace owner's ordered DF_1_GLOBAL group. Resource
// ownership remains in Rust. Does not discover libraries or infer visibility.
class NamespaceElfGroup {
 public:
  bool Capture(LinkerRegistry*, uint64_t namespace_id, std::string* error);
  const AimElfGlobalSource* data() const { return sources_.data(); }
  size_t size() const { return sources_.size(); }
 private:
  struct Drop {
    void operator()(LinkerImageGroup* group) const {
      aim_linker_group_destroy(group);
    }
  };
  std::unique_ptr<LinkerImageGroup, Drop> group_;
  std::vector<AimElfGlobalSource> sources_;
};

// Publish the actual selected ELF image with its mapped flags; the registry
// retains an independent Rust owner. Source remains owned by the caller.
bool PublishNamespaceElf(LinkerRegistry*, uint64_t namespace_id,
    const char* canonical_path, AimElfSelectedImage*, bool rtld_global,
    std::string* error);
}  // namespace aim::loader
