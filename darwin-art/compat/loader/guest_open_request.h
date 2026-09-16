#pragma once
#include "../../tools/android-dso-namespace/include/darwin_art_dso_namespace.h"

namespace darwin_art::loader {
// Transitional legacy callback capability boundary, not Android flag policy.
// Unsupported contracts must fail before cache/provider lookup or side effects.
// The namespace-backed linker must replace this limitation, not erase flags.
inline const char* LegacyOpenRequestError(int flags, const DarwinArtAndroidDlExtInfo* info) {
  if (info && info->flags != 0)
    return "legacy guest loader does not implement Android extended namespace/file loading";
  // Android RTLD_LAZY=1, RTLD_NOW=2, RTLD_LOCAL=0 (not Darwin RTLD_LOCAL).
  if (flags != 1 && flags != 2)
    return "legacy guest loader does not implement the requested Android dlopen flags";
  return nullptr;
}
}
