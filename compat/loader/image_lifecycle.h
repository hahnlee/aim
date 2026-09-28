#pragma once
#include "darwin_android_elf_image_registry.h"
#include "aim_bionic_dso_lifecycle.h"
#include <memory>
#include <mutex>

namespace aim::loader {
// Linker image ownership only: no Java loader, JNI trampolines or app policy.
// The mapping lifecycle retains this object until all its images are finalized.
class ImageLifecycle {
 public:
  using Registry = std::unique_ptr<android::aim_image_registry::Owner,
      decltype(&android::aim_image_registry::Destroy)>;
  static std::shared_ptr<ImageLifecycle> Create(Registry);
  int Publish(uintptr_t start, uintptr_t end);
  int Finalize(uintptr_t start, uintptr_t end);
 private:
  ImageLifecycle(Registry registry, AimBionicDsoLifecycleOwner* dso)
      : registry_(std::move(registry)), dso_(dso, aim_bionic_dso_lifecycle_owner_destroy) {}
  std::recursive_mutex mutex_;
  Registry registry_;
  std::unique_ptr<AimBionicDsoLifecycleOwner,
      decltype(&aim_bionic_dso_lifecycle_owner_destroy)> dso_;
};
}
