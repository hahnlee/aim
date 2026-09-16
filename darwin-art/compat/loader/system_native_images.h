#pragma once
#include "namespace_startup.h"
#include <vector>

namespace darwin_art::loader {
// Ported native images for the pinned Android 16 runtime, not an APK or a
// test fixture. Caller must pair these with that installed system-image/config
// identity; this is not a profile for arbitrary Android releases. Namespace
// visibility still comes from Android linkerconfig.
// Publication retains their resource owners, so this staging owner may be
// released afterward without tearing down an image still used by a namespace.
class SystemNativeImages final {
 public:
  // Explicit trusted installation artifact; the absolute-path check does not
  // establish bundle provenance. Caller supplies that authority. No cwd,
  // environment or host-dyld
  // libunwind fallback. Guest filesystem/provider services must already exist.
  static std::unique_ptr<SystemNativeImages> Create(
      const std::string& android_unwind_path, std::string* error);
  const SharedBionicProviders& providers() const { return providers_; }
  std::span<const NativeProviderPlacement> placements() const { return placements_; }
 private:
  SystemNativeImages() = default;
  SharedBionicProviders providers_;
  std::vector<NativeProviderPlacement> placements_;
};
}
