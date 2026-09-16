#include "configured_namespaces.h"
#include "linker_config.h"
#include <mutex>

namespace darwin_art::loader {
std::unique_ptr<ConfiguredNamespaces> ConfiguredNamespaces::Read(
    const std::string& path, const std::string& executable, bool asan, bool hwasan,
    const std::string& ld_paths, std::string* error) {
  if (error) error->clear();
  if (path.empty() || executable.empty() || path.front() != '/' || executable.front() != '/'
      || path.find('\0') != std::string::npos || executable.find('\0') != std::string::npos) {
    if (error) *error = "linker configuration requires absolute guest paths";
    return nullptr;
  }
  // Config::read_binary_config uses global scratch state. Keep its entire
  // parse/copy lifetime under one lock; published registries never borrow it.
  static std::mutex parser_mutex;
  std::lock_guard lock(parser_mutex);
  const Config* parsed = nullptr;
  std::string detail;
  if (!Config::read_binary_config(path.c_str(), executable.c_str(), asan, hwasan, &parsed, &detail)
      || !parsed) {
    if (error) *error = detail.empty() ? "no linker configuration for " + executable : detail;
    return nullptr;
  }
  auto result = Create(parsed->namespace_configs(), ld_paths, error);
  if (result) result->target_sdk_version_ = parsed->target_sdk_version();
  return result;
}
}  // namespace darwin_art::loader
