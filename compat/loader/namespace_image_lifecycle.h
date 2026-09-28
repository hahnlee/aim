#pragma once
#include "namespace_handles.h"
namespace aim::loader {
struct LifecycleDrop {
  void operator()(AimElfLifecycleOwner* value) const {
    aim_elf_lifecycle_owner_destroy(value);
  }
};
using OwnedImageLifecycle = std::unique_ptr<AimElfLifecycleOwner, LifecycleDrop>;
OwnedImageLifecycle CreateNamespaceImageLifecycle(const DiscoveredGraph&, std::string* error);
}
