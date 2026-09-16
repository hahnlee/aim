#include "process_namespace_installation.h"
#include "system_native_images.h"
#include "darwin_art/darwin_art.h"

namespace darwin_art::loader {
std::unique_ptr<ProcessNamespaceInstallation> ProcessNamespaceInstallation::CreateFromConfig(
    const darwin_art_native_loader_config* config, std::string* error) try {
  if (error) error->clear();
  if (!config || config->struct_size < sizeof(*config) ||
      config->abi_version != DARWIN_ART_NATIVE_LOADER_CONFIG_ABI_VERSION) {
    if (error) *error = "invalid native loader startup config ABI";
    return nullptr;
  }
  const auto absolute = [](const char* path) { return path && path[0] == '/'; };
  if (!absolute(config->linker_config_path) || !absolute(config->executable_path) ||
      !absolute(config->android_unwind_path) || !config->library_search_path) {
    if (error) *error = "native loader startup paths are incomplete or relative";
    return nullptr;
  }
  auto images = SystemNativeImages::Create(config->android_unwind_path, error);
  if (!images) return nullptr;
  return Create(config->linker_config_path, config->executable_path, false, false,
      config->library_search_path, images->providers(), images->placements(), error);
} catch (...) {
  return nullptr;
}
}
