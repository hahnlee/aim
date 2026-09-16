#include "image_lifecycle.h"
#include "darwin_art_bionic_vm.h"
#include <cstdlib>

namespace darwin_art::loader {
namespace images = android::darwin_art_image_registry;
std::shared_ptr<ImageLifecycle> ImageLifecycle::Create(Registry registry) {
  if (!registry) return {};
  auto* dso = darwin_art_bionic_dso_lifecycle_owner_create();
  if (!dso) return {};
  // Guard allocation failure before transferring ownership to the new object.
  std::unique_ptr<DarwinArtBionicDsoLifecycleOwner,
      decltype(&darwin_art_bionic_dso_lifecycle_owner_destroy)> guard(
      dso, darwin_art_bionic_dso_lifecycle_owner_destroy);
  auto* result = new ImageLifecycle(std::move(registry), dso);
  guard.release();
  return std::shared_ptr<ImageLifecycle>(result);
}
int ImageLifecycle::Publish(uintptr_t start, uintptr_t end) {
  std::lock_guard lock(mutex_);
  if (start >= end || images::Publish(registry_.get(), start, end) != 0) return -1;
  if (darwin_art_bionic_vm_register_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0) {
    if (images::RollbackPublish(registry_.get(), start, end) != 0) std::abort();
    return -1;
  }
  if (darwin_art_bionic_dso_lifecycle_publish_image(dso_.get(), start, end) == 0) return 0;
  if (darwin_art_bionic_vm_unregister_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0 ||
      images::RollbackPublish(registry_.get(), start, end) != 0) std::abort();
  return -1;
}
int ImageLifecycle::Finalize(uintptr_t start, uintptr_t end) {
  std::lock_guard lock(mutex_);
  if (start >= end || darwin_art_bionic_dso_lifecycle_finalize_image(
          dso_.get(), start, end) != 0) return -1;
  // Destructors have run: a bookkeeping failure is not a retryable close.
  if (darwin_art_bionic_vm_unregister_borrowed_range(
          reinterpret_cast<void*>(start), end - start) != 0 ||
      images::Finalize(registry_.get(), start, end) != 0) std::abort();
  return 0;
}
}
