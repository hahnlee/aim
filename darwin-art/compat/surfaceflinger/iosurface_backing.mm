#include "iosurface_backing.h"

#import <IOSurface/IOSurface.h>

namespace darwin_art::surfaceflinger {

std::shared_ptr<const IosurfaceBacking> IosurfaceBacking::Import(uint32_t id) {
  if (id == 0) return {};
  IOSurfaceRef surface = IOSurfaceLookup(id);
  if (surface == nullptr) return {};
  // shared_ptr construction owns the object even if its control allocation
  // throws. Protect the imported reference if object allocation itself fails.
  IosurfaceBacking* backing = nullptr;
  try {
    backing = new IosurfaceBacking(id, surface);
  } catch (...) {
    CFRelease(surface);
    throw;
  }
  return std::shared_ptr<const IosurfaceBacking>(backing);
}

IosurfaceBacking::~IosurfaceBacking() {
  CFRelease(static_cast<IOSurfaceRef>(surface_));
}

std::shared_ptr<const ImportedCompositionBackings>
ImportedCompositionBackings::Import(std::span<const uint32_t> ids) {
  auto imports = std::make_shared<ImportedCompositionBackings>();
  for (uint32_t id : ids) {
    if (id == 0 || imports->surfaces_.contains(id)) continue;
    auto backing = IosurfaceBacking::Import(id);
    if (!backing) return {};
    imports->surfaces_.emplace(id, std::move(backing));
  }
  return imports;
}

std::shared_ptr<const IosurfaceBacking> ImportedCompositionBackings::Find(
    uint32_t id) const {
  const auto found = surfaces_.find(id);
  return found == surfaces_.end() ? nullptr : found->second;
}

}  // namespace darwin_art::surfaceflinger
