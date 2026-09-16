#pragma once

#include <string>

namespace darwin_art::loader {

enum class BootApexJniDecision { kNotApplicable, kAllowed, kDenied };

struct BootApexJniResolution {
  BootApexJniDecision decision = BootApexJniDecision::kNotApplicable;
  std::string path;
  std::string error;
};

// Applies Android's generated APEX JNI export policy. The returned host path
// is only the backing for an Android namespace decision; the caller still uses
// the Android ELF graph loader.
BootApexJniResolution ResolveBootApexJniLibrary(
    const char* requested_soname, const char* caller_location,
    const char* android_filesystem_root);

// Resolves a DT_NEEDED edge through linkerconfig's cross-APEX public exports.
// Empty means the dependency is not publicly exported.
std::string ResolvePublicApexLibrary(const char* soname,
                                     const char* android_filesystem_root);

}  // namespace darwin_art::loader
