#include "namespace_handles.h"
#include "namespace_operation.h"

extern "C" int darwin_art_bionic_android_fdsan_set_error_level_from_property(int);

namespace darwin_art::loader {
bool NamespaceHandles::SetTargetSdkVersion(int target) {
  // Android16 dlfcn.cpp holds the recursive loader lock through the complete
  // policy update. This is the same operation owner used by library opens.
  NamespaceOperation operation(configured_->registry());
  if (!operation) return false;
  if (target == 0) target = __ANDROID_API__;
  target_sdk_version_.store(target);
  if (target < 30) {
    // Original linker_sdk_versions.cpp uses WARN_ONCE as a default, not an
    // override: debug.fdsan is interpreted by the existing property owner.
    darwin_art_bionic_android_fdsan_set_error_level_from_property(1);
  }
  // The current libc allocator backend is Darwin malloc. Original libc's
  // target-SDK hook adjusts Scudo slack only in USE_SCUDO/non-HWASan builds;
  // no unrelated guest allocator is invoked for Darwin-owned allocations.
  return true;
}
}
