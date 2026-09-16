#pragma once
#include "darwin_android_elf_image_registry.h"
#include "darwin_art_bionic_dso_lifecycle.h"
#include <memory>
#include <mutex>

namespace darwin_art::loader {
// Linker image ownership only: no Java loader, JNI trampolines or app policy.
// The mapping lifecycle retains this object until all its images are finalized.
class ImageLifecycle {
 public:
  using Registry = std::unique_ptr<android::darwin_art_image_registry::Owner,
      decltype(&android::darwin_art_image_registry::Destroy)>;
  static std::shared_ptr<ImageLifecycle> Create(Registry);
  int Publish(uintptr_t start, uintptr_t end);
  int Finalize(uintptr_t start, uintptr_t end);
 private:
  ImageLifecycle(Registry registry, DarwinArtBionicDsoLifecycleOwner* dso)
      : registry_(std::move(registry)), dso_(dso, darwin_art_bionic_dso_lifecycle_owner_destroy) {}
  std::recursive_mutex mutex_;
  Registry registry_;
  std::unique_ptr<DarwinArtBionicDsoLifecycleOwner,
      decltype(&darwin_art_bionic_dso_lifecycle_owner_destroy)> dso_;
};
}
