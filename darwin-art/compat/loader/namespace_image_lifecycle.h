#pragma once
#include "namespace_handles.h"
namespace darwin_art::loader {
struct LifecycleDrop {
  void operator()(DarwinArtElfLifecycleOwner* value) const {
    darwin_art_elf_lifecycle_owner_destroy(value);
  }
};
using OwnedImageLifecycle = std::unique_ptr<DarwinArtElfLifecycleOwner, LifecycleDrop>;
OwnedImageLifecycle CreateNamespaceImageLifecycle(const DiscoveredGraph&, std::string* error);
}
