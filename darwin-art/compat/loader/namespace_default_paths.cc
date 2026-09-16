#include "namespace_handles.h"
#include "namespace_operation.h"
#include <vector>

namespace darwin_art::loader {
bool NamespaceHandles::DefaultLibraryPath(std::string* output) {
  if (!output) return false;
  output->clear();
  auto* registry = configured_->registry();
  NamespaceOperation operation(registry);
  if (!operation) return false;
  const auto id = configured_->Find("default");
  size_t needed = 0;
  if (darwin_art_linker_default_paths(registry, id, nullptr, 0, &needed) != 1 || !needed)
    return false;
  std::vector<uint8_t> bytes(needed);
  if (darwin_art_linker_default_paths(registry, id, bytes.data(), bytes.size(), &needed) != 0)
    return false;
  output->assign(reinterpret_cast<const char*>(bytes.data()), needed - 1);
  return true;
}
}
