#pragma once
#include "darwin_art_linker_namespace.h"
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>
class NamespaceConfig;

namespace darwin_art::loader {
// Owns a newly configured registry. No resident linker/libdl images are
// fabricated here: startup must publish actual image owners before using it.
class ConfiguredNamespaces {
 public:
  // Reads through original bionic + guest filesystem. Serializes its singleton
  // parser and copies all state before releasing the parser lock.
  static std::unique_ptr<ConfiguredNamespaces> Read(
      const std::string& config_path, const std::string& executable_path,
      bool asan, bool hwasan, const std::string& default_ld_library_path,
      std::string* error);
  static std::unique_ptr<ConfiguredNamespaces> Create(
      const std::vector<std::unique_ptr<NamespaceConfig>>&,
      const std::string& default_ld_library_path, std::string* error);
  LinkerRegistry* registry() const { return registry_.get(); }
  uint64_t Find(const std::string& name) const;
  int target_sdk_version() const { return target_sdk_version_; }
 private:
  struct Drop { void operator()(LinkerRegistry* p) const { darwin_art_linker_registry_destroy(p); } };
  std::unique_ptr<LinkerRegistry, Drop> registry_;
  std::unordered_map<std::string, uint64_t> ids_;
  int target_sdk_version_ = 0; // Unset for manually constructed config objects.
};
}  // namespace darwin_art::loader
