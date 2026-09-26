#pragma once

#include <cstdlib>
#include <cstring>
#include <string_view>

// The Android process environment seen by Java. Variables the guest process
// snapshot defines (ANDROID_ROOT, ANDROID_DATA, ANDROID_STORAGE,
// EXTERNAL_STORAGE) carry their Android values, as Android's init sets them;
// the host's values for those names locate ART's own host runtime files and
// stay visible only to host native code. Other names are the profile-scoped
// host environment. A host-only image without the Bionic environment owner
// (the libcore ABI smoke) has no guest snapshot.
extern "C" char** darwin_art_bionic_environ __attribute__((weak_import));

namespace darwin_art::process {

inline std::string_view EnvironmentName(const char* entry) {
  const char* separator = std::strchr(entry, '=');
  return separator == nullptr ? std::string_view(entry)
                              : std::string_view(entry, separator - entry);
}

// The guest snapshot's "NAME=value" entry, or null when it does not define NAME.
inline const char* GuestEnvironmentEntry(std::string_view name) {
  if (&darwin_art_bionic_environ == nullptr) return nullptr;
  for (char** entry = darwin_art_bionic_environ; entry != nullptr && *entry != nullptr;
       ++entry) {
    if (EnvironmentName(*entry) == name) return *entry;
  }
  return nullptr;
}

inline const char* GuestGetenv(const char* name) {
  if (const char* guest = GuestEnvironmentEntry(name)) {
    return guest + std::strlen(name) + 1;
  }
  return std::getenv(name);
}

}  // namespace darwin_art::process
